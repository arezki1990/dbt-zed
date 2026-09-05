//! The remote extractor: database sources run in the on-demand
//! `zdbt-el-worker` binary, never in the main app. The worker streams one
//! JSON line per chunk on stdout and writes each chunk as an Arrow IPC
//! file; this side reads them back with polars. Killing the child (drop)
//! is cancellation.

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

/// One stdout line from the worker.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    /// The probed source schema, sent first.
    Schema { columns: Vec<(String, String)> },
    /// One extracted chunk, written to `path` as an Arrow IPC file.
    Chunk { path: PathBuf, rows: u64 },
    Done,
    Error { message: String },
}

pub struct RemoteExtractor {
    child: Child,
    stdout: std::io::BufReader<ChildStdout>,
    schema: Option<Schema>,
    done: bool,
    _scratch: tempfile::TempDir,
}

impl RemoteExtractor {
    /// Spawns `worker extract …`. `db_path` is already resolved (env
    /// templates applied, project-relative made absolute).
    pub fn spawn_duckdb(
        worker: &std::path::Path,
        db_path: &std::path::Path,
        schema: Option<&str>,
        table: &str,
        chunk_rows: usize,
    ) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating worker scratch dir")?;
        let mut command = Command::new(worker);
        command
            .arg("extract")
            .arg("--kind")
            .arg("duckdb")
            .arg("--db")
            .arg(db_path)
            .arg("--table")
            .arg(table)
            .arg("--chunk-rows")
            .arg(chunk_rows.to_string())
            .arg("--out-dir")
            .arg(scratch.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(schema) = schema {
            command.arg("--schema").arg(schema);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("spawning connector worker {}", worker.display()))?;
        let stdout = child.stdout.take().context("worker stdout")?;
        Ok(Self {
            child,
            stdout: std::io::BufReader::new(stdout),
            schema: None,
            done: false,
            _scratch: scratch,
        })
    }

    fn next_event(&mut self) -> Result<WorkerEvent> {
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.stdout.read_line(&mut line).context("reading worker")?;
            if read == 0 {
                let status = self.child.wait().ok();
                bail!(
                    "connector worker exited unexpectedly ({})",
                    status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "no status".into())
                );
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return serde_json::from_str(trimmed)
                .with_context(|| format!("bad worker event: {trimmed}"));
        }
    }

    fn dtype_from_wire(name: &str) -> DataType {
        match name {
            "bool" => DataType::Boolean,
            "i64" => DataType::Int64,
            "f64" => DataType::Float64,
            "date" => DataType::Date,
            "datetime_us" => DataType::Datetime(TimeUnit::Microseconds, None),
            _ => DataType::String,
        }
    }

    pub fn dtype_to_wire(dtype: &DataType) -> &'static str {
        match dtype {
            DataType::Boolean => "bool",
            DataType::Int64 => "i64",
            DataType::Float64 => "f64",
            DataType::Date => "date",
            DataType::Datetime(..) => "datetime_us",
            _ => "str",
        }
    }
}

impl Drop for RemoteExtractor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl super::Extractor for RemoteExtractor {
    fn schema(&mut self) -> Result<Schema> {
        if self.schema.is_none() {
            match self.next_event()? {
                WorkerEvent::Schema { columns } => {
                    self.schema = Some(Schema::from_iter(columns.into_iter().map(
                        |(name, dtype)| (name.into(), Self::dtype_from_wire(&dtype)),
                    )));
                }
                WorkerEvent::Error { message } => bail!("connector worker: {message}"),
                other => bail!("expected schema, got {other:?}"),
            }
        }
        Ok(self.schema.clone().expect("just set"))
    }

    fn next_chunk(&mut self) -> Result<Option<DataFrame>> {
        if self.done {
            return Ok(None);
        }
        if self.schema.is_none() {
            self.schema()?;
        }
        match self.next_event()? {
            WorkerEvent::Chunk { path, rows: _ } => {
                let file = std::fs::File::open(&path)
                    .with_context(|| format!("opening chunk {}", path.display()))?;
                let df = polars::prelude::IpcReader::new(file)
                    .finish()
                    .map_err(|error| anyhow!("reading chunk ipc: {error}"))?;
                let _ = std::fs::remove_file(&path);
                Ok(Some(df))
            }
            WorkerEvent::Done => {
                self.done = true;
                Ok(None)
            }
            WorkerEvent::Error { message } => bail!("connector worker: {message}"),
            WorkerEvent::Schema { .. } => bail!("unexpected second schema event"),
        }
    }
}
