//! Oracle source connector (worker-side, feature "oracle"). Two drivers
//! sit behind one [`Session`]: Oracle's pure-Rust thin driver (`oracledb`,
//! the default — it speaks the wire protocol itself, so no client software
//! is needed anywhere) and, behind the `oracle-thick` feature, the ODPI-C
//! binding that loads Oracle Instant Client at run time. The thin
//! protocol starts at Oracle Database 12.1; a 10g or 11g server is refused
//! at the handshake, and that refusal is what makes `driver: auto` fall
//! back to the thick driver. Credentials come from the environment
//! (`ZDBT_EL_SRC_ORACLE_*`, see `oracle_env`), never argv; a `TNS_ADMIN`
//! directory is handed to whichever driver is used. Setup, the test
//! container and the live tests: `el_engine/ORACLE.md`.
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
use oracledb::{OracleNumber, OracleTimestamp, ToDbValue};
use polars::prelude::*;

use super::oracle_env::{ENV_TNS_ADMIN, driver_from_env};
use crate::oracle_types::{OracleColumnType, normalize_ident, quote_ident, quote_stored};
use crate::spec::OracleDriver;
use crate::state::WatermarkValue;

/// Rows fetched per server round trip. Both drivers buffer this many rows
/// per fetch (ODPI-C allocates every column's buffer at this depth before
/// the first row), so the chunk size — 50,000 by default — must not drive
/// it; the chunk is still assembled lazily, one fetch at a time.
const FETCH_ARRAY_ROWS: u32 = 1_000;

/// Whole-handshake budget. The drivers' own timeouts cover the socket
/// only, and loading a client library plus a TNS lookup can be slow on a
/// cold machine — but a wedged listener must still become an error.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Widest `VARCHAR2` bind; wider text goes through the LOB path.
const MAX_VARCHAR2_BYTES: usize = 4000;

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

// -- the session: one interface over two drivers ----------------------------

/// An open connection through whichever driver reached the server.
pub struct Session {
    inner: Inner,
}

enum Inner {
    Thin(oracledb::Connection),
    #[cfg(feature = "oracle-thick")]
    Thick(oracle::Connection),
}

/// An open cursor.
pub enum Rows {
    Thin(oracledb::Cursor),
    #[cfg(feature = "oracle-thick")]
    Thick(oracle::ResultSet<'static, oracle::Row>),
}

/// One fetched row, read through typed getters.
pub enum Row {
    Thin(oracledb::Row),
    #[cfg(feature = "oracle-thick")]
    Thick(oracle::Row),
}

/// A statement bind, typed so a text watermark never reaches a DATE
/// comparison as an implicitly converted string.
#[derive(Clone, Debug, PartialEq)]
pub enum Bind {
    Int(i64),
    Float(f64),
    Text(String),
    Timestamp(NaiveDateTime),
    Date(NaiveDate),
}

/// One cell of a batch insert. `Null` takes its type from the column's
/// [`CellKind`], so a whole-NULL column still lands in the right shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Float(f64),
    /// Exact decimal text (`-12.50`), parsed by Oracle without rounding.
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Date(NaiveDate),
    /// A wall clock into DATE / TIMESTAMP.
    Timestamp(NaiveDateTime),
    /// An instant, as its UTC clock, into TIMESTAMP WITH TIME ZONE.
    TimestampUtc(NaiveDateTime),
}

/// The type of a batch column, for NULLs and for the thick driver's
/// per-column bind type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Int,
    Float,
    Decimal { precision: u8, scale: i8 },
    Text,
    Bytes,
    Date,
    Timestamp,
    TimestampTz,
}

fn thin_error(error: oracledb::Error) -> anyhow::Error {
    if is_version_refusal(&error) {
        anyhow!(
            "this Oracle server is older than 12.1 (10g or 11g): the thin driver speaks \
             only the protocol of Oracle Database 12.1 and later"
        )
    } else {
        anyhow!("{error}")
    }
}

