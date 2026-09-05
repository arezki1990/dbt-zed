//! Ad-hoc exploration of EL connections: list tables, run a capped query.
//! Everything goes through the on-demand worker — the app keeps zero
//! drivers. Results are pre-stringified for direct display.

use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::env::EnvMap;
use crate::spec::Connection;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ExploreEvent {
    Tables { items: Vec<(String, String)> },
    Columns { names: Vec<String> },
    Row { cells: Vec<Option<String>> },
    Done,
    Error { message: String },
}

pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Builds the worker command for a connection; the URL travels via child
/// env, the duckdb path via argv (a path is a location, not a secret).
fn connection_args(
    command: &mut Command,
    project_root: &Path,
    connection: &Connection,
    env: &EnvMap,
) -> Result<()> {
    match connection {
        Connection::Duckdb(conn) => {
            let resolved = crate::env::resolve_templates(&conn.path, env)
                .map_err(|missing| anyhow::anyhow!("{missing}"))?;
            let path = PathBuf::from(resolved.expose());
            let path = if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            };
            command.arg("--kind").arg("duckdb").arg("--db").arg(path);
        }
        Connection::Postgres(conn) => {
            let url = crate::env::resolve_templates(&conn.url, env)
                .map_err(|missing| anyhow::anyhow!("{missing}"))?;
            command.arg("--kind").arg("postgres");
            command.env("ZDBT_EL_SRC_URL", url.expose());
        }
        other => bail!("browsing {} connections is not supported yet", other.kind()),
    }
    Ok(())
}

fn read_events(
    worker: &Path,
    configure: impl FnOnce(&mut Command) -> Result<()>,
) -> Result<Vec<ExploreEvent>> {
    let mut command = Command::new(worker);
    configure(&mut command)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("spawning connector worker")?;
    let stdout = child.stdout.take().context("worker stdout")?;
    // Watchdog: whatever the driver does, the UI gets an answer. The
    // worker's own connect timeouts fire far earlier; this is the
    // backstop for anything else that wedges.
    let watchdog_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let done = std::sync::Arc::clone(&watchdog_flag);
        let timed_out = std::sync::Arc::clone(&timed_out);
        let pid = child.id();
        std::thread::spawn(move || {
            for _ in 0..900 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if done.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
            }
            timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        });
    }
    let mut events = Vec::new();
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.context("reading worker")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: ExploreEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("bad worker event: {trimmed}"))?;
        if let ExploreEvent::Error { message } = &event {
            let _ = child.kill();
            bail!("{message}");
        }
        events.push(event);
    }
    let status = child.wait().context("waiting for worker")?;
    watchdog_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    if timed_out.load(std::sync::atomic::Ordering::Relaxed) {
        bail!("timed out after 90s — is the database reachable?");
    }
    if !status.success() && events.is_empty() {
        bail!("connector worker failed ({status})");
    }
    Ok(events)
}

/// Every (schema, table) the connection can see, sorted.
pub fn list_tables(
    worker: &Path,
    project_root: &Path,
    connection: &Connection,
    env: &EnvMap,
) -> Result<Vec<(String, String)>> {
    let events = read_events(worker, |command| {
        command.arg("list");
        connection_args(command, project_root, connection, env)
    })?;
    for event in events {
        if let ExploreEvent::Tables { mut items } = event {
            items.sort();
            return Ok(items);
        }
    }
    Ok(Vec::new())
}

/// Runs `sql` capped at `limit` rows. The SQL travels via a scratch file —
/// never argv — so arbitrary text needs no shell quoting.
pub fn run_query(
    worker: &Path,
    project_root: &Path,
    connection: &Connection,
    env: &EnvMap,
    sql: &str,
    limit: usize,
) -> Result<QueryResult> {
    let scratch = tempfile::tempdir().context("creating query scratch")?;
    let sql_path = scratch.path().join("query.sql");
    std::fs::write(&sql_path, sql).context("writing query file")?;

    let events = read_events(worker, |command| {
        command
            .arg("query")
            .arg("--sql-file")
            .arg(&sql_path)
            .arg("--limit")
            .arg(limit.to_string());
        connection_args(command, project_root, connection, env)
    })?;

    let mut result = QueryResult {
        columns: Vec::new(),
        rows: Vec::new(),
    };
    for event in events {
        match event {
            ExploreEvent::Columns { names } => result.columns = names,
            ExploreEvent::Row { cells } => result.rows.push(
                cells
                    .into_iter()
                    .map(|cell| cell.unwrap_or_default())
                    .collect(),
            ),
            _ => {}
        }
    }
    Ok(result)
}
