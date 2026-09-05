//! Ad-hoc exploration: table listings and capped queries for the EL
//! explorer and query surfaces. Emits `el_engine::explore::ExploreEvent`
//! JSON lines on stdout.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use el_engine::explore::ExploreEvent;

fn emit(event: &ExploreEvent) {
    if let Ok(line) = serde_json::to_string(event) {
        println!("{line}");
    }
}

/// Connect with a hard 10s timeout — an unreachable database must
/// become an error, never an indefinite hang.
fn pg_connect(url: &str) -> Result<postgres::Client> {
    use std::str::FromStr as _;
    let mut config = postgres::Config::from_str(url).context("parsing postgres url")?;
    config.connect_timeout(std::time::Duration::from_secs(10));
    config
        .connect(postgres::NoTls)
        .context("connecting to postgres (10s timeout)")
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|ix| args.get(ix + 1).cloned())
}

const LIST_SQL: &str = "SELECT table_schema, table_name FROM information_schema.tables \
     WHERE table_schema NOT IN ('information_schema', 'pg_catalog') \
     ORDER BY 1, 2";

pub fn list(args: &[String]) -> Result<()> {
    let kind = flag(args, "--kind").context("--kind required")?;
    let mut items: Vec<(String, String)> = Vec::new();
    match kind.as_str() {
        "duckdb" => {
            let db = PathBuf::from(flag(args, "--db").context("--db required")?);
            let config = duckdb::Config::default()
                .access_mode(duckdb::AccessMode::ReadOnly)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let connection = duckdb::Connection::open_with_flags(&db, config)
                .with_context(|| format!("opening {}", db.display()))?;
            let mut statement = connection.prepare(LIST_SQL)?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                items.push(row?);
            }
        }
        "postgres" => {
            let url = std::env::var("ZDBT_EL_SRC_URL")
                .context("ZDBT_EL_SRC_URL is not set")?;
            let mut client = pg_connect(&url)?;
            for row in client.query(LIST_SQL, &[])? {
                items.push((row.get(0), row.get(1)));
            }
        }
        other => bail!("unsupported kind {other:?}"),
    }
    emit(&ExploreEvent::Tables { items });
    emit(&ExploreEvent::Done);
    Ok(())
}

pub fn query(args: &[String]) -> Result<()> {
    let kind = flag(args, "--kind").context("--kind required")?;
    let sql_file = PathBuf::from(flag(args, "--sql-file").context("--sql-file required")?);
    let sql = std::fs::read_to_string(&sql_file).context("reading sql file")?;
    let limit: usize = flag(args, "--limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(500)
        .clamp(1, 10_000);
    // Cap by wrapping — the inner SQL is the user's own query against
    // their own connection.
    let capped = format!("SELECT * FROM ({sql}) AS zdbt_q LIMIT {limit}");

    match kind.as_str() {
        "duckdb" => {
            let db = PathBuf::from(flag(args, "--db").context("--db required")?);
            let config = duckdb::Config::default()
                .access_mode(duckdb::AccessMode::ReadOnly)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let connection = duckdb::Connection::open_with_flags(&db, config)
                .with_context(|| format!("opening {}", db.display()))?;
            let mut statement = connection.prepare(&capped)?;
            let column_count;
            {
                let mut rows = statement.query([])?;
                let mut names: Option<Vec<String>> = None;
                while let Some(row) = rows.next()? {
                    if names.is_none() {
                        let stmt = row.as_ref();
                        let collected: Vec<String> =
                            stmt.column_names().into_iter().map(Into::into).collect();
                        emit(&ExploreEvent::Columns {
                            names: collected.clone(),
                        });
                        names = Some(collected);
                    }
                    let width = names.as_ref().map(Vec::len).unwrap_or(0);
                    let mut cells = Vec::with_capacity(width);
                    for ix in 0..width {
                        let value: Option<String> = row
                            .get::<_, Option<String>>(ix)
                            .or_else(|_| {
                                row.get::<_, Option<f64>>(ix)
                                    .map(|v| v.map(|f| f.to_string()))
                            })
                            .or_else(|_| {
                                row.get::<_, Option<i64>>(ix)
                                    .map(|v| v.map(|i| i.to_string()))
                            })
                            .unwrap_or(None);
                        cells.push(value);
                    }
                    emit(&ExploreEvent::Row { cells });
                }
                column_count = names.is_some();
            }
            if !column_count {
                // No rows: still describe the shape.
                let names: Vec<String> =
                    statement.column_names().into_iter().map(Into::into).collect();
                emit(&ExploreEvent::Columns { names });
            }
        }
        "postgres" => {
            let url = std::env::var("ZDBT_EL_SRC_URL")
                .context("ZDBT_EL_SRC_URL is not set")?;
            let mut client = pg_connect(&url)?;
            // Probe for names + text-cast every column for display.
            let probe = client
                .prepare(&format!("SELECT * FROM ({sql}) AS zdbt_q LIMIT 0"))
                .context("preparing query")?;
            let names: Vec<String> = probe
                .columns()
                .iter()
                .map(|column| column.name().to_owned())
                .collect();
            emit(&ExploreEvent::Columns {
                names: names.clone(),
            });
            let display_list = probe
                .columns()
                .iter()
                .map(|column| {
                    format!(
                        "{}::text",
                        format!("\"{}\"", column.name().replace('\"', "\"\""))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let display_sql = format!(
                "SELECT {display_list} FROM ({sql}) AS zdbt_q LIMIT {limit}"
            );
            for row in client.query(&display_sql, &[])? {
                let cells = (0..names.len())
                    .map(|ix| row.get::<_, Option<String>>(ix))
                    .collect();
                emit(&ExploreEvent::Row { cells });
            }
        }
        other => bail!("unsupported kind {other:?}"),
    }
    emit(&ExploreEvent::Done);
    Ok(())
}
