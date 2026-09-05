//! The `el serve` daemon: runs pipelines on cron schedules with retries
//! and failure hooks, and exposes a small JSON API the IDE (or curl)
//! drives — trigger runs, list history, poll live progress events.
//!
//! Blocking and threaded like the rest of the engine: no async runtime.
//! Live progress is polled (`GET /runs/:id/events?since=n`), which the
//! IDE turns into the same ticking rows a local run shows.
//!
//! Auth: `ZDBT_EL_TOKEN` as a bearer token. Binding beyond loopback
//! REQUIRES the token; loopback may run without one.

use std::collections::HashMap;
use std::io::Read as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use serde::Serialize;

use crate::progress::{CancelFlag, ProgressEvent};
use crate::run::{RunRequest, run_pipeline};
use crate::spec::{self, Pipeline};

const HISTORY_LIMIT: usize = 500;
const SCHEDULER_TICK: Duration = Duration::from_secs(15);
const DEFAULT_BACKOFF: Duration = Duration::from_secs(60);

pub struct ServerConfig {
    pub project_root: PathBuf,
    pub listen: SocketAddr,
    /// Bearer token; required unless listening on loopback.
    pub token: Option<String>,
    /// PEM certificate + private key files: serve HTTPS natively.
    pub tls: Option<(PathBuf, PathBuf)>,
    /// Permit plaintext HTTP beyond loopback (token still required) — for
    /// private networks and containers where TLS terminates at ingress.
    pub allow_insecure_http: bool,
    pub worker: Option<PathBuf>,
    pub driver: Option<PathBuf>,
    pub chunk_rows: usize,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Running,
    Ok,
    Failed,
    Cancelled,
}

struct RunRecord {
    id: u64,
    pipeline: String,
    started: SystemTime,
    attempt: u32,
    status: RunStatus,
    error: Option<String>,
    rows_written: u64,
    events: Vec<ProgressEvent>,
    cancel: CancelFlag,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    runs: Vec<RunRecord>,
}

struct State {
    config: ServerConfig,
    registry: Mutex<Registry>,
    started: SystemTime,
    /// The daemon's own activity log — a ring the IDE polls at /logs.
    log_lines: Mutex<Vec<(u64, String)>>,
    log_next: std::sync::atomic::AtomicU64,
}

const LOG_LIMIT: usize = 1000;

impl State {
    /// Logs to the ring AND the process log.
    fn log(&self, line: String) {
        log::info!("el serve: {line}");
        println!("el-serve  {line}");
        let stamp = chrono::Utc::now().format("%H:%M:%S");
        let mut lines = self.log_lines.lock().unwrap();
        // Sequence allocated under the ring's lock: /logs pagination sees
        // strictly ordered, gap-free entries.
        let seq = self
            .log_next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        lines.push((seq, format!("{stamp}  {line}")));
        let overflow = lines.len().saturating_sub(LOG_LIMIT);
        if overflow > 0 {
            lines.drain(..overflow);
        }
    }

    fn is_running(&self, pipeline: &str) -> bool {
        self.registry
            .lock()
            .unwrap()
            .runs
            .iter()
            .any(|run| run.pipeline == pipeline && run.status == RunStatus::Running)
    }
}

fn load_pipeline_by_name(root: &std::path::Path, name: &str) -> Result<Pipeline> {
    for path in spec::list_pipelines(&root.join("el")) {
        if let Ok(pipeline) = spec::load_pipeline(&path) {
            if pipeline.pipeline == name {
                return Ok(pipeline);
            }
        }
    }
    bail!("no pipeline named {name:?} under el/pipelines/")
}

