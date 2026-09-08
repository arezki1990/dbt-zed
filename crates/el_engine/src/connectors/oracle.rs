//! Oracle source connector (worker-side, feature "oracle"), on Oracle's
//! own pure-Rust thin driver (`oracledb`): it speaks the wire protocol
//! itself, so no Oracle client software is needed on any machine.
//! Credentials come from the environment (`ZDBT_EL_SRC_ORACLE_*`, see
//! `oracle_env`), never argv; a `TNS_ADMIN` directory is handed to the
//! driver as the place to find `tnsnames.ora` and a wallet. Setup, the
//! test container and the live tests: `el_engine/ORACLE.md`.
//!
//! Reading is a single server-side cursor pulled lazily one chunk per
//! `next_chunk` — no OFFSET re-scans, no whole result in memory. The
//! incremental cursor is a bind variable (`WHERE "k" > :1 ORDER BY "k"`),
//! never an interpolated literal, so a text watermark can never become
//! SQL.
//!
//! Identifiers follow Oracle's own folding: written unquoted in the spec
//! they are upper-cased (`employees` → `EMPLOYEES`, which is what
//! `CREATE TABLE employees` really made); to address a case-sensitive
//! table, quote it in the spec (`"MixedCase"`).

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{DateTime, Datelike as _, NaiveDate, NaiveDateTime, Timelike as _};
use oracledb::{Connection, Cursor, OracleNumber, OracleTimestamp, Row, ToDbValue};
use polars::prelude::*;

use super::oracle_env::ENV_TNS_ADMIN;
use crate::oracle_types::{OracleColumnType, normalize_ident, quote_ident, quote_stored};
use crate::state::WatermarkValue;

/// Rows fetched per server round trip. The driver buffers this many rows
/// per fetch, so the chunk size — 50,000 by default — must not drive it;
/// the chunk is still assembled lazily, one fetch at a time.
const FETCH_ARRAY_ROWS: u32 = 1_000;

/// Whole-handshake budget. Oracle's own connect timeout only covers the
/// TCP leg, and loading the client library plus a TNS lookup can be slow
/// on a cold machine — but a wedged listener must still become an error.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-column fetch strategy, derived from the dictionary type through
/// the single mapping in `oracle_types`.
#[derive(Clone, Debug, PartialEq)]
enum Fetch {
    Bool,
    Int,
    Float,
    /// Exact `NUMBER(p,s)`: fetched as text, parsed into a decimal column.
    Decimal(usize, usize),
    Text,
    Timestamp,
    TimestampTz,
    Binary,
}

impl Fetch {
    fn of(dtype: &DataType) -> Self {
        match dtype {
            DataType::Boolean => Fetch::Bool,
            DataType::Int64 => Fetch::Int,
            DataType::Float64 => Fetch::Float,
            DataType::Decimal(precision, scale) => Fetch::Decimal(*precision, *scale),
            DataType::Datetime(_, None) => Fetch::Timestamp,
            DataType::Datetime(_, Some(_)) => Fetch::TimestampTz,
            DataType::Binary => Fetch::Binary,
            _ => Fetch::Text,
        }
    }

    fn dtype(&self) -> DataType {
        match self {
            Fetch::Bool => DataType::Boolean,
            Fetch::Int => DataType::Int64,
            Fetch::Float => DataType::Float64,
            Fetch::Decimal(precision, scale) => DataType::Decimal(*precision, *scale),
            Fetch::Text => DataType::String,
            Fetch::Timestamp => DataType::Datetime(TimeUnit::Microseconds, None),
            Fetch::TimestampTz => {
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
            }
            Fetch::Binary => DataType::Binary,
        }
    }
}

// -- connecting ------------------------------------------------------------

/// Turns a driver error into an app error. Oracle's own text names ORA
/// codes but never our credentials. The one driver-side refusal worth
/// translating is the server-version one: the thin protocol needs Oracle
/// Database 12.1 or later, and "not supported" alone does not say so.
pub fn describe_error(error: oracledb::Error) -> anyhow::Error {
    if matches!(error.kind(), oracledb::ErrorKind::ServerVersionNotSupported) {
        anyhow!(
            "this Oracle server is older than 12.1 (10g or 11g): the connector's thin driver \
             speaks only the protocol of Oracle Database 12.1 and later, so it cannot read \
             or load this database"
        )
    } else {
        anyhow!("{error}")
    }
}