fn is_version_refusal(error: &oracledb::Error) -> bool {
    matches!(error.kind(), oracledb::ErrorKind::ServerVersionNotSupported)
}

impl Session {
    /// Which driver this session runs on: `"thin"` or `"thick"`.
    pub fn driver(&self) -> &'static str {
        match &self.inner {
            Inner::Thin(_) => "thin",
            #[cfg(feature = "oracle-thick")]
            Inner::Thick(_) => "thick",
        }
    }

    /// Runs a statement (DDL, DML, PL/SQL) and returns its row count.
    pub fn execute(&self, sql: &str) -> Result<u64> {
        match &self.inner {
            Inner::Thin(conn) => conn
                .execute(sql, &[])
                .map(|result| result.rows_affected())
                .map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Inner::Thick(conn) => conn
                .execute(sql, &[])
                .map_err(thick::error)
                .map(|statement| statement.row_count().unwrap_or(0)),
        }
    }

    pub fn commit(&self) -> Result<()> {
        match &self.inner {
            Inner::Thin(conn) => conn.commit().map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Inner::Thick(conn) => conn.commit().map_err(thick::error),
        }
    }

    /// Opens a cursor, fetching `fetch_rows` rows per round trip.
    pub fn query(&self, sql: &str, binds: &[Bind], fetch_rows: u32) -> Result<Rows> {
        let fetch_rows = fetch_rows.max(1);
        match &self.inner {
            Inner::Thin(conn) => {
                let values: Vec<Box<dyn ToDbValue>> = binds.iter().map(thin_bind).collect();
                let params: Vec<&dyn ToDbValue> = values.iter().map(|v| v.as_ref()).collect();
                let mut statement = conn.statement(sql).map_err(thin_error)?;
                statement.fetch_array_size(fetch_rows).prefetch_rows(fetch_rows);
                statement.query(&params).map(Rows::Thin).map_err(thin_error)
            }
            #[cfg(feature = "oracle-thick")]
            Inner::Thick(conn) => thick::query(conn, sql, binds, fetch_rows),
        }
    }

    /// The first cell of the first row, as text; `None` for no rows or NULL.
    pub fn query_scalar_text(&self, sql: &str) -> Result<Option<String>> {
        let mut rows = self.query(sql, &[], 1)?;
        match rows.next_row()? {
            Some(row) => row.get_text(0),
            None => Ok(None),
        }
    }

    /// One array-bound INSERT for a whole chunk: `sql` carries positional
    /// binds `:1 … :n`, one per `kinds` entry, and every row has as many
    /// cells.
    pub fn insert_batch(&self, sql: &str, kinds: &[CellKind], rows: &[Vec<Cell>]) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        match &self.inner {
            Inner::Thin(conn) => {
                let boxed: Vec<Vec<Box<dyn ToDbValue>>> = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .zip(kinds)
                            .map(|(cell, kind)| thin_cell(cell, *kind))
                            .collect::<Result<Vec<_>>>()
                    })
                    .collect::<Result<_>>()?;
                let refs: Vec<Vec<&dyn ToDbValue>> = boxed
                    .iter()
                    .map(|row| row.iter().map(|v| v.as_ref()).collect())
                    .collect();
                let slices: Vec<&[&dyn ToDbValue]> = refs.iter().map(|r| r.as_slice()).collect();
                conn.execute_batch(sql, oracledb::BindParameters::Slice(&slices))
                    .map(|result| result.rows_affected())
                    .map_err(thin_error)
            }
            #[cfg(feature = "oracle-thick")]
            Inner::Thick(conn) => thick::insert_batch(conn, sql, kinds, rows),
        }
    }
}

impl Rows {
    /// The cursor's column names — an empty result still describes its
    /// shape.
    pub fn column_names(&self) -> Vec<String> {
        match self {
            Rows::Thin(cursor) => cursor.columns().iter().map(|c| c.name().to_owned()).collect(),
            #[cfg(feature = "oracle-thick")]
            Rows::Thick(rows) => rows.column_info().iter().map(|c| c.name().to_owned()).collect(),
        }
    }

