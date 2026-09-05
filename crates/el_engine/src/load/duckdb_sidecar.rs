//! Parent side of the DuckDB warehouse loader: same worker process, same
//! JSON-lines protocol as the Snowflake path — but chunks travel as
//! parquet files that DuckDB ingests natively via `read_parquet`, and no
//! driver dylib or credential is involved. The fully-local warehouse for
//! testing EL end to end.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use polars::prelude::{DataFrame, ParquetWriter};

use super::protocol::{Request, Response};
use super::{LoadReport, Loader, StreamPlan, duckdb_sql};

pub struct DuckdbSidecarLoader {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<ChildStdout>,
    scratch: tempfile::TempDir,
    chunk_index: usize,
    staged_rows: u64,
}

impl DuckdbSidecarLoader {
    pub fn spawn(worker: &Path, warehouse_path: &Path) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating loader scratch dir")?;
        if let Some(parent) = warehouse_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut child = Command::new(worker)
            .arg("duckdb-loader")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning loader worker {}", worker.display()))?;
        let stdin = child.stdin.take().context("loader stdin")?;
        let stdout = std::io::BufReader::new(child.stdout.take().context("loader stdout")?);
        let mut loader = Self {
            child,
            stdin,
            stdout,
            scratch,
            chunk_index: 0,
            staged_rows: 0,
        };
        loader.request(&Request::OpenDuckdb {
            path: warehouse_path.to_path_buf(),
        })?;
        Ok(loader)
    }

    fn request(&mut self, request: &Request) -> Result<Response> {
        let line = serde_json::to_string(request).context("encoding request")?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .context("writing to loader")?;
        let mut reply = String::new();
        loop {
            reply.clear();
            let read = self
                .stdout
                .read_line(&mut reply)
                .context("reading loader reply")?;
            if read == 0 {
                let status = self.child.wait().ok();
                bail!(
                    "loader worker exited unexpectedly ({})",
                    status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "no status".into())
                );
            }
            let trimmed = reply.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response: Response = serde_json::from_str(trimmed)
                .with_context(|| format!("bad loader reply: {trimmed}"))?;
            if !response.ok {
                bail!("{}", response.error.unwrap_or_else(|| "loader error".into()));
            }
            return Ok(response);
        }
    }

    fn exec(&mut self, sql: String) -> Result<Response> {
        self.request(&Request::Exec { sql })
    }
}

impl Drop for DuckdbSidecarLoader {
    fn drop(&mut self) {
        let _ = self.request(&Request::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Loader for DuckdbSidecarLoader {
    fn begin(&mut self, plan: &StreamPlan) -> Result<()> {
        self.staged_rows = 0;
        self.exec(duckdb_sql::create_schema(&plan.schema))?;
        if plan.mode == crate::spec::Mode::Incremental {
            self.exec(duckdb_sql::create_target_if_not_exists(
                &plan.schema,
                &plan.target_table,
                &plan.columns,
            ))?;
        }
        self.exec(duckdb_sql::create_staging(
            &plan.schema,
            &plan.target_table,
            &plan.columns,
        ))?;
        Ok(())
    }

    fn stage_chunk(&mut self, plan: &StreamPlan, chunk: &mut DataFrame) -> Result<u64> {
        let path: PathBuf = self
            .scratch
            .path()
            .join(format!("chunk-{:06}.parquet", self.chunk_index));
        self.chunk_index += 1;
        let file = std::fs::File::create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        ParquetWriter::new(file)
            .finish(chunk)
            .map_err(|error| anyhow!("writing chunk parquet: {error}"))?;
        let response = self.request(&Request::Exec {
            sql: duckdb_sql::ingest_parquet(
                &plan.schema,
                &plan.target_table,
                path.to_str().context("non-utf8 chunk path")?,
            ),
        })?;
        let _ = std::fs::remove_file(&path);
        let rows = response.rows_affected.unwrap_or(chunk.height() as u64);
        self.staged_rows += rows;
        Ok(rows)
    }

    fn commit(&mut self, plan: &StreamPlan) -> Result<LoadReport> {
        let mut watermark_scalar = None;
        if plan.mode == crate::spec::Mode::Incremental {
            let (update_key, _) = plan
                .update_key
                .as_ref()
                .ok_or_else(|| anyhow!("incremental commit without update_key"))?;
            self.exec(duckdb_sql::upsert(
                &plan.schema,
                &plan.target_table,
                &plan.primary_key,
                update_key,
            ))?;
            let response = self.request(&Request::QueryScalar {
                sql: duckdb_sql::max_scalar(&plan.schema, &plan.target_table, update_key),
            })?;
            watermark_scalar = response
                .scalar
                .and_then(|value| value.as_str().map(str::to_owned));
        } else {
            self.exec(duckdb_sql::swap(&plan.schema, &plan.target_table))?;
        }
        self.exec(duckdb_sql::drop_staging(&plan.schema, &plan.target_table))?;
        Ok(LoadReport {
            rows_written: self.staged_rows,
            watermark_scalar,
        })
    }

    fn abort(&mut self, plan: &StreamPlan) -> Result<()> {
        self.exec(duckdb_sql::drop_staging(&plan.schema, &plan.target_table))
            .map(|_| ())
    }
}