/// Connects with the credentials the parent put in the environment.
pub fn connect_from_env() -> Result<Connection> {
    let creds = super::oracle_env::creds_from_env()?;
    connect(&creds.user, &creds.password, &creds.connect)
}

/// Connects, bounding the WHOLE handshake: the driver's own timeouts
/// cover the socket only, so the attempt runs on its own thread and is
/// abandoned at the deadline. Errors never echo the user or the connect
/// string.
pub fn connect(user: &str, password: &str, connect_string: &str) -> Result<Connection> {
    let mut config = oracledb::Config::default().set_credentials(user, password);
    // A wallet / tnsnames directory: the driver resolves an alias through
    // its tnsnames.ora and reads a wallet's ewallet.pem from there.
    if let Some(dir) = std::env::var_os(ENV_TNS_ADMIN).filter(|dir| !dir.is_empty()) {
        let dir = dir.to_string_lossy().into_owned();
        config = config.set_config_dir(&dir).set_wallet_location(dir);
    }
    let config = config
        .set_connect_string(connect_string)
        .map_err(describe_error)
        .context("parsing the oracle connect string")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let attempt = std::thread::spawn(move || {
        let _ = tx.send(oracledb::connect(config));
    });
    let conn = match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(result) => result.map_err(describe_error).context("connecting to oracle")?,
        Err(_) => {
            drop(attempt); // the thread dies with the process
            bail!(
                "connecting to oracle timed out after {}s — is the listener reachable?",
                CONNECT_TIMEOUT.as_secs()
            )
        }
    };
    // Deterministic session: timestamps with a zone come back as UTC and
    // numbers print with a dot, whatever the server's NLS defaults are.
    for statement in [
        "ALTER SESSION SET TIME_ZONE = 'UTC'",
        "ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,'",
    ] {
        conn.execute(statement, &[])
            .map_err(describe_error)
            .with_context(|| format!("running {statement}"))?;
    }
    Ok(conn)
}

/// A cell as text, whatever its type: the explorer's grid and the text
/// fetch path both want Oracle's own rendering rather than a conversion
/// error. Bytes render as hex.
pub fn cell_text(row: &Row, index: usize) -> Result<Option<String>, oracledb::Error> {
    if let Ok(value) = row.get::<Option<String>>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.get::<Option<OracleNumber>>(index) {
        return Ok(value.map(|value| value.to_string()));
    }
    if let Ok(value) = row.get::<Option<OracleTimestamp>>(index) {
        return Ok(value.map(|value| value.to_string()));
    }
    if let Ok(value) = row.get::<Option<oracledb::OracleIntervalDS>>(index) {
        return Ok(value.map(|value| value.to_string()));
    }
    if let Ok(value) = row.get::<Option<oracledb::OracleIntervalYM>>(index) {
        return Ok(value.map(|value| value.to_string()));
    }
    if let Ok(value) = row.get::<Option<bool>>(index) {
        return Ok(value.map(|value| value.to_string()));
    }
    row.get::<Option<Vec<u8>>>(index).map(|value| {
        value.map(|bytes| bytes.iter().map(|byte| format!("{byte:02X}")).collect())
    })
}

// -- explorer SQL ----------------------------------------------------------

/// Owners the table explorer never lists: Oracle's own dictionary and
/// option schemas, which every account can see and nobody extracts from.
const SYSTEM_OWNERS: &str = "'SYS','SYSTEM','XDB','OUTLN','DBSNMP','APPQOSSYS','AUDSYS',\
'CTXSYS','MDSYS','MDDATA','ORDSYS','ORDDATA','ORDPLUGINS','OLAPSYS','WMSYS','LBACSYS',\
'OJVMSYS','DVSYS','DVF','DBSFWUSER','GSMADMIN_INTERNAL','GGSYS','SI_INFORMTN_SCHEMA',\
'ANONYMOUS','REMOTE_SCHEDULER_AGENT','SYSBACKUP','SYSDG','SYSKM','SYSRAC','SYS$UMF',\
'ORACLE_OCM','XS$NULL','FLOWS_FILES','APEX_PUBLIC_USER','PDBADMIN'";