    pub fn next_row(&mut self) -> Result<Option<Row>> {
        match self {
            Rows::Thin(cursor) => cursor
                .next()
                .transpose()
                .map(|row| row.map(Row::Thin))
                .map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Rows::Thick(rows) => rows
                .next()
                .transpose()
                .map(|row| row.map(Row::Thick))
                .map_err(thick::error),
        }
    }
}

impl Row {
    pub fn get_i64(&self, index: usize) -> Result<Option<i64>> {
        match self {
            Row::Thin(row) => row.get(index).map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => row.get(index).map_err(thick::error),
        }
    }

    pub fn get_f64(&self, index: usize) -> Result<Option<f64>> {
        match self {
            Row::Thin(row) => row.get(index).map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => row.get(index).map_err(thick::error),
        }
    }

    pub fn get_bool(&self, index: usize) -> Result<Option<bool>> {
        match self {
            Row::Thin(row) => row.get(index).map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => row.get(index).map_err(thick::error),
        }
    }

    pub fn get_bytes(&self, index: usize) -> Result<Option<Vec<u8>>> {
        match self {
            Row::Thin(row) => row.get(index).map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => row.get(index).map_err(thick::error),
        }
    }

    /// A NUMBER as exact decimal text.
    pub fn get_decimal_text(&self, index: usize) -> Result<Option<String>> {
        match self {
            Row::Thin(row) => row
                .get::<Option<OracleNumber>>(index)
                .map(|value| value.map(|value| value.to_string()))
                .map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => row.get(index).map_err(thick::error),
        }
    }

    /// A cell as text, whatever its type: the explorer's grid and the
    /// text fetch path both want Oracle's own rendering rather than a
    /// conversion error. Bytes render as hex.
    pub fn get_text(&self, index: usize) -> Result<Option<String>> {
        match self {
            Row::Thin(row) => thin_cell_text(row, index).map_err(thin_error),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => thick::cell_text(row, index),
        }
    }

    /// A DATE / TIMESTAMP [WITH TIME ZONE] cell as microseconds since the
    /// epoch. The session runs in UTC, so a naive clock is read as UTC
    /// and a zoned value is converted to it.
    pub fn get_timestamp_micros(&self, index: usize, zoned: bool) -> Result<Option<i64>> {
        match self {
            // The thin driver hands back a zoned value's UTC clock with
            // the stored offset alongside: the fields are the instant.
            Row::Thin(row) => row
                .get::<Option<OracleTimestamp>>(index)
                .map_err(thin_error)?
                .map(|value| micros_of(&value))
                .transpose(),
            #[cfg(feature = "oracle-thick")]
            Row::Thick(row) => thick::timestamp_micros(row, index, zoned),
        }
    }
}

fn thin_cell_text(row: &oracledb::Row, index: usize) -> Result<Option<String>, oracledb::Error> {
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
    row.get::<Option<Vec<u8>>>(index).map(|value| value.map(hex))
}

fn hex(bytes: Vec<u8>) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn thin_bind(bind: &Bind) -> Box<dyn ToDbValue> {
    match bind {
        Bind::Int(value) => Box::new(*value),
        Bind::Float(value) => Box::new(*value),
        Bind::Text(value) => Box::new(value.clone()),
        Bind::Timestamp(value) => Box::new(thin_timestamp(*value)),
        Bind::Date(value) => Box::new(OracleTimestamp::new_date(
            value.year() as i16,
            value.month() as u8,
            value.day() as u8,
        )),
    }
}