/// Starts a run on background threads; returns its id. Refuses when the
/// pipeline already has a run in flight (Airbyte's skip-if-running).
fn start_run(state: &Arc<State>, pipeline_name: &str, attempt: u32) -> Result<u64> {
    let pipeline = load_pipeline_by_name(&state.config.project_root, pipeline_name)?;
    let cancel = CancelFlag::default();
    let run_id = {
        // Check-and-insert under ONE lock: two racing POSTs can't both
        // start the same pipeline.
        let mut registry = state.registry.lock().unwrap();
        if registry
            .runs
            .iter()
            .any(|run| run.pipeline == pipeline_name && run.status == RunStatus::Running)
        {
            bail!("{pipeline_name} is already running");
        }
        registry.next_id += 1;
        let id = registry.next_id;
        registry.runs.push(RunRecord {
            id,
            pipeline: pipeline_name.to_owned(),
            started: SystemTime::now(),
            attempt,
            status: RunStatus::Running,
            error: None,
            rows_written: 0,
            events: Vec::new(),
            cancel: cancel.clone(),
        });
        let overflow = registry.runs.len().saturating_sub(HISTORY_LIMIT);
        if overflow > 0 {
            // Never drop a running record, however old.
            let mut kept = 0;
            registry
                .runs
                .retain(|run| {
                    let drop_it = kept < overflow && run.status != RunStatus::Running;
                    if drop_it {
                        kept += 1;
                    }
                    !drop_it
                });
        }
        id
    };
    state.log(format!("run {run_id} ({pipeline_name}) started (attempt {attempt})"));

    let request = RunRequest {
        project_root: state.config.project_root.clone(),
        pipeline: pipeline.clone(),
        worker: state.config.worker.clone(),
        driver: state.config.driver.clone(),
        chunk_rows: state.config.chunk_rows,
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();

    // Consumer: append every event to the record.
    let consumer_state = Arc::clone(state);
    let consumer = std::thread::spawn(move || {
        use futures::StreamExt as _;
        while let Some(event) = futures::executor::block_on(rx.next()) {
            let mut registry = consumer_state.registry.lock().unwrap();
            if let Some(run) = registry.runs.iter_mut().find(|run| run.id == run_id) {
                if let ProgressEvent::StreamFinished { rows_written, .. } = &event {
                    run.rows_written += rows_written;
                }
                run.events.push(event);
            }
        }
    });

    // Engine thread: the blocking run, then verdict + retry + hooks.
    let engine_state = Arc::clone(state);
    let name = pipeline_name.to_owned();
    let engine_thread = std::thread::Builder::new()
        .name(format!("el-run-{name}"))
        .spawn(move || {
            let result = run_pipeline(&request, &tx, &cancel);
            drop(tx);
            // The record goes terminal only after every event is appended
            // — a poller that sees done has seen the whole tail.
            let _ = consumer.join();
            let was_cancelled = cancel.is_cancelled();
            let failed_error = match &result {
                Ok(report) if report.streams_failed == 0 => None,
                Ok(report) => Some(format!("{} stream(s) failed", report.streams_failed)),
                Err(error) => Some(format!("{error:#}")),
            };
            {
                let mut registry = engine_state.registry.lock().unwrap();
                if let Some(run) = registry.runs.iter_mut().find(|run| run.id == run_id) {
                    run.status = if was_cancelled {
                        RunStatus::Cancelled
                    } else if failed_error.is_none() {
                        RunStatus::Ok
                    } else {
                        RunStatus::Failed
                    };
                    run.error = if was_cancelled {
                        Some("cancelled".to_owned())
                    } else {
                        failed_error.clone()
                    };
                }
            }
            if was_cancelled {
                // A cancel is the operator's verdict: no retries, no
                // failure hooks.
                engine_state.log(format!("run {run_id} ({name}) cancelled"));
                return;
            }
            match &failed_error {
                None => engine_state.log(format!("run {run_id} ({name}) finished")),
                Some(error) => {
                    engine_state.log(format!("run {run_id} ({name}) failed: {error}"))
                }
            }
            let Some(error) = failed_error else { return };

            let retry = request.pipeline.retry.clone();
            let attempts_allowed = retry.as_ref().map(|retry| retry.attempts).unwrap_or(0);
            if attempt < attempts_allowed {
                let backoff = retry
                    .as_ref()
                    .and_then(|retry| retry.backoff.as_deref())
                    .and_then(spec::parse_backoff)
                    .unwrap_or(DEFAULT_BACKOFF);
                engine_state.log(format!(
                    "retrying {name} in {}s (attempt {}/{attempts_allowed})",
                    backoff.as_secs(),
                    attempt + 1
                ));
                std::thread::sleep(backoff);
                if let Err(error) = start_run(&engine_state, &name, attempt + 1) {
                    log::warn!("el serve: retry of {name} not started: {error:#}");
                }
            } else {
                fire_failure_hooks(&engine_state, &request.pipeline, &error);
            }
        });
    if let Err(spawn_error) = engine_thread {
        let mut registry = state.registry.lock().unwrap();
        if let Some(run) = registry.runs.iter_mut().find(|run| run.id == run_id) {
            run.status = RunStatus::Failed;
            run.error = Some(format!("could not start: {spawn_error}"));
        }
        anyhow::bail!("could not start the run thread: {spawn_error}");
    }
    Ok(run_id)
}

/// Fires the pipeline's on_failure webhook/command, after retries are
/// exhausted. Hook failures are logged, never fatal.
fn fire_failure_hooks(state: &Arc<State>, pipeline: &Pipeline, error: &str) {
    let Some(hooks) = &pipeline.on_failure else { return };
    let env = crate::env::EnvMap::load(&state.config.project_root, None);
    if let Some(webhook) = &hooks.webhook {
        match crate::env::resolve_templates(webhook, &env) {
            Ok(url) => {
                let body = serde_json::json!({
                    "pipeline": pipeline.pipeline,
                    "error": error,
                });
                // The resolved URL may embed a secret — never log it.
                // ureq errors Display the full URL — never log them.
                match ureq::post(url.expose()).send_json(body) {
                    Ok(_) => state.log("failure webhook delivered".to_owned()),
                    Err(ureq::Error::Status(code, _)) => {
                        state.log(format!("failure webhook not delivered: status {code}"))
                    }
                    Err(ureq::Error::Transport(_)) => {
                        state.log("failure webhook not delivered: transport error".to_owned())
                    }
                }
            }
            Err(missing) => log::warn!("el serve: webhook skipped: {missing}"),
        }
    }
    if let Some(command) = &hooks.command {
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .env("ZDBT_EL_PIPELINE", &pipeline.pipeline)
            .env("ZDBT_EL_ERROR", error)
            .current_dir(&state.config.project_root)
            .status();
        match status {
            Ok(status) if status.success() => state.log("failure command ran".to_owned()),
            Ok(status) => state.log(format!("failure command exited {status}")),
            Err(error) => state.log(format!("failure command not run: {error}")),
        }
    }
}

/// The scheduler: every tick, fire any pipeline whose cron schedule has a
/// trigger time inside (previous tick, now]. Skip-if-running.
fn scheduler_loop(state: Arc<State>) {
    let mut previous = Utc::now();
    let mut warned: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        std::thread::sleep(SCHEDULER_TICK);
        let now = Utc::now();
        for path in spec::list_pipelines(&state.config.project_root.join("el")) {
            let Ok(pipeline) = spec::load_pipeline(&path) else {
                continue;
            };
            let Some(schedule_text) = &pipeline.schedule else {
                continue;
            };
            let schedule: cron::Schedule = match schedule_text.parse() {
                Ok(schedule) => schedule,
                Err(error) => {
                    log::warn!(
                        "el serve: {} has an invalid schedule {schedule_text:?}: {error}",
                        pipeline.pipeline
                    );
                    continue;
                }
            };
            let timezone: chrono_tz::Tz = match pipeline.timezone.as_deref() {
                None => chrono_tz::UTC,
                Some(name) => match name.parse() {
                    Ok(timezone) => timezone,
                    Err(_) => {
                        if warned.insert(pipeline.pipeline.clone()) {
                            state.log(format!(
                                "{} has an unknown timezone {name:?} — using UTC",
                                pipeline.pipeline
                            ));
                        }
                        chrono_tz::UTC
                    }
                },
            };
            // cron+chrono can panic across DST edges — a bad schedule must
            // not kill the scheduler thread.
            let due = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                schedule
                    .after(&previous.with_timezone(&timezone))
                    .next()
                    .map(|fire| fire.with_timezone(&Utc) <= now)
                    .unwrap_or(false)
            }))
            .unwrap_or_else(|_| {
                if warned.insert(pipeline.pipeline.clone()) {
                    state.log(format!(
                        "{}: schedule evaluation panicked around a timezone                          transition — skipped this tick",
                        pipeline.pipeline
                    ));
                }
                false
            });
            if due {
                match start_run(&state, &pipeline.pipeline, 0) {
                    Ok(run_id) => state.log(format!(
                        "schedule fired {} (run {run_id})",
                        pipeline.pipeline
                    )),
                    Err(error) => state.log(format!(
                        "schedule skipped {}: {error:#}",
                        pipeline.pipeline
                    )),
                }
            }
        }
        previous = now;
    }
}