/// Every (owner, name) the account can see — tables and views, minus the
/// dictionary schemas and the recycle bin.
pub fn list_tables_sql() -> String {
    format!(
        "SELECT owner, table_name FROM all_tables \
         WHERE owner NOT IN ({SYSTEM_OWNERS}) AND table_name NOT LIKE 'BIN$%' \
         UNION ALL \
         SELECT owner, view_name FROM all_views WHERE owner NOT IN ({SYSTEM_OWNERS}) \
         ORDER BY 1, 2"
    )
}

/// Caps a user query by wrapping it. Oracle has no `LIMIT`, takes no `AS`
/// on a table alias, and rejects a trailing semicolon inside a subquery.
pub fn capped_query_sql(sql: &str, limit: usize) -> String {
    let inner = sql.trim().trim_end_matches(';').trim_end();
    format!("SELECT * FROM ({inner}) zdbt_q FETCH FIRST {limit} ROWS ONLY")
}

/// The column dictionary for one (owner, table). `ALL_TAB_COLUMNS` covers
/// views as well as tables and already hides system-generated columns.
pub const COLUMNS_SQL: &str = "SELECT column_name, data_type, data_precision, data_scale \
     FROM all_tab_columns WHERE owner = :1 AND table_name = :2 ORDER BY column_id";

/// The extraction statement for a table, with the cursor bound as `:1`.
/// `owner`, `table` and the column names are already in their stored
/// form; only `update_key` comes from the spec and is folded here.
fn select_sql(
    owner: &str,
    table: &str,
    columns: &[(String, Fetch)],
    update_key: Option<&str>,
) -> Result<String> {
    let select_list = columns
        .iter()
        .map(|(name, _)| quote_stored(name))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let relation = format!("{}.{}", quote_stored(owner)?, quote_stored(table)?);
    Ok(match update_key {
        Some(column) => {
            let column = quote_ident(column)?;
            format!("SELECT {select_list} FROM {relation} WHERE {column} > :1 ORDER BY {column}")
        }
        None => format!("SELECT {select_list} FROM {relation}"),
    })
}

// -- the extractor ---------------------------------------------------------

pub struct OracleExtractor {
    columns: Vec<(String, Fetch)>,
    /// The open cursor; `None` once the server ran out of rows.
    rows: Option<Cursor>,
    chunk_rows: usize,
    /// Declared after `rows` so the cursor is dropped before the session.
    _conn: Connection,
}

impl OracleExtractor {
    pub fn new(
        user: &str,
        password: &str,
        connect_string: &str,
        schema: &str,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, WatermarkValue)>,
    ) -> Result<Self> {
        let conn = connect(user, password, connect_string)?;
        Self::with_connection(conn, schema, table, chunk_rows, cursor)
    }

    /// The same construction on an already-open session — the smoke test's
    /// entry point, and where the explorer would reuse a connection.
    pub fn with_connection(
        conn: Connection,
        schema: &str,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, WatermarkValue)>,
    ) -> Result<Self> {
        let owner = normalize_ident(schema)?;
        let table_name = normalize_ident(table)?;
        let chunk_rows = chunk_rows.max(1);

        let columns = probe_columns(&conn, &owner, &table_name)?;
        let sql = select_sql(
            &owner,
            &table_name,
            &columns,
            cursor.as_ref().map(|(column, _)| column.as_str()),
        )?;

        let bind: Option<Box<dyn ToDbValue>> = match &cursor {
            Some((column, value)) => Some(
                watermark_bind(value)
                    .with_context(|| format!("binding the cursor on {column:?}"))?,
            ),
            None => None,
        };
        let params: Vec<&dyn ToDbValue> = bind.iter().map(|value| value.as_ref()).collect();

        let mut statement = conn
            .statement(&sql)
            .map_err(describe_error)
            .with_context(|| format!("preparing the read of {owner}.{table_name}"))?;
        let rows_per_fetch = fetch_array_size(chunk_rows);
        statement
            .fetch_array_size(rows_per_fetch)
            .prefetch_rows(rows_per_fetch);
        let rows = statement
            .query(&params)
            .map_err(describe_error)
            .with_context(|| format!("reading {owner}.{table_name}"))?;

        Ok(Self {
            columns,
            rows: Some(rows),
            chunk_rows,
            _conn: conn,
        })
    }
}

