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
        cursor: Option<(String, crate::state::WatermarkValue)>,
    ) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating worker scratch dir")?;
        let mut command = Self::extract_command(
            worker,
            "duckdb",
            schema,
            table,
            chunk_rows,
            scratch.path(),
            &cursor,
        )?;
        command.arg("--db").arg(db_path);
        Self::spawn(worker, command, scratch)
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

    /// The wire vocabulary. Only the schema travels this way (chunks
    /// carry their real dtypes in the Arrow IPC file), but the cast plan
    /// is built from it, so a name that loses information makes the plan
    /// wrong. Unknown names decode as text, which keeps older workers
    /// readable.
    fn dtype_from_wire(name: &str) -> DataType {
        match name {
            "bool" => DataType::Boolean,
            "i64" => DataType::Int64,
            "f64" => DataType::Float64,
            "date" => DataType::Date,
            "datetime_us" => DataType::Datetime(TimeUnit::Microseconds, None),
            "datetime_us_utc" => {
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
            }
            "binary" => DataType::Binary,
            _ => match name
                .strip_prefix("decimal_")
                .and_then(|rest| rest.split_once('_'))
                .and_then(|(precision, scale)| {
                    Some((precision.parse().ok()?, scale.parse().ok()?))
                }) {
                Some((precision, scale)) => DataType::Decimal(precision, scale),
                None => DataType::String,
            },
        }
    }

    pub fn dtype_to_wire(dtype: &DataType) -> String {
        match dtype {
            DataType::Boolean => "bool".to_owned(),
            DataType::Int64 => "i64".to_owned(),
            DataType::Float64 => "f64".to_owned(),
            DataType::Date => "date".to_owned(),
            DataType::Datetime(_, None) => "datetime_us".to_owned(),
            DataType::Datetime(_, Some(_)) => "datetime_us_utc".to_owned(),
            DataType::Decimal(precision, scale) => format!("decimal_{precision}_{scale}"),
            DataType::Binary => "binary".to_owned(),
            _ => "str".to_owned(),
        }
    }
}

impl RemoteExtractor {
    /// The shared `extract` invocation; each kind adds its own locators
    /// (argv) and credentials (child env) before spawning.
    fn extract_command(
        worker: &std::path::Path,
        kind: &str,
        schema: Option<&str>,
        table: &str,
        chunk_rows: usize,
        out_dir: &std::path::Path,
        cursor: &Option<(String, crate::state::WatermarkValue)>,
    ) -> Result<Command> {
        let mut command = Command::new(worker);
        command
            .arg("extract")
            .arg("--kind")
            .arg(kind)
            .arg("--table")
            .arg(table)
            .arg("--chunk-rows")
            .arg(chunk_rows.to_string())
            .arg("--out-dir")
            .arg(out_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(schema) = schema {
            command.arg("--schema").arg(schema);
        }
        if let Some((column, value)) = cursor {
            command
                .arg("--update-key")
                .arg(column)
                .arg("--cursor")
                .arg(serde_json::to_string(value).context("encoding cursor")?);
        }
        Ok(command)
    }

    fn spawn(
        worker: &std::path::Path,
        mut command: Command,
        scratch: tempfile::TempDir,
    ) -> Result<Self> {
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

    /// URL (with credentials) travels via the child environment, never argv.
    pub fn spawn_postgres(
        worker: &std::path::Path,
        url: crate::env::Secret,
        schema: Option<&str>,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, crate::state::WatermarkValue)>,
    ) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating worker scratch dir")?;
        let mut command = Self::extract_command(
            worker,
            "postgres",
            schema,
            table,
            chunk_rows,
            scratch.path(),
            &cursor,
        )?;
        command.env("ZDBT_EL_SRC_URL", url.expose());
        Self::spawn(worker, command, scratch)
    }

    /// Oracle credentials are discrete, so they travel as their own child
    /// environment variables — never argv. The schema is a location, not
    /// a secret, and is always resolved by the caller.
    pub fn spawn_oracle(
        worker: &std::path::Path,
        creds: &super::oracle_env::OracleCreds,
        schema: &str,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, crate::state::WatermarkValue)>,
    ) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating worker scratch dir")?;
        let mut command = Self::extract_command(
            worker,
            "oracle",
            Some(schema),
            table,
            chunk_rows,
            scratch.path(),
            &cursor,
        )?;
        creds.apply(&mut command);
        Self::spawn(worker, command, scratch)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema wire vocabulary must round-trip every dtype a connector
    /// can produce — a lossy name would make the parent's cast plan
    /// disagree with the chunks it reads.
    #[test]
    fn wire_dtypes_round_trip() {
        for dtype in [
            DataType::Boolean,
            DataType::Int64,
            DataType::Float64,
            DataType::Date,
            DataType::Datetime(TimeUnit::Microseconds, None),
            DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC)),
            DataType::Decimal(18, 2),
            DataType::Decimal(38, 0),
            DataType::Binary,
            DataType::String,
        ] {
            let wire = RemoteExtractor::dtype_to_wire(&dtype);
            assert_eq!(
                RemoteExtractor::dtype_from_wire(&wire),
                dtype,
                "round trip of {wire}"
            );
        }
        // Anything a newer worker invents still decodes as text.
        assert_eq!(RemoteExtractor::dtype_from_wire("str"), DataType::String);
        assert_eq!(
            RemoteExtractor::dtype_from_wire("something_new"),
            DataType::String
        );
        assert_eq!(
            RemoteExtractor::dtype_from_wire("decimal_x_y"),
            DataType::String
        );
    }
}