fn thin_cell(cell: &Cell, kind: CellKind) -> Result<Box<dyn ToDbValue>> {
    Ok(match cell {
        Cell::Null => match kind {
            CellKind::Int => Box::new(None::<i64>),
            CellKind::Float => Box::new(None::<f64>),
            CellKind::Decimal { .. } => Box::new(None::<OracleNumber>),
            CellKind::Text => Box::new(None::<String>),
            CellKind::Bytes => Box::new(None::<Vec<u8>>),
            CellKind::Date | CellKind::Timestamp | CellKind::TimestampTz => {
                Box::new(None::<OracleTimestamp>)
            }
        },
        Cell::Int(value) => Box::new(*value),
        Cell::Float(value) => Box::new(*value),
        Cell::Decimal(text) => Box::new(text.parse::<OracleNumber>().map_err(thin_error)?),
        Cell::Text(value) => Box::new(value.clone()),
        Cell::Bytes(value) => Box::new(value.clone()),
        Cell::Date(value) => Box::new(OracleTimestamp::new_date(
            value.year() as i16,
            value.month() as u8,
            value.day() as u8,
        )),
        Cell::Timestamp(value) => Box::new(thin_timestamp(*value)),
        Cell::TimestampUtc(value) => Box::new(OracleTimestamp::new_timestamp_tz(
            value.year() as i16,
            value.month() as u8,
            value.day() as u8,
            value.hour() as u8,
            value.minute() as u8,
            value.second() as u8,
            value.nanosecond(),
            0,
            0,
        )),
    })
}

fn thin_timestamp(value: NaiveDateTime) -> OracleTimestamp {
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

/// Microseconds since the epoch for the thin driver's timestamp value,
/// whose fields are read as UTC.
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

// -- connecting ------------------------------------------------------------

/// Connects with the credentials the parent put in the environment, on
/// the driver it named.
pub fn connect_from_env() -> Result<Session> {
    let creds = super::oracle_env::creds_from_env()?;
    connect_with(&creds.user, &creds.password, &creds.connect, driver_from_env())
}

/// Connects on the driver the environment names (`auto` when unset).
pub fn connect(user: &str, password: &str, connect_string: &str) -> Result<Session> {
    connect_with(user, password, connect_string, driver_from_env())
}

/// Connects on a chosen driver. `Auto` tries the thin driver and falls
/// back to the thick one only when the server is refused for its version
/// — a modern database never touches Instant Client. Errors never echo
/// the user or the connect string.
pub fn connect_with(
    user: &str,
    password: &str,
    connect_string: &str,
    driver: OracleDriver,
) -> Result<Session> {
    let tns_admin = std::env::var_os(ENV_TNS_ADMIN)
        .filter(|dir| !dir.is_empty())
        .map(|dir| dir.to_string_lossy().into_owned());
    let inner = match driver {
        OracleDriver::Thin => thin_connect(user, password, connect_string, tns_admin.as_deref())
            .map_err(|error| match error {
                ConnectError::OldServer => anyhow!(
                    "this Oracle server is older than 12.1 (10g or 11g), which the thin driver \
                     cannot speak to — set `driver: thick` (or `auto`) on the connection and run \
                     it on a Linux worker with Oracle Instant Client"
                ),
                ConnectError::Other(error) => error,
            })?,
        OracleDriver::Thick => thick_connect(user, password, connect_string)?,
        OracleDriver::Auto => {
            match thin_connect(user, password, connect_string, tns_admin.as_deref()) {
                Ok(inner) => inner,
                Err(ConnectError::OldServer) => thick_connect(user, password, connect_string)
                    .context("the server is older than 12.1, so the thick driver was tried")?,
                Err(ConnectError::Other(error)) => return Err(error),
            }
        }
    };
    let session = Session { inner };
    // Deterministic session: timestamps with a zone come back as UTC and
    // numbers print with a dot, whatever the server's NLS defaults are.
    for statement in [
        "ALTER SESSION SET TIME_ZONE = 'UTC'",
        "ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,'",
    ] {
        session
            .execute(statement)
            .with_context(|| format!("running {statement}"))?;
    }
    Ok(session)
}

enum ConnectError {
    /// The thin driver refused the server for its version.
    OldServer,
    Other(anyhow::Error),
}

/// Runs a connect attempt on its own thread and abandons it at the
/// deadline: the drivers' own timeouts cover the socket only.
fn bounded<T: Send + 'static, E: Send + 'static>(
    attempt: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<Result<T, E>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(attempt());
    });
    match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(result) => Ok(result),
        Err(_) => {
            drop(handle); // the thread dies with the process
            bail!(
                "connecting to oracle timed out after {}s — is the listener reachable?",
                CONNECT_TIMEOUT.as_secs()
            )
        }
    }
}