/// Rows per fetch: the chunk size for small chunks, the cap otherwise.
fn fetch_array_size(chunk_rows: usize) -> u32 {
    u32::try_from(chunk_rows)
        .unwrap_or(u32::MAX)
        .clamp(1, FETCH_ARRAY_ROWS)
}

/// The source schema, from the dictionary rather than the driver's own
/// descriptors: `ALL_TAB_COLUMNS` carries the declared precision and
/// scale, which is what decides Int64 vs an exact decimal.
fn probe_columns(conn: &Connection, owner: &str, table: &str) -> Result<Vec<(String, Fetch)>> {
    let rows = conn
        .query(COLUMNS_SQL, &[&owner, &table])
        .map_err(describe_error)
        .with_context(|| format!("describing {owner}.{table}"))?;
    let mut columns = Vec::new();
    for row in rows {
        let row = row
            .map_err(describe_error)
            .with_context(|| format!("describing {owner}.{table}"))?;
        let name = row.get::<String>(0).map_err(describe_error)?;
        let data_type = row.get::<String>(1).map_err(describe_error)?;
        let precision = row
            .get::<Option<i64>>(2)
            .unwrap_or(None)
            .and_then(|value| u8::try_from(value).ok());
        let scale = row
            .get::<Option<i64>>(3)
            .unwrap_or(None)
            .and_then(|value| i8::try_from(value).ok());
        let column_type = OracleColumnType::from_dictionary(&data_type, precision, scale);
        columns.push((name, Fetch::of(&column_type.polars_dtype())));
    }
    if columns.is_empty() {
        bail!("{owner}.{table} has no columns — does it exist and is it granted to this user?");
    }
    Ok(columns)
}

/// The typed bind for an incremental cursor. Every variant binds as its
/// own Oracle type: a text watermark never reaches a DATE comparison as
/// an implicitly converted string.
fn watermark_bind(value: &WatermarkValue) -> Result<Box<dyn ToDbValue>> {
    Ok(match value {
        WatermarkValue::Int(value) => Box::new(*value),
        WatermarkValue::Float(value) => Box::new(*value),
        WatermarkValue::Text(value) => Box::new(value.clone()),
        WatermarkValue::Timestamp(micros) => Box::new(oracle_timestamp(
            naive_from_micros(*micros)
                .ok_or_else(|| anyhow!("cursor timestamp {micros} is out of range"))?,
        )),
        WatermarkValue::Date(days) => {
            let date =
                date_from_days(*days).ok_or_else(|| anyhow!("cursor date {days} is out of range"))?;
            Box::new(OracleTimestamp::new_date(
                date.year() as i16,
                date.month() as u8,
                date.day() as u8,
            ))
        }
    })
}

fn naive_from_micros(micros: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp_micros(micros).map(|value| value.naive_utc())
}

fn date_from_days(days: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(1970, 1, 1)?.checked_add_signed(chrono::TimeDelta::days(days.into()))
}

/// A naive wall clock as the driver's timestamp value (no zone).
pub fn oracle_timestamp(value: NaiveDateTime) -> OracleTimestamp {
    OracleTimestamp::new_timestamp(
        value.year() as i16,
        value.month() as u8,
        value.day() as u8,
        value.hour() as u8,
        value.minute() as u8,
        value.second() as u8,
        value.nanosecond(),
    )
}

/// Microseconds since the epoch for a fetched timestamp. For a zoned
/// column the driver hands back the UTC clock with the stored offset
/// alongside, and the session runs in UTC, so the fields are the instant
/// in every case and the offset is not applied again.
fn micros_of(value: &OracleTimestamp) -> Result<i64> {
    NaiveDate::from_ymd_opt(value.year().into(), value.month().into(), value.day().into())
        .and_then(|date| {
            date.and_hms_nano_opt(
                value.hour().into(),
                value.minute().into(),
                value.second().into(),
                value.nanoseconds(),
            )
        })
        .map(|naive| naive.and_utc().timestamp_micros())
        .ok_or_else(|| anyhow!("timestamp {value} is out of range"))
}