// --------------------------------------------------------------------------
// HTTP API

#[derive(Serialize)]
struct PipelineInfo {
    name: String,
    schedule: Option<String>,
    timezone: Option<String>,
    streams: usize,
    running: bool,
}

#[derive(Serialize)]
struct RunInfo {
    id: u64,
    pipeline: String,
    status: RunStatus,
    attempt: u32,
    started_unix: u64,
    rows_written: u64,
    error: Option<String>,
}

fn run_info(run: &RunRecord) -> RunInfo {
    RunInfo {
        id: run.id,
        pipeline: run.pipeline.clone(),
        status: run.status,
        attempt: run.attempt,
        started_unix: run
            .started
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
        rows_written: run.rows_written,
        error: run.error.clone(),
    }
}

fn respond_json<T: Serialize>(request: tiny_http::Request, status: u16, body: &T) {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let response = tiny_http::Response::from_data(payload)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static header"),
        );
    let _ = request.respond(response);
}

fn authorized(state: &State, request: &tiny_http::Request) -> bool {
    let Some(expected) = &state.config.token else {
        return true;
    };
    let provided = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str().to_owned())
        .unwrap_or_default();
    let provided = provided.strip_prefix("Bearer ").unwrap_or("");
    // Compare fixed-size digests: constant time, and no signal about the
    // token's length either.
    use sha2::Digest as _;
    let expected = sha2::Sha256::digest(expected.as_bytes());
    let provided = sha2::Sha256::digest(provided.as_bytes());
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(provided.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// True when the header value names this machine: localhost forms only.
fn is_local_host_header(value: &str) -> bool {
    let host = value.rsplit_once(':').map(|(host, _)| host).unwrap_or(value);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        || value.starts_with("[::1]")
}

fn handle(state: &Arc<State>, mut request: tiny_http::Request) {
    // A token-less daemon exists only for loopback development — make the
    // browser attack surface (CSRF simple-requests, DNS rebinding) fail
    // closed: local Host required, any cross-site Origin rejected, and
    // POST bodies must declare JSON (never a no-preflight content type).
    if state.config.token.is_none() {
        let header = |name: &'static str| {
            request
                .headers()
                .iter()
                .find(move |header| header.field.equiv(name))
                .map(|header| header.value.as_str().to_owned())
        };
        let host_ok = header("Host").is_some_and(|host| is_local_host_header(&host));
        let origin_ok = match header("Origin") {
            None => true,
            Some(origin) => {
                origin == "null"
                    || url::Url::parse(&origin)
                        .ok()
                        .and_then(|origin| origin.host_str().map(str::to_owned))
                        .is_some_and(|host| is_local_host_header(&host))
            }
        };
        let content_ok = request.method().as_str() != "POST"
            || header("Content-Type")
                .is_some_and(|content| content.to_ascii_lowercase().contains("application/json"));
        if !host_ok || !origin_ok || !content_ok {
            respond_json(
                request,
                403,
                &serde_json::json!({"error": "cross-origin request refused — set                     ZDBT_EL_TOKEN for network access"}),
            );
            return;
        }
    }
    if !authorized(state, &request) {
        respond_json(
            request,
            401,
            &serde_json::json!({"error": "missing or wrong bearer token"}),
        );
        return;
    }
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let query: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    let method = request.method().clone();
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    match (method.as_str(), segments.as_slice()) {
        ("GET", ["health"]) => {
            let running = {
                let registry = state.registry.lock().unwrap();
                registry
                    .runs
                    .iter()
                    .filter(|run| run.status == RunStatus::Running)
                    .count()
            };
            let uptime = state
                .started
                .elapsed()
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            let profile = spec::load_connections(
                &state.config.project_root.join("el").join("connections.yml"),
            )
            .ok()
            .and_then(|raw| spec::active_profile(&state.config.project_root, &raw));
            respond_json(
                request,
                200,
                &serde_json::json!({
                    "ok": true,
                    "profile": profile,
                    "uptime_secs": uptime,
                    "running": running,
                    "project": state
                        .config
                        .project_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("project"),
                }),
            )
        }
        ("GET", ["logs"]) => {
            let since: u64 = query
                .get("since")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let (entries, next) = {
                let lines = state.log_lines.lock().unwrap();
                let entries: Vec<String> = lines
                    .iter()
                    .filter(|(seq, _)| *seq >= since)
                    .map(|(_, line)| line.clone())
                    .collect();
                let next = lines.last().map(|(seq, _)| seq + 1).unwrap_or(since);
                (entries, next)
            };
            respond_json(
                request,
                200,
                &serde_json::json!({"lines": entries, "next": next}),
            );
        }
        ("GET", ["pipelines"]) => {
            let mut pipelines = Vec::new();
            for path in spec::list_pipelines(&state.config.project_root.join("el")) {
                if let Ok(pipeline) = spec::load_pipeline(&path) {
                    pipelines.push(PipelineInfo {
                        running: state.is_running(&pipeline.pipeline),
                        name: pipeline.pipeline,
                        schedule: pipeline.schedule,
                        timezone: pipeline.timezone,
                        streams: pipeline.streams.len(),
                    });
                }
            }
            respond_json(request, 200, &pipelines);
        }
        ("GET", ["runs"]) => {
            let runs: Vec<RunInfo> = {
                let registry = state.registry.lock().unwrap();
                registry.runs.iter().rev().map(run_info).collect()
            };
            respond_json(request, 200, &runs);
        }
        ("POST", ["runs"]) => {
            let mut body = String::new();
            let _ = request
                .as_reader()
                .take(64 * 1024)
                .read_to_string(&mut body);
            let pipeline = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("pipeline")
                        .and_then(|name| name.as_str())
                        .map(str::to_owned)
                });
            let Some(pipeline) = pipeline else {
                respond_json(
                    request,
                    400,
                    &serde_json::json!({"error": "body must be {\"pipeline\": \"name\"}"}),
                );
                return;
            };
            match start_run(state, &pipeline, 0) {
                Ok(id) => respond_json(request, 200, &serde_json::json!({"id": id})),
                Err(error) => {
                    let message = format!("{error:#}");
                    let status = if message.contains("already running") {
                        409
                    } else if message.contains("no pipeline named") {
                        404
                    } else {
                        500
                    };
                    respond_json(request, status, &serde_json::json!({"error": message}));
                }
            }
        }
        ("POST", ["runs", id, "cancel"]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let registry = state.registry.lock().unwrap();
            match registry.runs.iter().find(|run| run.id == id) {
                Some(run) => {
                    run.cancel.cancel();
                    drop(registry);
                    respond_json(request, 200, &serde_json::json!({"ok": true}));
                }
                None => {
                    drop(registry);
                    respond_json(request, 404, &serde_json::json!({"error": "no such run"}));
                }
            }
        }
        ("GET", ["runs", id]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let registry = state.registry.lock().unwrap();
            match registry.runs.iter().find(|run| run.id == id) {
                Some(run) => {
                    let info = run_info(run);
                    drop(registry);
                    respond_json(request, 200, &info);
                }
                None => {
                    drop(registry);
                    respond_json(request, 404, &serde_json::json!({"error": "no such run"}));
                }
            }
        }
        ("GET", ["runs", id, "events"]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let since: usize = query
                .get("since")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let registry = state.registry.lock().unwrap();
            match registry.runs.iter().find(|run| run.id == id) {
                Some(run) => {
                    let events: Vec<ProgressEvent> =
                        run.events.iter().skip(since).cloned().collect();
                    let body = serde_json::json!({
                        "events": events,
                        "next": run.events.len(),
                        "done": run.status != RunStatus::Running,
                        "error": run.error,
                    });
                    drop(registry);
                    respond_json(request, 200, &body);
                }
                None => {
                    drop(registry);
                    respond_json(request, 404, &serde_json::json!({"error": "no such run"}));
                }
            }
        }
        _ => respond_json(request, 404, &serde_json::json!({"error": "no such endpoint"})),
    }
}