fn thin_connect(
    user: &str,
    password: &str,
    connect_string: &str,
    tns_admin: Option<&str>,
) -> Result<Inner, ConnectError> {
    let mut config = oracledb::Config::default().set_credentials(user, password);
    // A wallet / tnsnames directory: the driver resolves an alias through
    // its tnsnames.ora and reads a wallet's ewallet.pem from there.
    if let Some(dir) = tns_admin {
        config = config.set_config_dir(dir).set_wallet_location(dir);
    }
    let config = config
        .set_connect_string(connect_string)
        .map_err(thin_error)
        .context("parsing the oracle connect string")
        .map_err(ConnectError::Other)?;
    match bounded(move || oracledb::connect(config)).map_err(ConnectError::Other)? {
        Ok(conn) => Ok(Inner::Thin(conn)),
        Err(error) if is_version_refusal(&error) => Err(ConnectError::OldServer),
        Err(error) => Err(ConnectError::Other(
            thin_error(error).context("connecting to oracle (thin driver)"),
        )),
    }
}

#[cfg(feature = "oracle-thick")]
fn thick_connect(user: &str, password: &str, connect_string: &str) -> Result<Inner> {
    thick::connect(user, password, connect_string).map(Inner::Thick)
}

#[cfg(not(feature = "oracle-thick"))]
fn thick_connect(_user: &str, _password: &str, _connect_string: &str) -> Result<Inner> {
    bail!(
        "this server needs the thick Oracle driver (Instant Client), which this worker \
         build does not include — deploy the pipeline to a Linux remote whose worker was \
         built with it"
    )
}

/// The ODPI-C driver: reaches Oracle 10g and 11g through an Instant
/// Client of the matching generation (12.1 for 10g, 19c for 11g).
#[cfg(feature = "oracle-thick")]
mod thick {
    use std::sync::OnceLock;

    use anyhow::{Context as _, Result, anyhow, bail};
    use chrono::{DateTime, FixedOffset, NaiveDateTime};
    use oracle::sql_type::{OracleType, ToSql};
    use oracle::{Connection, Row};

    use super::super::oracle_env::{ENV_CLIENT_DIR, ENV_TRY_MACOS_CLIENT, client_dir_from_env};
    use super::{Bind, Cell, CellKind, MAX_VARCHAR2_BYTES, Rows, hex};

    /// What every thick path says when ODPI-C cannot find a client library.
    pub const INSTANT_CLIENT_HELP: &str = concat!(
        "Oracle Instant Client is not installed on the machine running the connector worker, ",
        "or its directory is not on the loader path (ODPI-C reported DPI-1047). It is only ",
        "needed for Oracle 10g/11g servers: install the Basic Light package of the generation ",
        "that reaches your server (19c for 11.2, 12.1 for 10g), name its directory in ",
        "ZDBT_EL_ORACLE_CLIENT_DIR and, on Linux, also in LD_LIBRARY_PATH or ld.so.conf ",
        "(the client's own libraries load through the system loader) — install.sh ",
        "--oracle-client-url does both (crates/el_engine/ORACLE.md)."
    );

    /// Why the thick driver will not connect from an Apple Silicon Mac.
    pub const MACOS_CLIENT_HELP: &str = concat!(
        "the thick Oracle driver cannot run on this Mac: Oracle Instant Client 23.3, the only ",
        "build for Apple Silicon, crashes inside its own crypto library at connect time ",
        "(Oracle bug 36790189), and no older client exists for it. Deploy the pipeline to a ",
        "Linux remote, or set ZDBT_EL_ORACLE_TRY_MACOS_CLIENT=1 to try anyway."
    );

