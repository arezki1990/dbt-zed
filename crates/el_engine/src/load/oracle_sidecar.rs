//! Parent side of the Oracle warehouse loader: the same worker process
//! and the same JSON-lines protocol as the Snowflake and DuckDB targets.
//! Chunks travel as Arrow IPC files that the sidecar array-binds through
//! the INSERT statement generated here, so every statement Oracle runs
//! comes from `oracle_sql` and no value is ever interpolated into SQL.
//!
//! The only secret is the password, and it reaches the child through its
//! environment ([`ENV_ORACLE_PASSWORD`]) — never a request, an argument or
//! a log line.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use polars::prelude::{DataFrame, IpcWriter, SerWriter as _};

use super::protocol::{ENV_ORACLE_PASSWORD, Request, Response};
use super::{LoadReport, Loader, StreamPlan, oracle_sql};
use crate::connectors::oracle_env::ENV_TNS_ADMIN;
use crate::env::Secret;
use crate::oracle_types::OracleDialect;
use crate::types::SfBase;

/// Resolved Oracle target details. `user` and `connect` are locations;
/// `password` is a [`Secret`] and only reaches the child's environment.
pub struct OracleSidecarConfig {
    pub worker: PathBuf,
    pub user: String,
    pub connect: String,
    pub password: Secret,
    /// Directory holding `tnsnames.ora` / a wallet, when the connection
    /// names one.
    pub tns_admin: Option<Secret>,
    /// Client-location settings from the project's `.env`
    /// (`ZDBT_EL_ORACLE_CLIENT_DIR`, …), forwarded to the sidecar.
    pub client_settings: Vec<(&'static str, String)>,
    /// Target-side DDL choices (23ai native `BOOLEAN`, …).
    pub dialect: OracleDialect,
}

pub struct OracleSidecarLoader {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<ChildStdout>,
    scratch: tempfile::TempDir,
    chunk_index: usize,
    staged_rows: u64,
    dialect: OracleDialect,
}

impl OracleSidecarLoader {
    pub fn spawn(config: &OracleSidecarConfig) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating loader scratch dir")?;
        let mut command = Command::new(&config.worker);
        command
            .arg("oracle-loader")
            .env(ENV_ORACLE_PASSWORD, config.password.expose());
        if let Some(dir) = &config.tns_admin {
            command.env(ENV_TNS_ADMIN, dir.expose());
        }
        for (name, value) in &config.client_settings {
            command.env(name, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning loader worker {}", config.worker.display()))?;
        let stdin = child.stdin.take().context("loader stdin")?;
        let stdout = std::io::BufReader::new(child.stdout.take().context("loader stdout")?);
        let mut loader = Self {
            child,
            stdin,
            stdout,
            scratch,
            chunk_index: 0,
            staged_rows: 0,
            dialect: config.dialect,
        };
        loader.request(&Request::OpenOracle {
            user: config.user.clone(),
            connect_string: config.connect.clone(),
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

    fn exec_all(&mut self, statements: Vec<String>) -> Result<()> {
        for statement in statements {
            self.exec(statement)?;
        }
        Ok(())
    }
}

/// A `VARCHAR2(n CHAR)` column holds n characters and, on a standard
/// server, at most 4000 bytes. Oracle would reject an oversize value with
/// ORA-12899 after the chunk was shipped; this names the column and the
/// cast that fixes it before anything is sent.
fn check_text_widths(plan: &StreamPlan, chunk: &DataFrame) -> Result<()> {
    for (index, (name, sf_type)) in plan.columns.iter().enumerate() {
        if sf_type.base != SfBase::Varchar {
            continue;
        }
        let max_chars = sf_type.length.unwrap_or(OracleDialect::MAX_VARCHAR2);
        if max_chars > OracleDialect::MAX_VARCHAR2 {
            continue; // lands as CLOB
        }
        let Some(column) = chunk.columns().get(index) else {
            continue;
        };
        let Ok(text) = column.str() else {
            continue;
        };
        let (mut widest_bytes, mut widest_chars) = (0usize, 0usize);
        for value in text.iter().flatten() {
            widest_bytes = widest_bytes.max(value.len());
            widest_chars = widest_chars.max(value.chars().count());
        }
        if widest_chars > max_chars as usize
            || widest_bytes > OracleDialect::MAX_VARCHAR2 as usize
        {
            bail!(
                "column {name:?}: a value of {widest_chars} characters ({widest_bytes} bytes) \
                 does not fit VARCHAR2({max_chars} CHAR) — Oracle caps VARCHAR2 at 4000 \
                 bytes; add `cast: VARCHAR({})` to the stream so the column lands as CLOB",
                OracleDialect::MAX_VARCHAR2 + 1
            );
        }
    }
    Ok(())
}

impl Drop for OracleSidecarLoader {
    fn drop(&mut self) {
        let _ = self.request(&Request::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Loader for OracleSidecarLoader {
    fn begin(&mut self, plan: &StreamPlan) -> Result<()> {
        self.staged_rows = 0;
        if plan.mode == crate::spec::Mode::Incremental {
            self.exec(oracle_sql::create_target_if_not_exists(
                &plan.schema,
                &plan.target_table,
                &plan.columns,
                &self.dialect,
            )?)?;
        }
        let statements = oracle_sql::create_staging(
            &plan.schema,
            &plan.target_table,
            &plan.columns,
            &self.dialect,
        )?;
        self.exec_all(statements)
    }

    fn stage_chunk(&mut self, plan: &StreamPlan, chunk: &mut DataFrame) -> Result<u64> {
        check_text_widths(plan, chunk)?;
        let insert_sql =
            oracle_sql::insert_staging(&plan.schema, &plan.target_table, &plan.columns)?;
        let path = self
            .scratch
            .path()
            .join(format!("chunk-{:06}.ipc", self.chunk_index));
        self.chunk_index += 1;
        let file =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        IpcWriter::new(file)
            .finish(chunk)
            .map_err(|error| anyhow!("writing chunk ipc: {error}"))?;
        let response = self.request(&Request::IngestOracle {
            insert_sql,
            ipc_path: path.clone(),
        })?;
        let _ = std::fs::remove_file(&path);
        let rows = response.rows_affected.unwrap_or(chunk.height() as u64);
        self.staged_rows += rows;
        Ok(rows)
    }

    fn commit(&mut self, plan: &StreamPlan) -> Result<LoadReport> {
        let mut watermark_scalar = None;
        if plan.mode == crate::spec::Mode::Incremental {
            let (update_key, sf_type) = plan
                .update_key
                .as_ref()
                .ok_or_else(|| anyhow!("incremental commit without update_key"))?;
            self.exec(oracle_sql::merge(
                &plan.schema,
                &plan.target_table,
                &plan.columns,
                &plan.primary_key,
                update_key,
                &self.dialect,
            )?)?;
            let response = self.request(&Request::QueryScalar {
                sql: oracle_sql::max_scalar(
                    &plan.schema,
                    &plan.target_table,
                    update_key,
                    sf_type,
                )?,
            })?;
            watermark_scalar = response
                .scalar
                .and_then(|value| value.as_str().map(str::to_owned));
        } else {
            let statements = oracle_sql::full_refresh_swap(&plan.schema, &plan.target_table)?;
            self.exec_all(statements)?;
        }
        // After a swap the staging table IS the target; the guarded drop
        // is a no-op then, and cleans up after a MERGE.
        self.exec(oracle_sql::drop_staging(&plan.schema, &plan.target_table)?)?;
        Ok(LoadReport {
            rows_written: self.staged_rows,
            watermark_scalar,
        })
    }

    fn abort(&mut self, plan: &StreamPlan) -> Result<()> {
        self.exec(oracle_sql::drop_staging(&plan.schema, &plan.target_table)?)
            .map(|_| ())
    }
}