/// Runs the daemon until the process is killed.
pub fn serve(config: ServerConfig) -> Result<()> {
    if config.token.is_none() && !config.listen.ip().is_loopback() {
        bail!(
            "refusing to listen on {} without a token — set ZDBT_EL_TOKEN",
            config.listen
        );
    }
    if config.tls.is_none() && !config.listen.ip().is_loopback() && !config.allow_insecure_http
    {
        bail!(
            "refusing plaintext HTTP on {} — pass --tls-cert/--tls-key (PEM), or \
             --insecure-http for a private network where TLS terminates at ingress",
            config.listen
        );
    }
    let listen = config.listen;
    let state = Arc::new(State {
        config,
        registry: Mutex::default(),
        started: SystemTime::now(),
        log_lines: Mutex::default(),
        log_next: std::sync::atomic::AtomicU64::new(0),
    });
    state.log(format!("daemon started, project {}", state.config.project_root.display()));

    let scheduler_state = Arc::clone(&state);
    std::thread::Builder::new()
        .name("el-scheduler".into())
        .spawn(move || scheduler_loop(scheduler_state))
        .ok();

    let server = match &state.config.tls {
        Some((cert_path, key_path)) => {
            let certificate = std::fs::read(cert_path)
                .with_context(|| format!("reading {}", cert_path.display()))?;
            let private_key = std::fs::read(key_path)
                .with_context(|| format!("reading {}", key_path.display()))?;
            tiny_http::Server::https(
                listen,
                tiny_http::SslConfig {
                    certificate,
                    private_key,
                },
            )
            .map_err(|error| anyhow::anyhow!("binding https {listen}: {error}"))?
        }
        None => tiny_http::Server::http(listen)
            .map_err(|error| anyhow::anyhow!("binding {listen}: {error}"))?,
    };
    let scheme = if state.config.tls.is_some() { "https" } else { "http" };
    log::info!("el serve: listening on {scheme}://{listen}");
    println!(
        "el serve: listening on {scheme}://{listen} (project {})",
        state.config.project_root.display()
    );
    for request in server.incoming_requests() {
        // One thread per request: a slow client never wedges the API.
        let state = Arc::clone(&state);
        std::thread::spawn(move || handle(&state, request));
    }
    Ok(())
}