    pub fn error(error: oracle::Error) -> anyhow::Error {
        if error.dpi_code() == Some(1047) || error.to_string().contains("DPI-1047") {
            anyhow!(INSTANT_CLIENT_HELP)
        } else {
            anyhow!("{error}")
        }
    }

    /// Points ODPI-C at the client directory the environment names, once
    /// per process. Without one the driver's own search applies.
    fn init_client() -> Result<()> {
        static OUTCOME: OnceLock<Result<(), String>> = OnceLock::new();
        OUTCOME
            .get_or_init(|| {
                let Some(dir) = client_dir_from_env() else {
                    return Ok(());
                };
                let mut params = oracle::InitParams::new();
                params
                    .oracle_client_lib_dir(dir.as_str())
                    .and_then(|params| params.init())
                    .map(|_| ())
                    .map_err(|error| format!("loading Oracle Instant Client from {dir}: {error}"))
            })
            .clone()
            .map_err(|message| anyhow!("{message} ({ENV_CLIENT_DIR})"))
    }

    pub fn connect(user: &str, password: &str, connect_string: &str) -> Result<Connection> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64"))
            && std::env::var_os(ENV_TRY_MACOS_CLIENT).is_none()
        {
            bail!(MACOS_CLIENT_HELP);
        }
        init_client()?;
        let (user, password, connect_string) = (
            user.to_owned(),
            password.to_owned(),
            connect_string.to_owned(),
        );
        // ODPI-C reads TNS_ADMIN from the process environment itself.
        super::bounded(move || Connection::connect(&user, &password, &connect_string))?
            .map_err(error)
            .context("connecting to oracle (thick driver)")
    }

    pub fn query(conn: &Connection, sql: &str, binds: &[Bind], fetch_rows: u32) -> Result<Rows> {
        let values: Vec<Box<dyn ToSql>> = binds.iter().map(bind).collect();
        let params: Vec<&dyn ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let statement = conn
            .statement(sql)
            .fetch_array_size(fetch_rows)
            .build()
            .map_err(error)?;
        statement
            .into_result_set::<Row>(&params)
            .map(Rows::Thick)
            .map_err(error)
    }

    pub fn cell_text(row: &Row, index: usize) -> Result<Option<String>> {
        match row.get::<usize, Option<String>>(index) {
            Ok(value) => Ok(value),
            Err(_) => row
                .get::<usize, Option<Vec<u8>>>(index)
                .map(|value| value.map(hex))
                .map_err(error),
        }
    }

    /// ODPI-C's `DateTime<Utc>` conversion copies the wall clock and drops
    /// the zone, so a zoned value is fetched with its offset and converted.
    pub fn timestamp_micros(row: &Row, index: usize, zoned: bool) -> Result<Option<i64>> {
        if zoned {
            row.get::<usize, Option<DateTime<FixedOffset>>>(index)
                .map(|value| value.map(|value| value.timestamp_micros()))
                .map_err(error)
        } else {
            row.get::<usize, Option<NaiveDateTime>>(index)
                .map(|value| value.map(|value| value.and_utc().timestamp_micros()))
                .map_err(error)
        }
    }

    fn bind(bind: &Bind) -> Box<dyn ToSql> {
        match bind {
            Bind::Int(value) => Box::new(*value),
            Bind::Float(value) => Box::new(*value),
            Bind::Text(value) => Box::new(value.clone()),
            Bind::Timestamp(value) => Box::new(*value),
            Bind::Date(value) => Box::new(*value),
        }
    }

    /// One array-bound INSERT: one bind type per column, set before the
    /// first row so an all-NULL column cannot fix the wrong type.
    pub fn insert_batch(
        conn: &Connection,
        sql: &str,
        kinds: &[CellKind],
        rows: &[Vec<Cell>],
    ) -> Result<u64> {
        let mut batch = conn.batch(sql, rows.len()).build().map_err(error)?;
        for (index, kind) in kinds.iter().enumerate() {
            batch
                .set_type(index + 1, &bind_type(*kind, index, rows))
                .map_err(error)
                .with_context(|| format!("binding column {}", index + 1))?;
        }
        for row in rows {
            let values: Vec<Box<dyn ToSql>> = row.iter().map(cell).collect();
            let refs: Vec<&dyn ToSql> = values.iter().map(|v| v.as_ref()).collect();
            batch.append_row(&refs).map_err(error)?;
        }
        batch.execute().map_err(error)?;
        Ok(rows.len() as u64)
    }

    fn bind_type(kind: CellKind, index: usize, rows: &[Vec<Cell>]) -> OracleType {
        match kind {
            CellKind::Int => OracleType::Int64,
            CellKind::Float => OracleType::BinaryDouble,
            CellKind::Decimal { precision, scale } => OracleType::Number(precision, scale),
            CellKind::Date => OracleType::Date,
            CellKind::Timestamp => OracleType::Timestamp(6),
            CellKind::TimestampTz => OracleType::TimestampTZ(6),
            CellKind::Bytes => OracleType::BLOB,
            // A VARCHAR2 bind is capped at 4000 bytes; wider text is a LOB
            // (and our DDL made that column CLOB).
            CellKind::Text => {
                let (mut widest_bytes, mut widest_chars) = (0usize, 0usize);
                for row in rows {
                    if let Some(Cell::Text(value)) = row.get(index) {
                        widest_bytes = widest_bytes.max(value.len());
                        widest_chars = widest_chars.max(value.chars().count());
                    }
                }
                if widest_bytes > MAX_VARCHAR2_BYTES {
                    OracleType::CLOB
                } else {
                    OracleType::Varchar2(widest_chars.max(1) as u32)
                }
            }
        }
    }

    fn cell(cell: &Cell) -> Box<dyn ToSql> {
        match cell {
            // The column's bind type was set already; a typed None is all
            // ODPI-C needs here.
            Cell::Null => Box::new(None::<String>),
            Cell::Int(value) => Box::new(*value),
            Cell::Float(value) => Box::new(*value),
            Cell::Decimal(text) | Cell::Text(text) => Box::new(text.clone()),
            Cell::Bytes(value) => Box::new(value.clone()),
            Cell::Date(value) => Box::new(*value),
            Cell::Timestamp(value) => Box::new(*value),
            Cell::TimestampUtc(value) => Box::new(value.and_utc()),
        }
    }
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
    rows: Option<Rows>,
    chunk_rows: usize,
    /// Declared after `rows` so the cursor is dropped before the session.
    _session: Session,
}