/// One column's values for the chunk being built.
enum Values {
    Bool(Vec<Option<bool>>),
    Int(Vec<Option<i64>>),
    Float(Vec<Option<f64>>),
    Decimal(Vec<Option<String>>),
    Text(Vec<Option<String>>),
    Timestamp(Vec<Option<i64>>),
    TimestampTz(Vec<Option<i64>>),
    Binary(Vec<Option<Vec<u8>>>),
}

fn make_buffers(columns: &[(String, Fetch)]) -> Vec<Values> {
    columns
        .iter()
        .map(|(_, fetch)| match fetch {
            Fetch::Bool => Values::Bool(Vec::new()),
            Fetch::Int => Values::Int(Vec::new()),
            Fetch::Float => Values::Float(Vec::new()),
            Fetch::Decimal(..) => Values::Decimal(Vec::new()),
            Fetch::Text => Values::Text(Vec::new()),
            Fetch::Timestamp => Values::Timestamp(Vec::new()),
            Fetch::TimestampTz => Values::TimestampTz(Vec::new()),
            Fetch::Binary => Values::Binary(Vec::new()),
        })
        .collect()
}

fn push_row(row: &Row, columns: &[(String, Fetch)], buffers: &mut [Values]) -> Result<()> {
    for (index, buffer) in buffers.iter_mut().enumerate() {
        let name = columns[index].0.as_str();
        let fail = |error: oracledb::Error| describe_error(error).context(format!("column {name}"));
        match buffer {
            Values::Bool(values) => values.push(row.get(index).map_err(fail)?),
            Values::Int(values) => values.push(row.get(index).map_err(fail)?),
            Values::Float(values) => values.push(row.get(index).map_err(fail)?),
            // NUMBER arrives as the driver's exact decimal and stays exact
            // as text all the way into the decimal column.
            Values::Decimal(values) => values.push(
                row.get::<Option<OracleNumber>>(index)
                    .map_err(fail)?
                    .map(|value| value.to_string()),
            ),
            Values::Text(values) => values.push(cell_text(row, index).map_err(fail)?),
            Values::Timestamp(values) | Values::TimestampTz(values) => values.push(
                row.get::<Option<OracleTimestamp>>(index)
                    .map_err(fail)?
                    .map(|value| micros_of(&value))
                    .transpose()
                    .with_context(|| format!("column {name}"))?,
            ),
            Values::Binary(values) => values.push(row.get(index).map_err(fail)?),
        }
    }
    Ok(())
}

fn flush(columns: &[(String, Fetch)], buffers: Vec<Values>) -> Result<DataFrame> {
    let series: Vec<polars::prelude::Column> = columns
        .iter()
        .zip(buffers)
        .map(|((name, fetch), buffer)| -> Result<polars::prelude::Column> {
            let name = name.as_str();
            let series = match buffer {
                Values::Bool(values) => Series::new(name.into(), values),
                Values::Int(values) => Series::new(name.into(), values),
                Values::Float(values) => Series::new(name.into(), values),
                Values::Text(values) => Series::new(name.into(), values),
                // Strict: a value that does not fit is an error, never a
                // silent NULL. Text with more decimals than the scale is
                // rounded by the parser, which only the unconstrained
                // NUMBER mapping (scale 10) can hit.
                Values::Decimal(values) => Series::new(name.into(), values)
                    .strict_cast(&fetch.dtype())
                    .map_err(|error| anyhow!("decimal column {name}: {error}"))?,
                Values::Timestamp(values) | Values::TimestampTz(values) => {
                    Series::new(name.into(), values)
                        .strict_cast(&fetch.dtype())
                        .map_err(|error| anyhow!("timestamp column {name}: {error}"))?
                }
                Values::Binary(values) => Series::new(name.into(), values),
            };
            Ok(series.into())
        })
        .collect::<Result<_>>()?;
    let height = series.first().map(|column| column.len()).unwrap_or(0);
    DataFrame::new(height, series).map_err(|error| anyhow!("building chunk: {error}"))
}

