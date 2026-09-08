//! Oracle source connector (worker-side, feature "oracle"). The `oracle`
//! crate wraps ODPI-C, which compiles anywhere but dlopens Oracle Instant
//! Client at run time — a missing client is DPI-1047 and becomes the
//! actionable message in [`INSTANT_CLIENT_HELP`]. Credentials come from
//! the environment (`ZDBT_EL_SRC_ORACLE_*`, see `oracle_env`), never argv.
//!
//! Reading is a single server-side cursor with the fetch array size set to
//! the chunk size, pulled lazily one chunk per `next_chunk` — no OFFSET
//! re-scans, no whole result in memory. The incremental cursor is a bind
//! variable (`WHERE "k" > :1 ORDER BY "k"`), never an interpolated
//! literal, so a text watermark can never become SQL.
//!
//! Identifiers follow Oracle's own folding: written unquoted in the spec
//! they are upper-cased (`employees` → `EMPLOYEES`, which is what
//! `CREATE TABLE employees` really made); to address a case-sensitive
//! table, quote it in the spec (`"MixedCase"`).

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use oracle::sql_type::ToSql;
use oracle::{Connection, ResultSet, Row};
use polars::prelude::*;

use crate::oracle_types::OracleColumnType;
use crate::state::WatermarkValue;

/// What every live path says when ODPI-C cannot find a client library.
pub const INSTANT_CLIENT_HELP: &str = concat!(
    "Oracle Instant Client is not installed on the machine running the connector worker ",
    "(ODPI-C reported DPI-1047). Install the Basic or Basic Light package and make it ",
    "visible to the process: macOS — the arm64 instantclient dmg from oracle.com, kept in ",
    "~/lib or named by DYLD_LIBRARY_PATH; Linux — unzip the client, install libaio, then add ",
    "the directory to LD_LIBRARY_PATH or /etc/ld.so.conf.d and run ldconfig; Windows — add ",
    "the directory to PATH."
);

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

// -- identifiers -----------------------------------------------------------