impl OracleExtractor {
    pub fn new(
        user: &str,
        password: &str,
        connect_string: &str,
        driver: OracleDriver,
        schema: &str,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, WatermarkValue)>,
    ) -> Result<Self> {
        let session = connect_with(user, password, connect_string, driver)?;
        Self::with_session(session, schema, table, chunk_rows, cursor)
    }

    /// The same construction on an already-open session — the smoke test's
    /// entry point, and where the explorer would reuse a connection.
    pub fn with_session(
        session: Session,
        schema: &str,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, WatermarkValue)>,
    ) -> Result<Self> {
        let owner = normalize_ident(schema)?;
        let table_name = normalize_ident(table)?;
        let chunk_rows = chunk_rows.max(1);

        let columns = probe_columns(&session, &owner, &table_name)?;
        let sql = select_sql(
            &owner,
            &table_name,
            &columns,
            cursor.as_ref().map(|(column, _)| column.as_str()),
        )?;
        let binds: Vec<Bind> = match &cursor {
            Some((column, value)) => vec![
                watermark_bind(value)
                    .with_context(|| format!("binding the cursor on {column:?}"))?,
            ],
            None => Vec::new(),
        };
        let rows = session
            .query(&sql, &binds, fetch_array_size(chunk_rows))
            .with_context(|| format!("reading {owner}.{table_name}"))?;

        Ok(Self {
            columns,
            rows: Some(rows),
            chunk_rows,
            _session: session,
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
fn probe_columns(session: &Session, owner: &str, table: &str) -> Result<Vec<(String, Fetch)>> {
    let mut rows = session
        .query(
            COLUMNS_SQL,
            &[Bind::Text(owner.to_owned()), Bind::Text(table.to_owned())],
            FETCH_ARRAY_ROWS,
        )
        .with_context(|| format!("describing {owner}.{table}"))?;
    let mut columns = Vec::new();
    while let Some(row) = rows
        .next_row()
        .with_context(|| format!("describing {owner}.{table}"))?
    {
        let name = row.get_text(0)?.unwrap_or_default();
        let data_type = row.get_text(1)?.unwrap_or_default();
        let precision = row
            .get_i64(2)
            .unwrap_or(None)
            .and_then(|value| u8::try_from(value).ok());
        let scale = row
            .get_i64(3)
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
fn watermark_bind(value: &WatermarkValue) -> Result<Bind> {
    Ok(match value {
        WatermarkValue::Int(value) => Bind::Int(*value),
        WatermarkValue::Float(value) => Bind::Float(*value),
        WatermarkValue::Text(value) => Bind::Text(value.clone()),
        WatermarkValue::Timestamp(micros) => Bind::Timestamp(
            naive_from_micros(*micros)
                .ok_or_else(|| anyhow!("cursor timestamp {micros} is out of range"))?,
        ),
        WatermarkValue::Date(days) => Bind::Date(
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
        let context = || format!("column {name}");
        match buffer {
            Values::Bool(values) => values.push(row.get_bool(index).with_context(context)?),
            Values::Int(values) => values.push(row.get_i64(index).with_context(context)?),
            Values::Float(values) => values.push(row.get_f64(index).with_context(context)?),
            // NUMBER arrives as exact text and stays exact all the way
            // into the decimal column.
            Values::Decimal(values) => {
                values.push(row.get_decimal_text(index).with_context(context)?)
            }
            Values::Text(values) => values.push(row.get_text(index).with_context(context)?),
            Values::Timestamp(values) => {
                values.push(row.get_timestamp_micros(index, false).with_context(context)?)
            }
            Values::TimestampTz(values) => {
                values.push(row.get_timestamp_micros(index, true).with_context(context)?)
            }
            Values::Binary(values) => values.push(row.get_bytes(index).with_context(context)?),
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
            match rows.next_row().context("reading an oracle row")? {
                None => {
                    // Exhausted: close the cursor, keep the session for
                    // Drop.
                    self.rows = None;
                    break;
                }
                Some(row) => {
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
        );
        conn.execute(
            "CREATE TABLE ZDBT_EL_SMOKE ( \
               ID NUMBER(10,0), NAME VARCHAR2(50), AMOUNT NUMBER(18,2), \
               ACTIVE NUMBER(1), NOTE CLOB, PAYLOAD RAW(16), \
               BORN DATE, UPDATED TIMESTAMP(6) WITH TIME ZONE)",
        )
        .unwrap();
        // Row 1 is stamped +02:00: it must read as 08:00 UTC, not 10:00.
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (1, 'ada', 10.50, 1, 'note', \
             HEXTORAW('DEADBEEF'), DATE '1990-03-01', \
             TIMESTAMP '2026-01-01 10:00:00 +02:00')",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (2, 'grace', NULL, 0, NULL, NULL, NULL, \
             TIMESTAMP '2026-01-02 11:30:00 +00:00')",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZDBT_EL_SMOKE VALUES (3, 'alan', -2.25, 1, 'x', NULL, \
             DATE '1985-12-31', TIMESTAMP '2026-01-03 12:00:00 +00:00')",
        )
        .unwrap();
        conn.commit().unwrap();
        let schema_owner: String = conn
            .query_scalar_text("SELECT USER FROM DUAL")
            .unwrap()
            .unwrap();

        let mut extractor = OracleExtractor::new(
            &user,
            &password,
            &connect_string,
            OracleDriver::Auto,
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
            OracleDriver::Auto,
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