impl super::Extractor for OracleExtractor {
    fn schema(&mut self) -> Result<Schema> {
        Ok(Schema::from_iter(
            self.columns
                .iter()
                .map(|(name, fetch)| (name.as_str().into(), fetch.dtype())),
        ))
    }

    fn next_chunk(&mut self) -> Result<Option<DataFrame>> {
        let Some(rows) = self.rows.as_mut() else {
            return Ok(None);
        };
        let mut buffers = make_buffers(&self.columns);
        let mut height = 0usize;
        while height < self.chunk_rows {
            match rows.next() {
                None => {
                    // Exhausted: close the cursor, keep the session for
                    // Drop.
                    self.rows = None;
                    break;
                }
                Some(Err(error)) => {
                    return Err(describe_error(error).context("reading an oracle row"));
                }
                Some(Ok(row)) => {
                    push_row(&row, &self.columns, &mut buffers)?;
                    height += 1;
                }
            }
        }
        if height == 0 {
            return Ok(None);
        }
        Ok(Some(flush(&self.columns, buffers)?))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Extractor as _;
    use super::*;

    #[test]
    fn identifiers_fold_like_oracle() {
        assert_eq!(normalize_ident("employees").unwrap(), "EMPLOYEES");
        assert_eq!(normalize_ident("  Hr_Staff ").unwrap(), "HR_STAFF");
        assert_eq!(normalize_ident("\"MixedCase\"").unwrap(), "MixedCase");
        assert_eq!(quote_ident("employees").unwrap(), "\"EMPLOYEES\"");
        assert_eq!(quote_ident("\"MixedCase\"").unwrap(), "\"MixedCase\"");
        for rejected in ["", "  ", "we\"ird", "\"open", "\"a\"b\""] {
            assert!(
                normalize_ident(rejected).is_err(),
                "{rejected:?} should be rejected"
            );
        }
    }

    #[test]
    fn select_binds_the_cursor() {
        // Owner, table and column names are the stored ones: quoted as
        // they are, never folded a second time.
        let columns = vec![
            ("ID".to_owned(), Fetch::Int),
            ("UPDATED_AT".to_owned(), Fetch::Timestamp),
        ];
        assert_eq!(
            select_sql("HR", "EMPLOYEES", &columns, None).unwrap(),
            "SELECT \"ID\", \"UPDATED_AT\" FROM \"HR\".\"EMPLOYEES\""
        );
        assert_eq!(
            select_sql("HR", "EMPLOYEES", &columns, Some("updated_at")).unwrap(),
            "SELECT \"ID\", \"UPDATED_AT\" FROM \"HR\".\"EMPLOYEES\" \
             WHERE \"UPDATED_AT\" > :1 ORDER BY \"UPDATED_AT\""
        );
        // A case-sensitive table keeps its case all the way through.
        let mixed = vec![("id".to_owned(), Fetch::Int)];
        assert_eq!(
            select_sql("HR", "MixedCase", &mixed, None).unwrap(),
            "SELECT \"id\" FROM \"HR\".\"MixedCase\""
        );
        // A hostile column name never reaches the statement.
        assert!(select_sql("HR", "EMPLOYEES", &columns, Some("a\" OR 1=1--")).is_err());
        assert!(
            select_sql("HR", "EMPLOYEES", &[("a\"b".to_owned(), Fetch::Int)], None).is_err()
        );
    }

    #[test]
    fn explorer_sql_is_oracle_shaped() {
        let list = list_tables_sql();
        assert!(list.contains("all_tables"), "{list}");
        assert!(list.contains("all_views"), "{list}");
        assert!(list.contains("'SYS'"), "{list}");
        assert!(!list.contains("information_schema"), "{list}");
        // No LIMIT, no AS on the alias, no trailing semicolon inside.
        assert_eq!(
            capped_query_sql("SELECT * FROM hr.employees;\n", 100),
            "SELECT * FROM (SELECT * FROM hr.employees) zdbt_q FETCH FIRST 100 ROWS ONLY"
        );
    }

    #[test]
    fn cursor_values_bind_as_their_own_type() {
        // Every variant must produce a bind; nothing falls back to text.
        for value in [
            WatermarkValue::Int(7),
            WatermarkValue::Float(1.5),
            WatermarkValue::Text("a'b".to_owned()),
            WatermarkValue::Timestamp(1_767_225_600_000_000),
            WatermarkValue::Date(20_000),
        ] {
            assert!(watermark_bind(&value).is_ok(), "{value:?}");
        }
        assert!(watermark_bind(&WatermarkValue::Timestamp(i64::MAX)).is_err());
        assert_eq!(
            naive_from_micros(1_767_225_600_000_000)
                .unwrap()
                .to_string(),
            "2026-01-01 00:00:00"
        );
        assert_eq!(date_from_days(20_000).unwrap().to_string(), "2024-10-04");
        assert_eq!(date_from_days(-1).unwrap().to_string(), "1969-12-31");
    }

    /// The exactness contract: `NUMBER(p,s)` text becomes a decimal
    /// column, not a float.
    #[test]
    fn decimal_columns_keep_their_scale() {
        let columns = vec![
            ("AMOUNT".to_owned(), Fetch::Decimal(18, 2)),
            ("NAME".to_owned(), Fetch::Text),
        ];
        let mut buffers = make_buffers(&columns);
        match &mut buffers[0] {
            Values::Decimal(values) => {
                values.push(Some("10.5".to_owned()));
                values.push(Some("-2.25".to_owned()));
                values.push(None);
            }
            _ => panic!("decimal buffer"),
        }
        match &mut buffers[1] {
            Values::Text(values) => {
                values.extend([Some("ada".to_owned()), None, Some("alan".to_owned())])
            }
            _ => panic!("text buffer"),
        }
        let frame = flush(&columns, buffers).unwrap();
        assert_eq!(frame.height(), 3);
        assert_eq!(
            frame.column("AMOUNT").unwrap().dtype(),
            &DataType::Decimal(18, 2)
        );
        assert_eq!(
            frame.column("AMOUNT").unwrap().get(0).unwrap().to_string(),
            "10.50"
        );
        assert!(frame.column("AMOUNT").unwrap().get(2).unwrap().is_null());
    }

    /// Unconstrained NUMBER lands as Decimal(38,10): a 19-digit id stays
    /// exact (it would not in an f64), Oracle's leading-dot and exponent
    /// spellings parse, and a value too wide for the column is an error
    /// rather than a NULL.
    #[test]
    fn unconstrained_numbers_stay_exact() {
        let columns = vec![("N".to_owned(), Fetch::Decimal(38, 10))];
        let mut buffers = make_buffers(&columns);
        let Values::Decimal(values) = &mut buffers[0] else {
            panic!("decimal buffer");
        };
        values.push(Some("1234567890123456789".to_owned()));
        values.push(Some(".5".to_owned()));
        values.push(Some("-1.5E+3".to_owned()));
        values.push(Some("3.14159265358979".to_owned()));
        let frame = flush(&columns, buffers).unwrap();
        let column = frame.column("N").unwrap();
        assert_eq!(column.get(0).unwrap().to_string(), "1234567890123456789.0000000000");
        assert_eq!(column.get(1).unwrap().to_string(), "0.5000000000");
        assert_eq!(column.get(2).unwrap().to_string(), "-1500.0000000000");
        assert_eq!(column.get(3).unwrap().to_string(), "3.1415926536");

        let mut buffers = make_buffers(&columns);
        let Values::Decimal(values) = &mut buffers[0] else {
            panic!("decimal buffer");
        };
        values.push(Some("1".repeat(30)));
        let error = flush(&columns, buffers).unwrap_err().to_string();
        assert!(error.contains("decimal column N"), "{error}");
    }

    /// The default chunk (50,000 rows) must not size the fetch buffers.
    #[test]
    fn fetch_array_size_is_capped() {
        assert_eq!(fetch_array_size(50_000), FETCH_ARRAY_ROWS);
        assert_eq!(fetch_array_size(2), 2);
        assert_eq!(fetch_array_size(0), 1);
    }

    /// Live smoke, gated: `EL_ORACLE_SMOKE_URL=host:1521/service
    /// EL_ORACLE_SMOKE_USER=… EL_ORACLE_SMOKE_PASSWORD=… cargo test -p
    /// el_engine --features oracle -- --ignored oracle_smoke --nocapture`
    /// (needs a reachable database; no client software).
    #[test]
    #[ignore]
    fn oracle_smoke() {
        let connect_string = std::env::var("EL_ORACLE_SMOKE_URL").expect("set EL_ORACLE_SMOKE_URL");
        let user = std::env::var("EL_ORACLE_SMOKE_USER").expect("set EL_ORACLE_SMOKE_USER");
        let password =
            std::env::var("EL_ORACLE_SMOKE_PASSWORD").expect("set EL_ORACLE_SMOKE_PASSWORD");

        let conn = connect(&user, &password, &connect_string).unwrap();
        let _ = conn.execute(
            "BEGIN EXECUTE IMMEDIATE 'DROP TABLE ZDBT_EL_SMOKE PURGE'; \
             EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;",
            &[],
        );
        conn.execute(
            "CREATE TABLE ZDBT_EL_SMOKE ( \
               ID NUMBER(10,0), NAME VARCHAR2(50), AMOUNT NUMBER(18,2), \
               ACTIVE NUMBER(1), NOTE CLOB, PAYLOAD RAW(16), \
               BORN DATE, UPDATED TIMESTAMP(6) WITH TIME ZONE)",
            &[],
        )
        .unwrap();
        // Row 1 is stamped +02:00: it must read as 08:00 UTC, not 10:00.
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (1, 'ada', 10.50, 1, 'note', \
             HEXTORAW('DEADBEEF'), DATE '1990-03-01', \
             TIMESTAMP '2026-01-01 10:00:00 +02:00')",
            &[],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (2, 'grace', NULL, 0, NULL, NULL, NULL, \
             TIMESTAMP '2026-01-02 11:30:00 +00:00')",
            &[],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (3, 'alan', -2.25, 1, 'x', NULL, \
             DATE '1985-12-31', TIMESTAMP '2026-01-03 12:00:00 +00:00')",
            &[],
        )
        .unwrap();
        conn.commit().unwrap();
        let schema_owner: String = conn
            .query_row("SELECT USER FROM DUAL", &[])
            .unwrap()
            .get(0)
            .unwrap();

        let mut extractor = OracleExtractor::new(
            &user,
            &password,
            &connect_string,
            &schema_owner,
            "ZDBT_EL_SMOKE",
            2,
            None,
        )
        .unwrap();
        let schema = extractor.schema().unwrap();
        assert_eq!(schema.get("ID").unwrap(), &DataType::Int64);
        assert_eq!(schema.get("AMOUNT").unwrap(), &DataType::Decimal(18, 2));
        assert_eq!(schema.get("NOTE").unwrap(), &DataType::String);
        assert_eq!(schema.get("PAYLOAD").unwrap(), &DataType::Binary);
        assert_eq!(
            schema.get("UPDATED").unwrap(),
            &DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
        );
        let mut total = 0;
        let mut first_updated = None;
        while let Some(chunk) = extractor.next_chunk().unwrap() {
            assert!(chunk.height() <= 2);
            if first_updated.is_none() {
                first_updated = chunk
                    .column("UPDATED")
                    .unwrap()
                    .as_materialized_series()
                    .to_physical_repr()
                    .i64()
                    .unwrap()
                    .get(0);
            }
            total += chunk.height();
        }
        assert_eq!(total, 3);
        // 2026-01-01 08:00:00 UTC.
        assert_eq!(first_updated, Some(1_767_254_400_000_000));

        // Incremental: only rows past the cursor come back.
        // 2026-01-02 12:00 UTC — after row 2, before row 3.
        let cursor = WatermarkValue::Timestamp(1_767_355_200_000_000);
        let mut incremental = OracleExtractor::new(
            &user,
            &password,
            &connect_string,
            &schema_owner,
            "ZDBT_EL_SMOKE",
            10,
            Some(("UPDATED".to_owned(), cursor)),
        )
        .unwrap();
        let delta = incremental.next_chunk().unwrap().map(|c| c.height());
        println!("oracle smoke ok: {total} rows, incremental {delta:?}");
        assert_eq!(delta, Some(1));
    }
}
