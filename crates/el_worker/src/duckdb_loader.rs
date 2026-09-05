//! The DuckDB warehouse loader sidecar: the fully-local target for EL
//! testing. Same JSON-lines protocol as the Snowflake loader; ingest is
//! plain SQL (`read_parquet`) sent as Exec by the parent, so this loop
//! only needs Open/Exec/QueryScalar/Shutdown.

use std::io::BufRead as _;

use anyhow::{Context as _, Result, anyhow};
use duckdb::Connection;
use el_engine::load::protocol::{Request, Response};

fn respond(response: &Response) {
    if let Ok(line) = serde_json::to_string(response) {
        println!("{line}");
    }
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut connection: Option<Connection> = None;

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
            Request::OpenDuckdb { path } => match Connection::open(&path) {
                Ok(conn) => {
                    connection = Some(conn);
                    respond(&Response::ok());
                }
                Err(error) => respond(&Response::error(format!(
                    "opening duckdb warehouse {}: {error}",
                    path.display()
                ))),
            },
            Request::Exec { sql } => match &connection {
                Some(conn) => match conn.execute_batch(&sql) {
                    Ok(()) => {
                        // execute_batch reports no row count; INSERTs get it
                        // via a follow-up changes() call.
                        let mut response = Response::ok();
                        response.rows_affected = last_changes(conn);
                        respond(&response);
                    }
                    Err(error) => respond(&Response::error(format!("{error}"))),
                },
                None => respond(&Response::error("not connected — send open first")),
            },
            Request::QueryScalar { sql } => match &connection {
                Some(conn) => respond(&query_scalar(conn, &sql).unwrap_or_else(|error| {
                    Response::error(format!("{error:#}"))
                })),
                None => respond(&Response::error("not connected — send open first")),
            },
            Request::Open { .. } | Request::Ingest { .. } => {
                respond(&Response::error(
                    "this is the duckdb loader — snowflake requests go to snowflake-loader",
                ));
            }
        }
    }
    Ok(())
}

fn last_changes(connection: &Connection) -> Option<u64> {
    connection
        .query_row("SELECT changes()", [], |row| row.get::<_, i64>(0))
        .ok()
        .map(|changes| changes.max(0) as u64)
}

fn query_scalar(connection: &Connection, sql: &str) -> Result<Response> {
    let value: Option<String> = connection
        .query_row(sql, [], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| anyhow!("{error}"))?;
    let mut response = Response::ok();
    response.scalar = value.map(serde_json::Value::String);
    Ok(response)
}