impl ServerConfig {
    /// Standard config: token from ZDBT_EL_TOKEN, defaults elsewhere.
    pub fn new(project_root: PathBuf, listen: SocketAddr) -> Self {
        Self {
            project_root,
            listen,
            token: std::env::var("ZDBT_EL_TOKEN").ok().filter(|t| !t.is_empty()),
            tls: None,
            allow_insecure_http: false,
            worker: None,
            driver: None,
            chunk_rows: 50_000,
        }
    }
}


// --------------------------------------------------------------------------
// Client side: the IDE (and tests) drive a declared remote daemon.

/// A page of a run's progress events, as polled from a remote.
#[derive(Debug, serde::Deserialize)]
pub struct EventsPage {
    pub events: Vec<ProgressEvent>,
    pub next: usize,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RemotePipeline {
    pub name: String,
    pub schedule: Option<String>,
    pub streams: usize,
    pub running: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct RemoteRun {
    pub id: u64,
    pub pipeline: String,
    pub status: String,
    pub attempt: u32,
    pub started_unix: u64,
    pub rows_written: u64,
    pub error: Option<String>,
}

pub struct RemoteClient {
    base: String,
    token: Option<crate::env::Secret>,
}

/// Refuses any remote URL a bearer token must not travel to: parsed with
/// the same URL grammar the HTTP client uses (no hand string surgery, so
/// no `http://localhost:6@evil.com` userinfo bypass), credentials in the
/// URL rejected outright, and plaintext http allowed only when the HOST
/// is genuinely loopback.
fn check_remote_url(raw: &str) -> Result<()> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| anyhow::anyhow!("invalid remote url: {error}"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("remote URLs must not embed credentials — use the token field");
    }
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let loopback = match parsed.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                None => false,
            };
            if loopback {
                Ok(())
            } else {
                bail!("remote URLs must use https (plain http is allowed for loopback only)")
            }
        }
        other => bail!("unsupported remote url scheme {other:?}"),
    }
}

