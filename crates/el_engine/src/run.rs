//! The run orchestrator: extract → cast → stage → commit per stream, with
//! per-stream error isolation, progress events, and cooperative
//! cancellation. v1 executes full-refresh streams; incremental (MERGE +
//! watermarks) is the next phase and validated away until then.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::channel::mpsc::UnboundedSender;

use crate::cast::CastPlan;
use crate::connectors::SourceContext;
use crate::env::EnvMap;
use crate::load::adbc_sidecar::{AdbcSidecarLoader, SidecarConfig};
use crate::load::protocol::AuthMethod;
use crate::load::{Loader, StreamPlan};
use crate::progress::{CancelFlag, Phase, ProgressEvent};
use crate::spec::{Connection, Mode, Pipeline, SnowflakeAuth};

pub struct RunRequest {
    pub project_root: PathBuf,
    pub pipeline: Pipeline,
    /// Connector worker binary (also hosts the loader sidecar).
    pub worker: Option<PathBuf>,
    /// ADBC Snowflake driver dylib.
    pub driver: Option<PathBuf>,
    pub chunk_rows: usize,
}

#[derive(Clone, Debug, Default)]
pub struct RunReport {
    pub streams_ok: usize,
    pub streams_failed: usize,
    pub rows_written: u64,
}

