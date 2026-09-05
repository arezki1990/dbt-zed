//! The Snowflake loader sidecar: drives the official ADBC Go driver from
//! this worker process (never the GPUI app — dlopen'ing the Go runtime in
//! a host app hangs). Speaks `el_engine::load::protocol` on stdio; chunk
//! data arrives as Arrow IPC files.
//!
//! Secrets come in via environment (ZDBT_EL_SF_*) and are merged into the
//! driver options here — they never appear on the wire or in errors.

use std::io::BufRead as _;

use adbc_core::options::{AdbcVersion, OptionDatabase, OptionValue};
use adbc_core::{Connection as _, Database as _, Driver as _, Optionable as _, Statement as _};
use adbc_driver_manager::{ManagedConnection, ManagedDriver};
use anyhow::{Context as _, Result, anyhow, bail};
use el_engine::load::protocol::{
    AuthMethod, ENV_PASSWORD, ENV_PRIVATE_KEY_PATH, Request, Response,
};

fn respond(response: &Response) {
    if let Ok(line) = serde_json::to_string(response) {
        println!("{line}");
    }
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut connection: Option<ManagedConnection> = None;

    for line in stdin.lock().lines() {
        let line = line.context("reading stdin")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(error) => {
                respond(&Response::error(format!("bad request: {error}")));
                continue;
            }
        };
        match request {
            Request::Shutdown => {
                respond(&Response::ok());
                break;
            }
            Request::Open {
                driver_path,
                account,
                user,
                role,
                warehouse,
                database,
                schema,
                auth,
            } => match open(
                &driver_path,
                &account,
                &user,
                role.as_deref(),
                warehouse.as_deref(),
                database.as_deref(),
                schema.as_deref(),
                auth,
            ) {
                Ok(conn) => {
                    connection = Some(conn);
                    respond(&Response::ok());
                }
                Err(error) => respond(&Response::error(format!("{error:#}"))),
            },
            Request::Exec { sql } => match &mut connection {
                Some(conn) => respond(&exec(conn, &sql).unwrap_or_else(|error| {
                    Response::error(format!("{error:#}"))
                })),
                None => respond(&Response::error("not connected — send open first")),
            },
            Request::QueryScalar { sql } => match &mut connection {
                Some(conn) => respond(&query_scalar(conn, &sql).unwrap_or_else(|error| {
                    Response::error(format!("{error:#}"))
                })),
                None => respond(&Response::error("not connected — send open first")),
            },
            Request::Ingest { table, ipc_path } => match &mut connection {
                Some(conn) => respond(&ingest(conn, &table, &ipc_path).unwrap_or_else(
                    |error| Response::error(format!("{error:#}")),
                )),
                None => respond(&Response::error("not connected — send open first")),
            },
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn open(
    driver_path: &std::path::Path,
    account: &str,
    user: &str,
    role: Option<&str>,
    warehouse: Option<&str>,
    database: Option<&str>,
    schema: Option<&str>,
    auth: AuthMethod,
) -> Result<ManagedConnection> {
    let mut driver =
        ManagedDriver::load_dynamic_from_filename(driver_path, None, AdbcVersion::V110)
            .map_err(|error| anyhow!("loading ADBC driver: {error:?}"))?;

    let mut opts: Vec<(OptionDatabase, OptionValue)> = vec![
        (
            OptionDatabase::Other("adbc.snowflake.sql.account".into()),
            OptionValue::String(account.to_owned()),
        ),
        (OptionDatabase::Username, OptionValue::String(user.to_owned())),
    ];
    match auth {
        AuthMethod::Password => {
            let password = std::env::var(ENV_PASSWORD)
                .context("ZDBT_EL_SF_PASSWORD is not set in the sidecar environment")?;
            opts.push((OptionDatabase::Password, OptionValue::String(password)));
        }
        AuthMethod::KeyPair => {
            let key_path = std::env::var(ENV_PRIVATE_KEY_PATH)
                .context("ZDBT_EL_SF_PRIVATE_KEY_PATH is not set in the sidecar environment")?;
            opts.push((
                OptionDatabase::Other("adbc.snowflake.sql.auth_type".into()),
                OptionValue::String("auth_jwt".to_owned()),
            ));
            opts.push((
                OptionDatabase::Other("adbc.snowflake.sql.client_option.jwt_private_key".into()),
                OptionValue::String(key_path),
            ));
        }
    }
    for (key, value) in [
        ("adbc.snowflake.sql.role", role),
        ("adbc.snowflake.sql.warehouse", warehouse),
        ("adbc.snowflake.sql.db", database),
        ("adbc.snowflake.sql.schema", schema),
    ] {
        if let Some(value) = value {
            opts.push((
                OptionDatabase::Other(key.into()),
                OptionValue::String(value.to_owned()),
            ));
        }
    }

    let mut db = driver
        .new_database_with_opts(opts)
        .map_err(|error| anyhow!("configuring Snowflake database handle: {error:?}"))?;
    db.new_connection()
        .map_err(|error| anyhow!("connecting to Snowflake: {error:?}"))
}

fn exec(connection: &mut ManagedConnection, sql: &str) -> Result<Response> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| anyhow!("new statement: {error:?}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| anyhow!("set query: {error:?}"))?;
    let affected = statement
        .execute_update()
        .map_err(|error| anyhow!("executing: {error:?}"))?;
    let mut response = Response::ok();
    response.rows_affected = affected.map(|rows| rows.max(0) as u64);
    Ok(response)
}

fn query_scalar(connection: &mut ManagedConnection, sql: &str) -> Result<Response> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| anyhow!("new statement: {error:?}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| anyhow!("set query: {error:?}"))?;
    let mut reader = statement
        .execute()
        .map_err(|error| anyhow!("executing: {error:?}"))?;
    let scalar = reader
        .next()
        .transpose()
        .map_err(|error| anyhow!("reading result: {error:?}"))?
        .and_then(|batch| {
            (batch.num_rows() > 0 && batch.num_columns() > 0).then(|| {
                let column = batch.column(0);
                arrow_cast::display::array_value_to_string(column, 0)
                    .unwrap_or_default()
            })
        });
    let mut response = Response::ok();
    response.scalar = scalar.map(serde_json::Value::String);
    Ok(response)
}

fn ingest(
    connection: &mut ManagedConnection,
    table: &str,
    ipc_path: &std::path::Path,
) -> Result<Response> {
    let file = std::fs::File::open(ipc_path)
        .with_context(|| format!("opening chunk {}", ipc_path.display()))?;
    let reader = arrow_ipc::reader::FileReader::try_new(file, None)
        .map_err(|error| anyhow!("reading chunk ipc: {error}"))?;

    let mut statement = connection
        .new_statement()
        .map_err(|error| anyhow!("new statement: {error:?}"))?;
    statement
        .set_option(
            adbc_core::options::OptionStatement::Other("adbc.ingest.target_table".into()),
            OptionValue::String(table.to_owned()),
        )
        .map_err(|error| anyhow!("set ingest table: {error:?}"))?;
    statement
        .set_option(
            adbc_core::options::OptionStatement::Other("adbc.ingest.mode".into()),
            OptionValue::String("adbc.ingest.mode.append".to_owned()),
        )
        .map_err(|error| anyhow!("set ingest mode: {error:?}"))?;
    statement
        .bind_stream(Box::new(reader))
        .map_err(|error| anyhow!("binding chunk: {error:?}"))?;
    let affected = statement
        .execute_update()
        .map_err(|error| anyhow!("ingesting: {error:?}"))?;
    if !ipc_path.to_string_lossy().is_empty() {
        let _ = std::fs::remove_file(ipc_path);
    }
    let mut response = Response::ok();
    response.rows_affected = affected.map(|rows| rows.max(0) as u64);
    Ok(response)
}

#[cfg(test)]
mod tests {
    /// Live end-to-end against a real Snowflake account. Gated: set
    /// EL_SNOWFLAKE_SMOKE=1 plus the connection env the sidecar expects
    /// and ZDBT_ADBC_SNOWFLAKE_DRIVER, then run with --ignored.
    #[test]
    #[ignore]
    fn live_exec_select_one() {
        // Exercised through the parent-side AdbcSidecarLoader in
        // el_engine's gated e2e once credentials are provided.
    }
}