impl RemoteClient {
    /// Connects to the named remote from `el/remotes.yml`. The token is
    /// resolved from the environment at call time and non-loopback URLs
    /// must be https — a credential never travels plaintext.
    pub fn connect(project_root: &std::path::Path, name: &str) -> Result<Self> {
        let remotes = spec::load_remotes(&project_root.join("el").join("remotes.yml"))
            .map_err(|error| anyhow::anyhow!("reading el/remotes.yml: {error}"))?;
        let remote = remotes
            .remotes
            .get(name)
            .with_context(|| format!("no remote named {name:?} in el/remotes.yml"))?;
        check_remote_url(&remote.url)
            .map_err(|error| anyhow::anyhow!("remote {name:?}: {error:#}"))?;
        let env = crate::env::EnvMap::load(project_root, None);
        let token = match &remote.token {
            Some(template) => Some(
                crate::env::resolve_templates(template, &env)
                    .map_err(|missing| anyhow::anyhow!("{missing}"))?,
            ),
            None => None,
        };
        Ok(Self {
            base: remote.url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// Direct construction (tests, ad-hoc URLs). Same https rule.
    pub fn direct(url: &str, token: Option<crate::env::Secret>) -> Result<Self> {
        check_remote_url(url)?;
        Ok(Self {
            base: url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        let mut request = ureq::request(method, &format!("{}{path}", self.base))
            .timeout(Duration::from_secs(20));
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {}", token.expose()));
        }
        request
    }

    fn read_json<T: serde::de::DeserializeOwned>(
        response: std::result::Result<ureq::Response, ureq::Error>,
    ) -> Result<T> {
        match response {
            Ok(response) => response.into_json().context("decoding remote reply"),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response
                    .into_json::<serde_json::Value>()
                    .ok()
                    .and_then(|value| {
                        value.get("error").and_then(|e| e.as_str()).map(str::to_owned)
                    })
                    .unwrap_or_default();
                bail!("remote returned {code}: {detail}")
            }
            Err(error) => bail!("remote unreachable: {error}"),
        }
    }

    pub fn pipelines(&self) -> Result<Vec<RemotePipeline>> {
        Self::read_json(self.request("GET", "/pipelines").call())
    }

    pub fn runs(&self) -> Result<Vec<RemoteRun>> {
        Self::read_json(self.request("GET", "/runs").call())
    }

    pub fn start_run(&self, pipeline: &str) -> Result<u64> {
        let value: serde_json::Value = Self::read_json(
            self.request("POST", "/runs")
                .send_json(serde_json::json!({"pipeline": pipeline})),
        )?;
        value
            .get("id")
            .and_then(|id| id.as_u64())
            .context("remote reply had no run id")
    }

    pub fn cancel(&self, run_id: u64) -> Result<()> {
        let _: serde_json::Value = Self::read_json(
            self.request("POST", &format!("/runs/{run_id}/cancel"))
                .send_json(serde_json::json!({})),
        )?;
        Ok(())
    }

    pub fn health(&self) -> Result<serde_json::Value> {
        Self::read_json(self.request("GET", "/health").call())
    }

    /// Daemon log lines at and after `since`; returns (lines, next).
    pub fn logs(&self, since: u64) -> Result<(Vec<String>, u64)> {
        let value: serde_json::Value =
            Self::read_json(self.request("GET", &format!("/logs?since={since}")).call())?;
        let lines = value
            .get("lines")
            .and_then(|lines| lines.as_array())
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(|line| line.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let next = value.get("next").and_then(|next| next.as_u64()).unwrap_or(since);
        Ok((lines, next))
    }

    pub fn events(&self, run_id: u64, since: usize) -> Result<EventsPage> {
        Self::read_json(
            self.request("GET", &format!("/runs/{run_id}/events?since={since}"))
                .call(),
        )
    }
}