/// Folds an identifier the way Oracle folds an unquoted one — upper case
/// — unless the spec quoted it, in which case the case is kept verbatim.
/// A stray double quote is rejected rather than escaped: an Oracle object
/// name with an embedded quote is not something a spec should address.
pub fn normalize_ident(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("empty Oracle identifier");
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        let inner = rest
            .strip_suffix('"')
            .ok_or_else(|| anyhow!("unbalanced quotes in Oracle identifier {name:?}"))?;
        if inner.is_empty() || inner.contains('"') {
            bail!("Oracle identifier {name:?} contains a double quote");
        }
        return Ok(inner.to_owned());
    }
    if trimmed.contains('"') {
        bail!("Oracle identifier {name:?} contains a double quote");
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// Quotes a name Oracle itself gave us — a dictionary column, or a name
/// already through [`normalize_ident`] — verbatim. Folding it again would
/// break a genuinely lower-case column.
fn quote_stored(name: &str) -> Result<String> {
    if name.is_empty() || name.contains('"') {
        bail!("Oracle identifier {name:?} is not addressable");
    }
    Ok(format!("\"{name}\""))
}

/// The normalized identifier, quoted for a statement.
pub fn quote_ident(name: &str) -> Result<String> {
    quote_stored(&normalize_ident(name)?)
}

// -- connecting ------------------------------------------------------------

/// Turns a driver error into an app error. DPI-1047 (no client library)
/// becomes the actionable install message; everything else keeps Oracle's
/// own text, which names ORA codes but never our credentials.
pub fn describe_error(error: oracle::Error) -> anyhow::Error {
    if error.dpi_code() == Some(1047) || error.to_string().contains("DPI-1047") {
        anyhow!(INSTANT_CLIENT_HELP)
    } else {
        anyhow!("{error}")
    }
}

/// Connects with the credentials the parent put in the environment.
pub fn connect_from_env() -> Result<Connection> {
    let creds = super::oracle_env::creds_from_env()?;
    connect(&creds.user, &creds.password, &creds.connect)
}

/// Connects, bounding the WHOLE handshake: ODPI-C's own timeouts cover
/// the socket only, so the attempt runs on its own thread and is
/// abandoned at the deadline. Errors never echo the user or the connect
/// string.
pub fn connect(user: &str, password: &str, connect_string: &str) -> Result<Connection> {
    let (user, password, connect_string) = (
        user.to_owned(),
        password.to_owned(),
        connect_string.to_owned(),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let attempt = std::thread::spawn(move || {
        let _ = tx.send(Connection::connect(&user, &password, &connect_string));
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
    rows: Option<ResultSet<'static, Row>>,
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

        let bind: Option<Box<dyn ToSql>> = match &cursor {
            Some((column, value)) => Some(
                watermark_bind(value)
                    .with_context(|| format!("binding the cursor on {column:?}"))?,
            ),
            None => None,
        };
        let params: Vec<&dyn ToSql> = bind.iter().map(|value| value.as_ref()).collect();

        let statement = conn
            .statement(&sql)
            // One server round trip per chunk.
            .fetch_array_size(chunk_rows as u32)
            .build()
            .map_err(describe_error)
            .with_context(|| format!("preparing the read of {owner}.{table_name}"))?;
        let rows = statement
            .into_result_set::<Row>(&params)
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
        let name = row.get::<usize, String>(0).map_err(describe_error)?;
        let data_type = row.get::<usize, String>(1).map_err(describe_error)?;
        let precision = row.get::<usize, Option<u8>>(2).unwrap_or(None);
        let scale = row.get::<usize, Option<i8>>(3).unwrap_or(None);
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
fn watermark_bind(value: &WatermarkValue) -> Result<Box<dyn ToSql>> {
    Ok(match value {
        WatermarkValue::Int(value) => Box::new(*value),
        WatermarkValue::Float(value) => Box::new(*value),
        WatermarkValue::Text(value) => Box::new(value.clone()),
        WatermarkValue::Timestamp(micros) => Box::new(
            naive_from_micros(*micros)
                .ok_or_else(|| anyhow!("cursor timestamp {micros} is out of range"))?,
        ),
        WatermarkValue::Date(days) => Box::new(
            date_from_days(*days).ok_or_else(|| anyhow!("cursor date {days} is out of range"))?,
        ),
    })
}

fn naive_from_micros(micros: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp_micros(micros).map(|value| value.naive_utc())
}

fn date_from_days(days: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(1970, 1, 1)?.checked_add_signed(chrono::TimeDelta::days(days.into()))
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
        let fail = |error: oracle::Error| describe_error(error).context(format!("column {name}"));
        match buffer {
            Values::Bool(values) => values.push(row.get(index).map_err(fail)?),
            Values::Int(values) => values.push(row.get(index).map_err(fail)?),
            Values::Float(values) => values.push(row.get(index).map_err(fail)?),
            // NUMBER arrives as text and stays exact all the way into the
            // decimal column.
            Values::Decimal(values) | Values::Text(values) => {
                values.push(row.get(index).map_err(fail)?)
            }
            Values::Timestamp(values) => values.push(
                row.get::<_, Option<NaiveDateTime>>(index)
                    .map_err(fail)?
                    .map(|value| value.and_utc().timestamp_micros()),
            ),
            Values::TimestampTz(values) => values.push(
                row.get::<_, Option<DateTime<Utc>>>(index)
                    .map_err(fail)?
                    .map(|value| value.timestamp_micros()),
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
                Values::Decimal(values) => Series::new(name.into(), values)
                    .cast(&fetch.dtype())
                    .map_err(|error| anyhow!("decimal column {name}: {error}"))?,
                Values::Timestamp(values) | Values::TimestampTz(values) => {
                    Series::new(name.into(), values)
                        .cast(&fetch.dtype())
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

    /// Live smoke, gated: `EL_ORACLE_SMOKE_URL=host:1521/service
    /// EL_ORACLE_SMOKE_USER=… EL_ORACLE_SMOKE_PASSWORD=… cargo test -p
    /// el_engine --features oracle -- --ignored oracle_smoke --nocapture`
    /// (needs Oracle Instant Client on this machine).
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
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (1, 'ada', 10.50, 1, 'note', \
             HEXTORAW('DEADBEEF'), DATE '1990-03-01', \
             TIMESTAMP '2026-01-01 10:00:00 +00:00')",
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
            .query_row_as::<String>("SELECT USER FROM DUAL", &[])
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
        while let Some(chunk) = extractor.next_chunk().unwrap() {
            assert!(chunk.height() <= 2);
            total += chunk.height();
        }
        assert_eq!(total, 3);

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