/// Runs the whole pipeline, blocking. Call on a background thread; watch
/// `progress` for live state and `cancel` to stop between chunks.
pub fn run_pipeline(
    request: &RunRequest,
    progress: &UnboundedSender<ProgressEvent>,
    cancel: &CancelFlag,
) -> Result<RunReport> {
    let emit = |event: ProgressEvent| {
        let _ = progress.unbounded_send(event);
    };

    let pipeline = &request.pipeline;
    emit(ProgressEvent::RunStarted {
        pipeline: pipeline.pipeline.clone(),
        streams: pipeline
            .streams
            .iter()
            .map(|stream| stream.name.clone())
            .collect(),
    });

    let connections = crate::spec::load_connections(
        &request.project_root.join("el").join("connections.yml"),
    )
    .context("loading connections.yml")?;
    let env = EnvMap::load(&request.project_root, None);

    // Fail fast on anything validation can catch.
    let issues = crate::spec::validate(pipeline, &connections);
    let hard: Vec<_> = issues
        .iter()
        .filter(|issue| !issue.message.contains("references ${"))
        .collect();
    if !hard.is_empty() {
        bail!(
            "the pipeline has validation issues:\n{}",
            hard.iter()
                .map(|issue| format!("- {}", issue.message))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let mut loader = build_loader(request, &connections, &env)?;

    let source_connection = connections.connections.get(&pipeline.source);
    let ctx = SourceContext {
        project_root: &request.project_root,
        connection: source_connection,
        env: &env,
        worker: request.worker.as_deref(),
    };

    let state = crate::state::StateStore::open(&request.project_root)
        .context("opening incremental state store")?;

    let mut report = RunReport::default();
    for stream in &pipeline.streams {
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        emit(ProgressEvent::StreamStarted {
            stream: stream.name.clone(),
        });
        match run_stream(
            &ctx,
            pipeline,
            stream,
            loader.as_mut(),
            request,
            &state,
            &emit,
            cancel,
        ) {
            Ok((rows_read, rows_written, cast_failures)) => {
                report.streams_ok += 1;
                report.rows_written += rows_written;
                let _ = state.record_run(
                    &pipeline.pipeline,
                    &stream.name,
                    "ok",
                    rows_read,
                    rows_written,
                );
                emit(ProgressEvent::StreamFinished {
                    stream: stream.name.clone(),
                    rows_read,
                    rows_written,
                    cast_failures,
                });
            }
            Err(error) => {
                report.streams_failed += 1;
                let _ = state.record_run(&pipeline.pipeline, &stream.name, "failed", 0, 0);
                emit(ProgressEvent::StreamFailed {
                    stream: stream.name.clone(),
                    error: format!("{error:#}"),
                });
                if cancel.is_cancelled() {
                    break;
                }
            }
        }
    }

    emit(ProgressEvent::RunFinished {
        ok: report.streams_failed == 0,
    });
    Ok(report)
}

fn build_loader(
    request: &RunRequest,
    connections: &crate::spec::Connections,
    env: &EnvMap,
) -> Result<Box<dyn Loader>> {
    let target_name = &request.pipeline.target.connection;
    let Some(worker) = request.worker.clone() else {
        bail!(
            "connector support is not installed — the loader runs in the zdbt worker binary"
        );
    };
    // DuckDB warehouse: fully local, zero credentials — the test target.
    if let Some(Connection::Duckdb(conn)) = connections.connections.get(target_name) {
        let path = crate::env::resolve_templates(&conn.path, env)
            .map_err(|missing| anyhow!("{missing}"))?;
        let path = std::path::PathBuf::from(path.expose());
        let path = if path.is_absolute() {
            path
        } else {
            request.project_root.join(path)
        };
        return Ok(Box::new(
            crate::load::duckdb_sidecar::DuckdbSidecarLoader::spawn(&worker, &path)?,
        ));
    }
    let Some(Connection::Snowflake(conn)) = connections.connections.get(target_name) else {
        bail!("target connection {target_name:?} is not a snowflake or duckdb connection");
    };
    let Some(driver) = crate::load::adbc_sidecar::find_driver(request.driver.as_deref()) else {
        bail!(
            "the ADBC Snowflake driver is not installed — set ZDBT_ADBC_SNOWFLAKE_DRIVER \
             to the libadbc_driver_snowflake dylib (managed download coming with settings)"
        );
    };

    let resolve = |value: &str| {
        crate::env::resolve_templates(value, env).map_err(|missing| anyhow!("{missing}"))
    };
    let (auth, secret) = match &conn.auth {
        SnowflakeAuth::KeyPair { private_key_path } => {
            (AuthMethod::KeyPair, resolve(private_key_path)?)
        }
        SnowflakeAuth::Password { password } => (AuthMethod::Password, resolve(password)?),
    };
    let config = SidecarConfig {
        worker,
        driver_path: driver,
        account: resolve(&conn.account)?.expose().to_owned(),
        user: resolve(&conn.user)?.expose().to_owned(),
        role: conn
            .role
            .as_deref()
            .map(resolve)
            .transpose()?
            .map(|secret| secret.expose().to_owned()),
        warehouse: conn
            .warehouse
            .as_deref()
            .map(resolve)
            .transpose()?
            .map(|secret| secret.expose().to_owned()),
        database: request
            .pipeline
            .target
            .database
            .clone()
            .or_else(|| conn.database.clone()),
        schema: Some(request.pipeline.target.schema.clone()),
        auth,
        secret,
    };
    Ok(Box::new(AdbcSidecarLoader::spawn(&config)?))
}

fn run_stream(
    ctx: &SourceContext,
    pipeline: &Pipeline,
    stream: &crate::spec::StreamSpec,
    loader: &mut dyn Loader,
    request: &RunRequest,
    state: &crate::state::StateStore,
    emit: &dyn Fn(ProgressEvent),
    cancel: &CancelFlag,
) -> Result<(u64, u64, u64)> {
    let mode = stream.mode(pipeline.defaults.as_ref());
    // The incremental cursor from the last successful commit. Stream name
    // is the cursor's identity: rename the stream, reset the cursor.
    let resume = (mode == Mode::Incremental)
        .then(|| state.watermark(&pipeline.pipeline, &stream.name))
        .flatten();
    let mut extractor =
        crate::connectors::make_extractor(ctx, stream, request.chunk_rows, resume.as_ref())?;
    emit(ProgressEvent::Chunk {
        stream: stream.name.clone(),
        phase: Phase::Connect,
        rows_read: 0,
        rows_written: 0,
        cast_failures: 0,
    });
    let schema = extractor.schema()?;
    let plan = CastPlan::build(&schema, stream)?;

    // Target-side names for pk/cursor: renames apply.
    let target_name_of = |source: &str| -> String {
        plan.target_columns()
            .iter()
            .map(|(name, _)| name.clone())
            .zip(plan.source_names())
            .find(|(_, src)| src == source)
            .map(|(target, _)| target)
            .unwrap_or_else(|| source.to_owned())
    };
    let update_key = stream.update_key.as_ref().map(|source| {
        let target = target_name_of(source);
        let sf_type = plan
            .target_columns()
            .iter()
            .find(|(name, _)| name == &target)
            .map(|(_, sf_type)| sf_type.clone())
            .unwrap_or_else(|| crate::types::SnowflakeType::from_polars(
                &polars::prelude::DataType::String,
            ));
        (target, sf_type)
    });
    let stream_plan = StreamPlan {
        database: pipeline.target.database.clone(),
        schema: pipeline.target.schema.clone(),
        target_table: stream.target_table(&pipeline.target),
        columns: plan.target_columns(),
        mode,
        primary_key: stream
            .primary_key
            .iter()
            .map(|key| target_name_of(key))
            .collect(),
        update_key,
    };

    loader.begin(&stream_plan)?;
    let mut rows_read = 0u64;
    let mut rows_written = 0u64;
    let mut cast_failures = 0u64;
    let result = (|| -> Result<()> {
        loop {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            let Some(chunk) = extractor.next_chunk()? else {
                break;
            };
            rows_read += chunk.height() as u64;
            emit(ProgressEvent::Chunk {
                stream: stream.name.clone(),
                phase: Phase::Cast,
                rows_read,
                rows_written,
                cast_failures,
            });
            let outcome = plan.apply(chunk, 8)?;
            cast_failures += outcome
                .failures
                .iter()
                .map(|failure| failure.count)
                .sum::<u64>();
            let mut shaped = outcome.df;
            emit(ProgressEvent::Chunk {
                stream: stream.name.clone(),
                phase: Phase::Stage,
                rows_read,
                rows_written,
                cast_failures,
            });
            rows_written += loader.stage_chunk(&stream_plan, &mut shaped)?;
            emit(ProgressEvent::Chunk {
                stream: stream.name.clone(),
                phase: Phase::Stage,
                rows_read,
                rows_written,
                cast_failures,
            });
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            emit(ProgressEvent::Chunk {
                stream: stream.name.clone(),
                phase: Phase::Copy,
                rows_read,
                rows_written,
                cast_failures,
            });
            match loader.commit(&stream_plan) {
                Ok(commit) => {
                    // Persist the cursor read back FROM THE TARGET — a
                    // crash between load and this write only re-extracts
                    // rows the MERGE absorbs.
                    if let (Some(scalar), Some((_, sf_type))) =
                        (&commit.watermark_scalar, &stream_plan.update_key)
                    {
                        if let Some(watermark) =
                            crate::state::WatermarkValue::parse_scalar(scalar, sf_type)
                        {
                            let _ = state.set_watermark(
                                &pipeline.pipeline,
                                &stream.name,
                                &watermark,
                            );
                        }
                    }
                    Ok((rows_read, commit.rows_written, cast_failures))
                }
                Err(error) => {
                    // A failed commit must not leak the staging table.
                    let _ = loader.abort(&stream_plan);
                    Err(error)
                }
            }
        }
        Err(error) => {
            let _ = loader.abort(&stream_plan);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::MockLoader;

    fn fixture_pipeline(dir: &std::path::Path) -> Pipeline {
        std::fs::create_dir_all(dir.join("el")).unwrap();
        std::fs::write(
            dir.join("el").join("connections.yml"),
            r#"version: 1
connections:
  files: { type: local }
  wh:
    type: snowflake
    account: acct
    user: loader
    auth: { method: password, password: "${ZDBT_TEST_SF_PASSWORD}" }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("data.csv"),
            "id,amount\n1,10.5\n2,20.0\n3,oops\n",
        )
        .unwrap();
        serde_yaml_ng::from_str(
            r#"version: 1
pipeline: t
source: files
target: { connection: wh, database: RAW, schema: LANDING }
streams:
- name: data
  source: { path: data.csv, format: csv }
  columns:
  - { name: amount, cast: FLOAT }
"#,
        )
        .unwrap()
    }

    /// The orchestrator drives extract→cast→stage→commit with the right
    /// SQL in the right order — no warehouse involved.
    #[test]
    fn full_refresh_through_mock_loader() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = fixture_pipeline(dir.path());
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let cancel = CancelFlag::default();

        let mut loader = MockLoader::default();
        let connections =
            crate::spec::load_connections(&dir.path().join("el/connections.yml")).unwrap();
        let env = EnvMap::empty();
        let ctx = SourceContext {
            project_root: dir.path(),
            connection: connections.connections.get("files"),
            env: &env,
            worker: None,
        };
        let request = RunRequest {
            project_root: dir.path().to_path_buf(),
            pipeline: pipeline.clone(),
            worker: None,
            driver: None,
            chunk_rows: 2,
        };
        let emit = |event: ProgressEvent| {
            let _ = tx.unbounded_send(event);
        };
        let state_dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", state_dir.path()) };
        let state = crate::state::StateStore::open(dir.path()).unwrap();
        let (rows_read, rows_written, cast_failures) = run_stream(
            &ctx,
            &pipeline,
            &pipeline.streams[0],
            &mut loader,
            &request,
            &state,
            &emit,
            &cancel,
        )
        .unwrap();
        unsafe { std::env::remove_var("ZDBT_EL_STATE_DIR") };

        assert_eq!(rows_read, 3);
        assert_eq!(rows_written, 3);
        assert_eq!(cast_failures, 1, "the 'oops' cell");
        let sql = loader.statements.join("\n");
        assert!(sql.contains("CREATE OR REPLACE TRANSIENT TABLE"), "{sql}");
        assert!(sql.contains("INGEST 2"), "chunked: {sql}");
        assert!(sql.contains("INGEST 1"), "chunked: {sql}");
        assert!(sql.contains("CLONE"), "{sql}");
        assert!(sql.contains("DROP TABLE IF EXISTS"), "{sql}");
        // Progress events flowed.
        drop(tx);
        let events: Vec<_> = std::iter::from_fn(|| rx.try_next().ok().flatten()).collect();
        assert!(events.len() >= 4, "{}", events.len());
    }

    /// A failing commit aborts (drops staging) and surfaces the error.
    #[test]
    fn failed_commit_aborts() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = fixture_pipeline(dir.path());
        let cancel = CancelFlag::default();
        let mut loader = MockLoader {
            fail_on_commit: true,
            ..Default::default()
        };
        let connections =
            crate::spec::load_connections(&dir.path().join("el/connections.yml")).unwrap();
        let env = EnvMap::empty();
        let ctx = SourceContext {
            project_root: dir.path(),
            connection: connections.connections.get("files"),
            env: &env,
            worker: None,
        };
        let request = RunRequest {
            project_root: dir.path().to_path_buf(),
            pipeline: pipeline.clone(),
            worker: None,
            driver: None,
            chunk_rows: 10,
        };
        let state_dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", state_dir.path()) };
        let state = crate::state::StateStore::open(dir.path()).unwrap();
        let result = run_stream(
            &ctx,
            &pipeline,
            &pipeline.streams[0],
            &mut loader,
            &request,
            &state,
            &|_| {},
            &cancel,
        );
        unsafe { std::env::remove_var("ZDBT_EL_STATE_DIR") };
        assert!(result.is_err());
        assert!(
            loader.statements.last().unwrap().contains("DROP TABLE"),
            "abort must drop staging: {:?}",
            loader.statements
        );
    }
}
