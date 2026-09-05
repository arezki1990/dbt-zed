//! Parent side of the ADBC loader: spawns the on-demand worker in
//! `snowflake-loader` mode, sends [`protocol::Request`]s as JSON lines on
//! its stdin, and hands chunks over as Arrow IPC files. One sidecar per
//! run, reused across streams; killing the child is the cancel backstop.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use polars::prelude::{DataFrame, IpcWriter, SerWriter as _};

use super::protocol::{AuthMethod, ENV_PASSWORD, ENV_PRIVATE_KEY_PATH, Request, Response};
use super::{LoadReport, Loader, StreamPlan, snowflake_sql};
use crate::env::Secret;

/// Resolved Snowflake connection details for the sidecar. `secret` is the
/// password or the private-key path, per `auth` — it travels only via the
/// child's environment.
pub struct SidecarConfig {
    pub worker: PathBuf,
    pub driver_path: PathBuf,
    pub account: String,
    pub user: String,
    pub role: Option<String>,
    pub warehouse: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub auth: AuthMethod,
    pub secret: Secret,
}

pub struct AdbcSidecarLoader {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<ChildStdout>,
    scratch: tempfile::TempDir,
    chunk_index: usize,
    staged_rows: u64,
}

impl AdbcSidecarLoader {
    pub fn spawn(config: &SidecarConfig) -> Result<Self> {
        let scratch = tempfile::tempdir().context("creating loader scratch dir")?;
        let env_name = match config.auth {
            AuthMethod::Password => ENV_PASSWORD,
            AuthMethod::KeyPair => ENV_PRIVATE_KEY_PATH,
        };
        let mut child = Command::new(&config.worker)
            .arg("snowflake-loader")
            .env(env_name, config.secret.expose())
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
        };
        loader.request(&Request::Open {
            driver_path: config.driver_path.clone(),
            account: config.account.clone(),
            user: config.user.clone(),
            role: config.role.clone(),
            warehouse: config.warehouse.clone(),
            database: config.database.clone(),
            schema: config.schema.clone(),
            auth: config.auth,
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
                bail!(
                    "{}",
                    response.error.unwrap_or_else(|| "loader error".into())
                );
            }
            return Ok(response);
        }
    }

    fn exec(&mut self, sql: String) -> Result<Response> {
        self.request(&Request::Exec { sql })
    }

    pub fn shutdown(mut self) {
        let _ = self.request(&Request::Shutdown);
    }
}

impl Drop for AdbcSidecarLoader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Loader for AdbcSidecarLoader {
    fn begin(&mut self, plan: &StreamPlan) -> Result<()> {
        self.staged_rows = 0;
        if plan.mode == crate::spec::Mode::Incremental {
            self.exec(snowflake_sql::create_target_if_not_exists(
                plan.database.as_deref(),
                &plan.schema,
                &plan.target_table,
                &plan.columns,
            ))?;
        }
        self.exec(snowflake_sql::create_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
            &plan.columns,
        ))?;
        Ok(())
    }

    fn stage_chunk(&mut self, plan: &StreamPlan, chunk: &mut DataFrame) -> Result<u64> {
        let path = self
            .scratch
            .path()
            .join(format!("chunk-{:06}.ipc", self.chunk_index));
        self.chunk_index += 1;
        let file = std::fs::File::create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        IpcWriter::new(file)
            .finish(chunk)
            .map_err(|error| anyhow!("writing chunk ipc: {error}"))?;
        let table = snowflake_sql::fqn(
            plan.database.as_deref(),
            &plan.schema,
            &snowflake_sql::staging_table_name(&plan.target_table),
        );
        let response = self.request(&Request::Ingest {
            table,
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
            let (update_key, _) = plan
                .update_key
                .as_ref()
                .ok_or_else(|| anyhow!("incremental commit without update_key"))?;
            self.exec(snowflake_sql::merge(
                plan.database.as_deref(),
                &plan.schema,
                &plan.target_table,
                &plan.columns,
                &plan.primary_key,
                update_key,
            ))?;
            let response = self.request(&Request::QueryScalar {
                sql: snowflake_sql::max_scalar(
                    plan.database.as_deref(),
                    &plan.schema,
                    &plan.target_table,
                    update_key,
                ),
            })?;
            watermark_scalar = response
                .scalar
                .and_then(|value| value.as_str().map(str::to_owned));
        } else {
            self.exec(snowflake_sql::clone_swap(
                plan.database.as_deref(),
                &plan.schema,
                &plan.target_table,
            ))?;
        }
        self.exec(snowflake_sql::drop_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
        ))?;
        Ok(LoadReport {
            rows_written: self.staged_rows,
            watermark_scalar,
        })
    }

    fn abort(&mut self, plan: &StreamPlan) -> Result<()> {
        self.exec(snowflake_sql::drop_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
        ))
        .map(|_| ())
    }
}

/// Resolves the managed ADBC driver dylib, or an explicit override.
pub fn find_driver(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("ZDBT_ADBC_SNOWFLAKE_DRIVER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}
