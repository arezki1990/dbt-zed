//! The EL console — the plugin's bottom-dock surface. Two views: Runs
//! (hosting [`super::run_view::ElRunView`]) and Query (ad-hoc SQL against
//! any EL connection, through the on-demand worker). Mapping-editor
//! previews overlay either view and return with Back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use editor::Editor;
use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::run_view::ElRunView;
use crate::ToggleElRunsFocus;

const QUERY_ROW_CAP: usize = 500;

#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Runs,
    Query,
    Remote,
}

pub struct ElRunsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    run_view: Entity<ElRunView>,
    surface: Surface,
    /// A preview table (stream sample or failed casts) — overlays the
    /// current surface until Back.
    preview: Option<PreviewTable>,
    preview_scroll: UniformListScrollHandle,
    // Query surface state.
    sql: Entity<Editor>,
    /// (name, kind) — names only, credential posture as everywhere.
    connections: Vec<(SharedString, SharedString)>,
    selected: Option<usize>,
    root: Option<PathBuf>,
    running: bool,
    result: Option<PreviewTable>,
    result_scroll: UniformListScrollHandle,
    elapsed: Option<Duration>,
    query_error: Option<SharedString>,
    _query: Task<()>,
    // Remote surface state (from el/remotes.yml).
    remotes: Vec<SharedString>,
    selected_remote: Option<usize>,
    remote_pipelines: Vec<el_engine::server::RemotePipeline>,
    remote_runs: Vec<el_engine::server::RemoteRun>,
    remote_error: Option<SharedString>,
    /// A failed Run/Cancel action — kept visible until the next action
    /// or remote switch (the 2s poll must not wipe it).
    remote_action_error: Option<SharedString>,
    /// What the server said about itself, kept as facts rather than a
    /// sentence so the uptime ticks between pushes instead of ageing.
    /// None = not connected.
    remote_status: Option<RemoteStatus>,
    remote_logs: Vec<SharedString>,
    /// Logs live in a read-only editor so they can be selected/copied.
    remote_log_editor: Entity<Editor>,
    remote_logs_dirty: bool,
    remote_log_next: u64,
    remote_show_logs: bool,
    /// A pipeline opened in the detail view; None = overview.
    remote_detail: Option<SharedString>,
    /// The detail was opened on a pipeline this server has not listed yet
    /// (a just-deployed one) — "not fetched" must not read as "gone".
    remote_detail_pending: bool,
    /// A run opened in the run-detail view (its per-stream breakdown).
    remote_run_detail: Option<u64>,
    /// Progress per run. The socket pushes it for every run in flight, so
    /// this is no longer "the opened run's events" — opening a run that
    /// finished before we attached fetches its history from the API.
    remote_run_events: HashMap<u64, Vec<el_engine::ProgressEvent>>,
    /// The opened run's error as reported by its events page — keeps the
    /// message on the detail view after the run drops out of `/runs`.
    remote_run_error: Option<SharedString>,
    /// The run whose history has been asked for. Holds across the fetch
    /// so a busy socket cannot restart it on every pushed message, and
    /// across a failure so it does not retry forever — closing the detail
    /// clears it, which is how a person asks again.
    remote_history_for: Option<u64>,
    /// Height of the SQL editor in the Query tab; its splitter drags it.
    query_split: f32,
    query_split_drag: Option<(f32, f32)>,
    /// Height of the pipelines section; the splitter drags it.
    remote_split: f32,
    /// (pointer y at drag start, split at drag start).
    remote_split_drag: Option<(f32, f32)>,
    remote_epoch: u64,
    /// The live socket's applying task, and the polling fallback for a
    /// server too old to have one. Only ever one of them is doing work.
    _remote_feed: Task<()>,
    _remote_poll: Task<()>,
    /// A one-shot fetch of a finished run's event history.
    _remote_history: Task<()>,
}

/// The remote's self-description, as the socket's snapshot (or `/health`)
/// reports it.
struct RemoteStatus {
    started_unix: u64,
    profile: Option<SharedString>,
    /// True while the WebSocket is attached; false while falling back to
    /// polling, so the difference is visible rather than mysterious.
    live: bool,
}

/// What the watch thread hands the panel. The thread is a plain OS thread
/// because [`el_engine::watch::WatchClient`] blocks; it talks to the
/// panel over a channel rather than blocking a background executor slot
/// for the life of the connection.
enum RemoteFeed {
    /// The socket is up. A snapshot follows immediately.
    Attached(SharedString),
    Message(Box<el_engine::watch::ServerMessage>),
    /// A reconnect failed — the server is actually unreachable, not just
    /// cycling a session.
    Unreachable(SharedString),
    /// This daemon predates the socket. Poll it instead, quietly.
    Unsupported,
}

/// Folds one pushed run into the table, which reads newest first. The
/// message is absolute, so a run already held is simply replaced — that
/// is what makes the snapshot/stream overlap harmless.
fn merge_run(runs: &mut Vec<el_engine::server::RemoteRun>, run: el_engine::server::RemoteRun) {
    match runs.iter().position(|held| held.id == run.id) {
        Some(ix) => runs[ix] = run,
        None => runs.insert(0, run),
    }
}

/// Places one pushed progress event by the index the daemon gave it.
///
/// Three cases, and only one of them appends. An index already held is a
/// repeat (the snapshot and the stream overlap by design) and is dropped.
/// An index beyond the end is a *gap*: the rest of that list can no
/// longer fold into honest per-stream rows, so it is discarded and the
/// detail view refetches the run's history. And a list is never started
/// part-way through, which would render as a run that skipped its own
/// beginning.
fn place_run_event(
    events: &mut HashMap<u64, Vec<el_engine::ProgressEvent>>,
    run_id: u64,
    index: usize,
    event: el_engine::ProgressEvent,
) {
    use std::collections::hash_map::Entry;
    match events.entry(run_id) {
        Entry::Occupied(mut held) => {
            let events = held.get_mut();
            if index == events.len() {
                events.push(event);
            } else if index > events.len() {
                held.remove();
            }
        }
        Entry::Vacant(slot) => {
            if index == 0 {
                slot.insert(vec![event]);
            }
        }
    }
}

/// How many daemon log lines the panel keeps.
const REMOTE_LOG_CAP: usize = 400;

/// Appends one daemon log line, ignoring anything already held. Returns
/// whether the buffer changed — the log editor is only rebuilt when it
/// did.
fn append_log(logs: &mut Vec<SharedString>, next: &mut u64, seq: u64, line: String) -> bool {
    if seq < *next {
        return false;
    }
    *next = seq + 1;
    logs.push(line.into());
    let overflow = logs.len().saturating_sub(REMOTE_LOG_CAP);
    if overflow > 0 {
        logs.drain(..overflow);
    }
    true
}

/// Merges a snapshot's log tail into what the panel already holds.
///
/// The lines carry no sequences of their own — the tail simply ends at
/// `log_next` — so each line's place is arithmetic, and [`append_log`]
/// then drops the ones already held. Appending rather than replacing is
/// what keeps a reconnect (the server retires sessions on a timer) from
/// shortening a log somebody is reading.
fn merge_log_tail(
    logs: &mut Vec<SharedString>,
    next: &mut u64,
    lines: Vec<String>,
    log_next: u64,
) -> bool {
    let first_seq = log_next.saturating_sub(lines.len() as u64);
    let mut changed = false;
    for (offset, line) in lines.into_iter().enumerate() {
        changed |= append_log(logs, next, first_seq + offset as u64, line);
    }
    changed
}

/// Holds one live connection open, reconnecting for as long as anyone is
/// listening. Returns when the receiver is dropped — which is what a
/// closed tab, a switched remote or a dropped panel looks like from here.
fn watch_remote(
    root: PathBuf,
    name: String,
    sender: futures::channel::mpsc::UnboundedSender<RemoteFeed>,
) {
    use el_engine::watch::{ServerMessage, WatchClient, WatchRefused};

    let mut backoff = Duration::from_secs(1);
    loop {
        if sender.is_closed() {
            return;
        }
        let mut watch = match WatchClient::connect(&root, &name) {
            Ok(watch) => watch,
            Err(WatchRefused::Unsupported) => {
                let _ = sender.unbounded_send(RemoteFeed::Unsupported);
                return;
            }
            Err(WatchRefused::Failed(error)) => {
                let message = format!("{error:#}");
                if sender
                    .unbounded_send(RemoteFeed::Unreachable(message.into()))
                    .is_err()
                {
                    return;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        let opened = Instant::now();
        if sender
            .unbounded_send(RemoteFeed::Attached(name.clone().into()))
            .is_err()
        {
            return;
        }
        // The server retires a session on a timer and expects us back; a
        // reconnect on its say-so is not a fault and must not surface as
        // one. Any other drop just reconnects too — an error is only
        // worth reporting once the *reconnect* fails.
        let mut expected_close = false;
        loop {
            if sender.is_closed() {
                return;
            }
            match watch.next() {
                Ok(Some(message)) => {
                    expected_close |= matches!(message, ServerMessage::Closing { .. });
                    if sender
                        .unbounded_send(RemoteFeed::Message(Box::new(message)))
                        .is_err()
                    {
                        return;
                    }
                }
                // The poll window passed quietly — the chance to notice
                // nobody is listening any more.
                Ok(None) => {}
                Err(error) => {
                    if !expected_close {
                        log::debug!("el: watch socket for {name} dropped: {error:#}");
                    }
                    break;
                }
            }
        }
        // A connection that held is a healthy one, however it ended — the
        // server retires sessions on a timer and expects us straight back.
        // One that died on arrival is a server in trouble, and reconnecting
        // at once would spin a core on it.
        if opened.elapsed() >= Duration::from_secs(2) {
            backoff = Duration::from_secs(1);
        } else {
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

pub struct PreviewTable {
    pub title: SharedString,
    pub columns: std::sync::Arc<Vec<SharedString>>,
    pub rows: std::sync::Arc<Vec<Vec<SharedString>>>,
}

/// The failing-columns grid: one row per column with its count and
/// sample values. Shared by the mapping preview and the run badges so
/// a failure looks the same wherever it is opened.
pub(crate) fn failures_table(
    failures: &[el_engine::ColumnFailures],
) -> (Vec<SharedString>, Vec<Vec<SharedString>>) {
    (
        vec!["column".into(), "failed".into(), "sample values".into()],
        failures
            .iter()
            .map(|failure| {
                vec![
                    failure.column.clone().into(),
                    failure.count.to_string().into(),
                    failure.samples.join(" · ").into(),
                ]
            })
            .collect(),
    )
}

/// Title of the failing-columns grid for `stream` — the same name from
/// the mapping sidebar's Failed casts through both run badges.
pub(crate) fn failures_title(stream: &str) -> SharedString {
    format!("{stream} · failed casts").into()
}

/// One remote stream's state, folded from the run's event log.
#[derive(Default)]
pub(crate) struct StreamAgg {
    pub(crate) phase: Option<String>,
    pub(crate) read: u64,
    pub(crate) written: u64,
    pub(crate) casts: u64,
    pub(crate) failures: Vec<el_engine::ColumnFailures>,
    pub(crate) error: Option<String>,
    pub(crate) done: bool,
}

/// Replays a run's events into per-stream rows, in the order the run
/// announced them.
pub(crate) fn fold_stream_events(
    events: &[el_engine::ProgressEvent],
) -> (Vec<String>, std::collections::HashMap<String, StreamAgg>) {
    use el_engine::ProgressEvent as E;
    let mut order: Vec<String> = Vec::new();
    let mut agg: std::collections::HashMap<String, StreamAgg> = Default::default();
    for event in events {
        match event {
            E::RunStarted { streams, .. } => {
                for stream in streams {
                    if !order.contains(stream) {
                        order.push(stream.clone());
                        agg.entry(stream.clone()).or_default();
                    }
                }
            }
            E::StreamStarted { stream } => {
                agg.entry(stream.clone()).or_default().phase = Some("connect".to_owned());
            }
            E::Chunk {
                stream,
                phase,
                rows_read,
                rows_written,
                cast_failures,
            } => {
                let entry = agg.entry(stream.clone()).or_default();
                entry.phase = Some(format!("{phase:?}").to_lowercase());
                entry.read = *rows_read;
                entry.written = *rows_written;
                entry.casts = *cast_failures;
            }
            E::StreamFinished {
                stream,
                rows_read,
                rows_written,
                cast_failures,
                column_failures,
            } => {
                let entry = agg.entry(stream.clone()).or_default();
                entry.phase = None;
                entry.read = *rows_read;
                entry.written = *rows_written;
                entry.casts = *cast_failures;
                entry.failures = column_failures.clone();
                entry.done = true;
            }
            E::StreamFailed { stream, error } => {
                let entry = agg.entry(stream.clone()).or_default();
                entry.phase = None;
                entry.error = Some(error.clone());
            }
            E::RunFinished { .. } => {}
        }
    }
    (order, agg)
}

impl ElRunsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            Self::new(workspace, window, cx)
        })
    }

    pub fn new(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        let languages = workspace.project().read(cx).languages().clone();
        let sql = cx.new(|cx| {
            let mut editor = Editor::multi_line(window, cx);
            editor.set_placeholder_text("SELECT …", window, cx);
            editor.set_show_gutter(false, cx);
            editor
        });
        let remote_log_editor = cx.new(|cx| {
            let mut editor = Editor::multi_line(window, cx);
            editor.set_read_only(true);
            editor
        });
        // Give the query editor SQL syntax highlighting.
        {
            let sql = sql.clone();
            cx.spawn(async move |_, cx| {
                // Plain SQL grammar first (what the dbt results grid uses);
                // the Jinja-host dbt language as a fallback.
                let language = match languages.language_for_name("SQL (dbt)").await {
                    Ok(language) => language,
                    Err(_) => match languages.language_for_name("dbt SQL").await {
                        Ok(language) => language,
                        Err(_) => return,
                    },
                };
                sql.update(cx, |editor, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                        buffer.update(cx, |buffer, cx| buffer.set_language(Some(language), cx));
                    }
                });
            })
            .detach();
        }
        cx.new(|cx| {
            let run_view = cx.new(|cx| ElRunView::new(workspace_handle.clone(), cx));
            Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                run_view,
                surface: Surface::Runs,
                preview: None,
                preview_scroll: UniformListScrollHandle::new(),
                sql,
                connections: Vec::new(),
                selected: None,
                root: None,
                running: false,
                result: None,
                result_scroll: UniformListScrollHandle::new(),
                elapsed: None,
                query_error: None,
                _query: Task::ready(()),
                remotes: Vec::new(),
                selected_remote: None,
                remote_pipelines: Vec::new(),
                remote_runs: Vec::new(),
                remote_error: None,
                remote_action_error: None,
                remote_status: None,
                remote_logs: Vec::new(),
                remote_log_editor,
                remote_logs_dirty: false,
                remote_log_next: 0,
                remote_show_logs: false,
                remote_detail: None,
                remote_detail_pending: false,
                remote_run_detail: None,
                remote_run_events: HashMap::new(),
                remote_run_error: None,
                remote_history_for: None,
                query_split: 110.,
                query_split_drag: None,
                remote_split: 170.,
                remote_split_drag: None,
                remote_epoch: 0,
                _remote_feed: Task::ready(()),
                _remote_poll: Task::ready(()),
                _remote_history: Task::ready(()),
            }
        })
    }

    pub fn run_view(&self) -> Entity<ElRunView> {
        self.run_view.clone()
    }

    /// The last run's failing columns for `stream` of `pipeline`: the
    /// local run first, else the remote run whose detail is open. Reads
    /// only — safe from a canvas that holds no workspace lease.
    pub fn last_stream_failures(
        &self,
        pipeline: &str,
        stream: &str,
        cx: &App,
    ) -> Vec<el_engine::ColumnFailures> {
        let local = self.run_view.read(cx).stream_failures(pipeline, stream);
        if !local.is_empty() {
            return local;
        }
        let Some(run_id) = self.remote_run_detail else {
            return Vec::new();
        };
        let same_pipeline = self
            .remote_runs
            .iter()
            .any(|run| run.id == run_id && run.pipeline == pipeline);
        if !same_pipeline {
            return Vec::new();
        }
        let (_, mut agg) = fold_stream_events(self.run_events(run_id));
        agg.remove(stream)
            .map(|entry| entry.failures)
            .unwrap_or_default()
    }

    pub fn show_preview(
        &mut self,
        title: SharedString,
        columns: Vec<SharedString>,
        rows: Vec<Vec<SharedString>>,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(PreviewTable {
            title,
            columns: std::sync::Arc::new(columns),
            rows: std::sync::Arc::new(rows),
        });
        cx.notify();
    }

    pub fn show_runs(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        cx.notify();
    }

    /// The EL panel's choice of connection for ad-hoc queries.
    pub fn select_connection(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.refresh_connections(cx);
        if let Some(ix) = self.connections.iter().position(|(n, _)| *n == name) {
            self.selected = Some(ix);
        }
        cx.notify();
    }

    /// The sidebar's click-a-server entry point: Remote tab, that server.
    pub fn show_remote(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.preview = None;
        self.surface = Surface::Remote;
        self.refresh_connections(cx);
        if let Some(ix) = self.remotes.iter().position(|remote| *remote == name) {
            if self.selected_remote != Some(ix) {
                self.selected_remote = Some(ix);
                self.remote_pipelines.clear();
                self.remote_runs.clear();
            }
        }
        self.remote_detail = None;
        self.remote_detail_pending = false;
        self.remote_run_detail = None;
        self.start_remote_feed(cx);
        cx.notify();
    }

    /// After a deploy: Remote tab, that server, opened on that pipeline.
    /// The first poll (which lists the just-deployed copy) keeps the
    /// detail open, so the view fills in rather than bouncing to the
    /// overview. Until it lands the pipeline is simply not listed yet —
    /// the detail says so instead of claiming it is gone.
    pub fn show_remote_pipeline(
        &mut self,
        remote: SharedString,
        pipeline: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.show_remote(remote, cx);
        self.remote_detail = Some(pipeline);
        self.remote_detail_pending = true;
        self.remote_run_detail = None;
        cx.notify();
    }

    /// A profile switch changed what every connection name means: drop
    /// the old environment's query state and re-read the resolved set.
    pub fn profile_changed(&mut self, cx: &mut Context<Self>) {
        self.remote_detail = None;
        self.remote_run_detail = None;
        self.result = None;
        self.query_error = None;
        self.elapsed = None;
        self.refresh_connections(cx);
        if self.surface == Surface::Remote {
            self.start_remote_feed(cx);
        }
        cx.notify();
    }

    /// The explorer's click-a-table entry point: switches to Query,
    /// selects the connection, seeds the SQL, and runs it.
    pub fn show_query_for_table(
        &mut self,
        connection: SharedString,
        schema: &str,
        table: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = None;
        self.surface = Surface::Query;
        self.refresh_connections(cx);
        self.selected = self
            .connections
            .iter()
            .position(|(name, _)| *name == connection)
            .or(self.selected);
        let quote = |ident: &str| format!("\"{}\"", ident.replace('"', "\"\""));
        // Oracle has no LIMIT; ROWNUM works on every version (FETCH
        // FIRST only from 12c, and the thick driver reaches 10g / 11g).
        let kind = self
            .selected
            .and_then(|ix| self.connections.get(ix))
            .map(|(_, kind)| kind.as_ref())
            .unwrap_or("");
        let cap = if kind == "oracle" {
            "WHERE ROWNUM <= 200"
        } else {
            "LIMIT 200"
        };
        let sql = format!("SELECT * FROM {}.{} {cap}", quote(schema), quote(table));
        self.sql.update(cx, |editor, cx| {
            editor.set_text(sql, window, cx);
        });
        self.run_query(cx);
    }

    fn refresh_connections(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone().or_else(|| {
            self.workspace
                .upgrade()
                .and_then(|workspace| super::discover_el_root(workspace.read(cx), cx))
        });
        self.root = root.clone();
        let Some(root) = root else { return };
        self.connections = el_engine::spec::load_active_connections(&root)
            .map(|(connections, _)| {
                connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        (name.clone().into(), connection.kind().to_owned().into())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self
            .selected
            .map_or(true, |ix| ix >= self.connections.len())
        {
            self.selected = (!self.connections.is_empty()).then_some(0);
        }
        self.remotes = el_engine::spec::load_remotes(&super::el_dir(&root).join("remotes.yml"))
            .map(|remotes| {
                remotes
                    .remotes
                    .keys()
                    .map(|name| name.clone().into())
                    .collect()
            })
            .unwrap_or_default();
        if self
            .selected_remote
            .map_or(true, |ix| ix >= self.remotes.len())
        {
            self.selected_remote = (!self.remotes.is_empty()).then_some(0);
        }
    }

    /// The remote currently selected, as (project root, remote name).
    fn remote_target(&self) -> Option<(PathBuf, String)> {
        let root = self.root.clone()?;
        let name = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix))
            .map(|name| name.to_string())?;
        Some((root, name))
    }

    /// Attaches to the server's live status socket, and falls back to the
    /// old poll loop when the server is too old to have one. This is the
    /// entry point for every "show me this remote" path; nothing else
    /// should reach for the poller directly.
    fn start_remote_feed(&mut self, cx: &mut Context<Self>) {
        self.remote_epoch += 1;
        self.remote_status = None;
        self.remote_logs.clear();
        self.remote_log_next = 0;
        let epoch = self.remote_epoch;
        let Some((root, name)) = self.remote_target() else {
            return;
        };

        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        // A real thread, not a background_spawn: `next` blocks, and this
        // connection lives for as long as the tab is open. Dropping the
        // task below drops the receiver, which is how the thread is told
        // to stop — it notices within one poll window.
        if let Err(error) = std::thread::Builder::new()
            .name("el-watch".to_owned())
            .spawn(move || watch_remote(root, name, sender))
        {
            log::warn!("el: no thread for the watch socket ({error}) — polling instead");
            self.start_remote_poll(cx);
            return;
        }

        self._remote_feed = cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(update) = receiver.next().await {
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.remote_epoch != epoch {
                            return false;
                        }
                        match update {
                            RemoteFeed::Attached(profile) => {
                                this.remote_error = None;
                                log::debug!("el: watching {profile} live");
                            }
                            RemoteFeed::Message(message) => {
                                this.apply_watch(*message);
                                // A gap discards the opened run's events
                                // (see `place_run_event`); fetch them
                                // rather than draw an empty breakdown.
                                if let Some(run_id) = this.remote_run_detail {
                                    this.ensure_run_events(run_id, cx);
                                }
                            }
                            RemoteFeed::Unreachable(error) => {
                                if let Some(status) = &mut this.remote_status {
                                    status.live = false;
                                }
                                this.remote_error = Some(error);
                            }
                            RemoteFeed::Unsupported => {
                                // Not worth a word to the operator: the
                                // server simply predates the socket.
                                this.start_remote_poll(cx);
                                return false;
                            }
                        }
                        cx.notify();
                        this.surface == Surface::Remote
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        });
    }

    /// Folds one pushed message into the panel. Every message is either
    /// absolute or indexed, so applying one twice is harmless — which is
    /// what makes the snapshot/stream overlap safe.
    fn apply_watch(&mut self, message: el_engine::watch::ServerMessage) {
        use el_engine::watch::ServerMessage as Message;
        match message {
            Message::Snapshot(snapshot) => {
                self.remote_status = Some(RemoteStatus {
                    started_unix: snapshot.started_unix,
                    profile: snapshot.profile.map(SharedString::from),
                    live: true,
                });
                self.remote_pipelines = snapshot.pipelines;
                self.remote_runs = snapshot.runs;
                self.remote_error = None;
                // The server has now said what it holds.
                self.remote_detail_pending = false;
                // Keep the opened run's history (the socket does not
                // carry it) and take the server's word for everything in
                // flight. Anything else is dropped: this is what bounds
                // the map across a long-lived session.
                let opened = self.remote_run_detail;
                self.remote_run_events
                    .retain(|run_id, _| Some(*run_id) == opened);
                for run in snapshot.live_events {
                    self.remote_run_events.insert(run.run_id, run.events);
                }
                self.remote_logs_dirty |= merge_log_tail(
                    &mut self.remote_logs,
                    &mut self.remote_log_next,
                    snapshot.logs,
                    snapshot.log_next,
                );
            }
            Message::Run { run } => merge_run(&mut self.remote_runs, run),
            Message::RunEvent {
                run_id,
                index,
                event,
            } => place_run_event(&mut self.remote_run_events, run_id, index, event),
            Message::Pipelines { pipelines } => {
                self.remote_pipelines = pipelines;
                self.remote_detail_pending = false;
            }
            Message::Log { seq, line } => {
                self.remote_logs_dirty |=
                    append_log(&mut self.remote_logs, &mut self.remote_log_next, seq, line);
            }
            // Proof of life, the opening handshake, and the two frames
            // that only tell the client what is about to arrive anyway.
            Message::Hello { .. }
            | Message::Heartbeat { .. }
            | Message::Resync
            | Message::Closing { .. }
            | Message::Unknown => {}
        }
    }

    /// The opened run's events. A run in flight arrives on the socket; one
    /// that finished before we attached is history the socket does not
    /// carry, so fetch it once from the JSON API.
    fn ensure_run_events(&mut self, run_id: u64, cx: &mut Context<Self>) {
        if self.remote_run_events.contains_key(&run_id) || self.remote_history_for == Some(run_id) {
            return;
        }
        let Some((root, name)) = self.remote_target() else {
            return;
        };
        self.remote_history_for = Some(run_id);
        self._remote_history = cx.spawn(async move |this, cx| {
            let page = cx
                .background_spawn(async move {
                    let client = el_engine::server::RemoteClient::connect(&root, &name)?;
                    client.events(run_id, 0)
                })
                .await;
            this.update(cx, |this, cx| {
                match page {
                    Ok(page) => {
                        if let Some(error) = page.error {
                            this.remote_run_error = Some(error.into());
                        }
                        this.remote_run_events.insert(run_id, page.events);
                    }
                    Err(error) => this.remote_run_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Whether the status socket is attached, as opposed to falling back
    /// to polling or not being connected at all.
    fn remote_is_live(&self) -> bool {
        self.remote_status
            .as_ref()
            .is_some_and(|status| status.live)
    }

    /// One run's progress, empty when neither the socket nor a history
    /// fetch has produced it yet.
    fn run_events(&self, run_id: u64) -> &[el_engine::ProgressEvent] {
        self.remote_run_events
            .get(&run_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The connected line in the toolbar, built at render time so the
    /// uptime is the real one rather than the one the last push carried.
    fn remote_status_label(&self) -> Option<SharedString> {
        let status = self.remote_status.as_ref()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(0);
        let uptime = now.saturating_sub(status.started_unix);
        let uptime = if uptime >= 3600 {
            format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
        } else if uptime >= 60 {
            format!("{}m", uptime / 60)
        } else {
            format!("{uptime}s")
        };
        let running = self
            .remote_runs
            .iter()
            .filter(|run| run.status == "running")
            .count();
        let profile = status
            .profile
            .as_ref()
            .map(|name| format!(", profile {name}"))
            .unwrap_or_default();
        let how = if status.live { "live" } else { "polling" };
        Some(format!("connected ({how}) — up {uptime}, {running} running{profile}").into())
    }

    /// The polling fallback, for a server that predates the live socket:
    /// fetch pipelines + runs now, then every two seconds while the
    /// Remote surface stays visible.
    fn start_remote_poll(&mut self, cx: &mut Context<Self>) {
        self.remote_epoch += 1;
        self.remote_status = None;
        self.remote_logs.clear();
        self.remote_log_next = 0;
        let epoch = self.remote_epoch;
        let Some((root, name)) = self.remote_target() else {
            return;
        };
        self._remote_poll = cx.spawn(async move |this, cx| {
            let mut log_cursor = 0u64;
            loop {
                let root = root.clone();
                let name = name.clone();
                let since = log_cursor;
                let watched_run = this
                    .read_with(cx, |this, _| {
                        this.remote_run_detail.map(|id| {
                            let held = this.remote_run_events.get(&id).map(Vec::len).unwrap_or(0);
                            (id, held)
                        })
                    })
                    .ok()
                    .flatten();
                let fetch = cx
                    .background_spawn(async move {
                        let client = el_engine::server::RemoteClient::connect(&root, &name)?;
                        let pipelines = client.pipelines()?;
                        let runs = client.runs()?;
                        let health = client.health().ok();
                        let logs = client.logs(since).ok();
                        let run_events = match watched_run {
                            Some((run_id, cursor)) => client
                                .events(run_id, cursor)
                                .ok()
                                .map(|page| (run_id, page)),
                            None => None,
                        };
                        anyhow::Ok((pipelines, runs, health, logs, run_events))
                    })
                    .await;
                if let Ok((_, _, _, Some((_, next)), _)) = &fetch {
                    log_cursor = *next;
                }
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.remote_epoch != epoch {
                            return false;
                        }
                        match fetch {
                            Ok((pipelines, runs, health, logs, run_events)) => {
                                if let Some((run_id, page)) = run_events {
                                    if this.remote_run_detail == Some(run_id) {
                                        this.remote_run_events
                                            .entry(run_id)
                                            .or_default()
                                            .extend(page.events);
                                        if let Some(error) = page.error {
                                            this.remote_run_error = Some(error.into());
                                        }
                                    }
                                }
                                this.remote_pipelines = pipelines;
                                this.remote_runs = runs;
                                this.remote_error = None;
                                // The server has now said what it holds.
                                this.remote_detail_pending = false;
                                this.remote_status = health.map(|value| {
                                    let now = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|since| since.as_secs())
                                        .unwrap_or(0);
                                    let uptime = value
                                        .get("uptime_secs")
                                        .and_then(|secs| secs.as_u64())
                                        .unwrap_or(0);
                                    RemoteStatus {
                                        started_unix: now.saturating_sub(uptime),
                                        profile: value
                                            .get("profile")
                                            .and_then(|name| name.as_str())
                                            .map(SharedString::from),
                                        live: false,
                                    }
                                });
                                if let Some((lines, next)) = logs {
                                    this.remote_log_next = next;
                                    if !lines.is_empty() {
                                        this.remote_logs_dirty = true;
                                    }
                                    this.remote_logs
                                        .extend(lines.into_iter().map(SharedString::from));
                                    let overflow =
                                        this.remote_logs.len().saturating_sub(REMOTE_LOG_CAP);
                                    if overflow > 0 {
                                        this.remote_logs.drain(..overflow);
                                    }
                                }
                            }
                            Err(error) => {
                                this.remote_status = None;
                                this.remote_error = Some(format!("{error:#}").into());
                            }
                        }
                        cx.notify();
                        this.surface == Surface::Remote
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
    }

    /// Fire-and-refresh action against the selected remote.
    fn remote_action(
        &mut self,
        action: impl FnOnce(&el_engine::server::RemoteClient) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(name) = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix))
            .map(|name| name.to_string())
        else {
            return;
        };
        self.remote_action_error = None;
        let task = cx.background_spawn(async move {
            let client = el_engine::server::RemoteClient::connect(&root, &name)?;
            action(&client)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    // A live socket has already been told what happened;
                    // tearing it down to reconnect would only lose the
                    // feed for a moment. A poller has to be kicked, or the
                    // result waits for its next tick.
                    Ok(()) if this.remote_is_live() => {}
                    Ok(()) => this.start_remote_feed(cx),
                    Err(error) => {
                        this.remote_action_error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_query(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.query_error = None;
        let Some(root) = self.root.clone() else {
            self.query_error = Some(
                "No EL project in this workspace — open one or run el: initialize workspace."
                    .into(),
            );
            cx.notify();
            return;
        };
        let Some((name, _)) = self.selected.and_then(|ix| self.connections.get(ix)) else {
            self.query_error = Some("Pick a connection first.".into());
            cx.notify();
            return;
        };
        let sql = self.sql.read(cx).text(cx).trim().to_owned();
        if sql.is_empty() {
            self.query_error =
                Some("Write a query first — or click a table in the EL panel.".into());
            cx.notify();
            return;
        }
        let connection_name = name.to_string();
        self.running = true;
        self.result = None;
        self.elapsed = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            let started = std::time::Instant::now();
            let result = super::run_query(&root, &connection_name, &sql, QUERY_ROW_CAP)?;
            anyhow::Ok((result, started.elapsed()))
        });
        self._query = cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.running = false;
                match outcome {
                    Ok((result, elapsed)) => {
                        this.elapsed = Some(elapsed);
                        this.result = Some(PreviewTable {
                            title: "query".into(),
                            columns: std::sync::Arc::new(
                                result.columns.into_iter().map(Into::into).collect(),
                            ),
                            rows: std::sync::Arc::new(
                                result
                                    .rows
                                    .into_iter()
                                    .map(|row| row.into_iter().map(Into::into).collect())
                                    .collect(),
                            ),
                        });
                    }
                    Err(error) => this.query_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn render_query(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        // The connection comes from the EL panel (expand one, or click a
        // table); here it is only stated, not chosen.
        let target: gpui::AnyElement = match self.selected.and_then(|ix| self.connections.get(ix)) {
            Some((name, kind)) => h_flex()
                .gap_1()
                .items_center()
                .child(Label::new("on").size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    Label::new(name.clone())
                        .size(LabelSize::Default)
                        .color(Color::Accent),
                )
                .child(
                    Label::new(kind.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            None => Label::new("Pick a connection in the EL panel")
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element(),
        };

        let status: Option<SharedString> = if self.running {
            Some("Running…".into())
        } else if let (Some(result), Some(elapsed)) = (&self.result, self.elapsed) {
            let count = result.rows.len();
            let capped = if count >= QUERY_ROW_CAP {
                format!(" (first {QUERY_ROW_CAP})")
            } else {
                String::new()
            };
            Some(format!("{count} rows{capped} in {:.2}s", elapsed.as_secs_f64()).into())
        } else {
            None
        };

        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .child(target)
            .child(div().flex_1())
            .children(status.map(|status| {
                Label::new(status)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
            }))
            .child(
                Button::new("el-query-run", "Run query")
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .disabled(self.running)
                    .on_click(cx.listener(|this, _, _, cx| this.run_query(cx))),
            );

        let dragging = self.query_split_drag.is_some();
        let editor = div().w_full().px_1().child(
            div()
                .w_full()
                .h(px(self.query_split))
                .p_1()
                .rounded_sm()
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .child(self.sql.clone()),
        );
        let splitter = div()
            .id("el-query-split")
            .w_full()
            .h(px(5.))
            .mt_1()
            .flex_shrink_0()
            .cursor(gpui::CursorStyle::ResizeRow)
            .border_t_1()
            .border_color(if dragging {
                colors.border_focused
            } else {
                colors.border
            })
            .hover(|style| style.border_color(colors.border_focused))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.query_split_drag = Some((f32::from(event.position.y), this.query_split));
                    cx.notify();
                }),
            );

        let body: gpui::AnyElement = if let Some(error) = &self.query_error {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(error.clone())
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .into_any_element()
        } else if let Some(result) = &self.result {
            render_grid(
                result,
                &self.result_scroll,
                "el-query-grid",
                cx.theme().colors().element_background,
            )
        } else if self.connections.is_empty() {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "No connections yet — run el: initialize workspace to create el/connections.yml.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "Write a query and run it — or click a table in the EL panel to \
                         start from SELECT *.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .when(dragging, |flex| {
                flex.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((start_y, start_split)) = this.query_split_drag {
                        this.query_split =
                            (start_split + f32::from(event.position.y) - start_y).clamp(56., 480.);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.query_split_drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.query_split_drag = None;
                        cx.notify();
                    }),
                )
            })
            .child(toolbar)
            .child(editor)
            .child(splitter)
            .child(body)
            .into_any_element()
    }
}

impl ElRunsPanel {
    fn render_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        if self.remote_logs_dirty {
            self.remote_logs_dirty = false;
            let text = self
                .remote_logs
                .iter()
                .map(|line| line.as_ref())
                .collect::<Vec<_>>()
                .join("\n");
            self.remote_log_editor.update(cx, |editor, cx| {
                editor.set_read_only(false);
                editor.set_text(text, window, cx);
                editor.set_read_only(true);
                editor.move_to_end(&Default::default(), window, cx);
            });
        }
        let chips = h_flex().gap_1().flex_wrap().children(
            self.remotes
                .iter()
                .enumerate()
                .map(|(ix, name)| {
                    Button::new(("el-remote", ix), name.clone())
                        .label_size(LabelSize::Small)
                        .toggle_state(self.selected_remote == Some(ix))
                        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_remote = Some(ix);
                            this.remote_pipelines.clear();
                            this.remote_runs.clear();
                            this.remote_action_error = None;
                            this.remote_detail = None;
                            this.remote_detail_pending = false;
                            this.remote_run_detail = None;
                            this.start_remote_feed(cx);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>(),
        );
        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .child(chips)
            .child(div().flex_1())
            .children(self.remote_status_label().map(|status| {
                Label::new(status)
                    .size(LabelSize::XSmall)
                    .color(Color::Success)
            }))
            .children(self.remote_error.clone().map(|error| {
                Label::new(error)
                    .size(LabelSize::XSmall)
                    .color(Color::Error)
            }))
            .children(self.remote_action_error.clone().map(|error| {
                Label::new(error)
                    .size(LabelSize::XSmall)
                    .color(Color::Error)
            }))
            .child(
                Button::new("el-remote-logs", "Logs")
                    .label_size(LabelSize::XSmall)
                    .toggle_state(self.remote_show_logs)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_show_logs = !this.remote_show_logs;
                        cx.notify();
                    })),
            );

        let body: gpui::AnyElement = if self.remote_pipelines.is_empty()
            && self.remote_runs.is_empty()
            && self.remote_error.is_none()
        {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new("Connecting to the remote…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else if let Some(run_id) = self.remote_run_detail {
            self.render_remote_run_detail(run_id, cx)
        } else if let Some(detail) = self.remote_detail.clone() {
            self.render_remote_detail(&detail, cx)
        } else {
            self.render_remote_overview(cx)
        };

        if self.remote_show_logs {
            let log_pane = v_flex()
                .id("el-remote-logs-pane")
                .h(px(140.))
                .flex_shrink_0()
                .border_t_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .px_2()
                .py_1()
                .child(if self.remote_logs.is_empty() {
                    Label::new("No daemon activity yet.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .into_any_element()
                } else {
                    // A read-only editor: selectable, copyable logs.
                    self.remote_log_editor.clone().into_any_element()
                });
            return v_flex()
                .size_full()
                .child(toolbar)
                .child(body)
                .child(log_pane)
                .into_any_element();
        }
        v_flex()
            .size_full()
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    /// The overview: pipelines as a striped table (click a row for its
    /// detail view), recent runs below.
    fn render_remote_overview(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let head = |width: f32, text: &'static str| {
            div().w(px(width)).flex_shrink_0().child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Accent),
            )
        };
        let pipeline_header = h_flex()
            .w_full()
            .px_2()
            .gap_2()
            .child(head(140., "pipeline"))
            .child(head(55., "streams"))
            .child(head(110., "schedule"))
            .child(head(80., "next run"))
            .child(head(70., "profile"))
            .child(head(80., "created"))
            .child(head(80., "deployed"))
            .child(head(80., "last run"))
            .child(head(60., "state"));
        let mut pipelines = v_flex().w_full().px_1();
        for (ix, pipeline) in self.remote_pipelines.iter().enumerate() {
            let name: SharedString = pipeline.name.clone().into();
            let run_name = pipeline.name.clone();
            let cell = |width: f32, text: String, color: Color| {
                div().w(px(width)).flex_shrink_0().overflow_hidden().child(
                    Label::new(text)
                        .size(LabelSize::XSmall)
                        .color(color)
                        .truncate(),
                )
            };
            let open_name = name.clone();
            let cell_id = |col: &str| {
                SharedString::from(format!("el-remote-pipeline-{col}-{}", pipeline.name))
            };
            pipelines = pipelines.child(
                h_flex()
                    .id(("el-remote-pipeline", ix))
                    .w_full()
                    .h(px(26.))
                    .px_1()
                    .gap_2()
                    .items_center()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .hover(|style| style.bg(colors.element_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_detail = Some(open_name.clone());
                        cx.notify();
                    }))
                    .child(tip_cell(
                        cell_id("name"),
                        140.,
                        pipeline.name.clone(),
                        Color::Default,
                    ))
                    .child(cell(55., pipeline.streams.to_string(), Color::Muted))
                    .child(tip_cell(
                        cell_id("schedule"),
                        110.,
                        pipeline.schedule.clone().unwrap_or_else(|| "manual".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        80.,
                        pipeline
                            .next_run_unix
                            .map(|next| relative_time(next, true))
                            .unwrap_or_else(|| "—".into()),
                        Color::Muted,
                    ))
                    .child(tip_cell(
                        cell_id("profile"),
                        70.,
                        pipeline.profile.clone().unwrap_or_else(|| "default".into()),
                        if pipeline.profile.is_some() {
                            Color::Accent
                        } else {
                            Color::Muted
                        },
                    ))
                    .child(cell(
                        80.,
                        pipeline
                            .created_unix
                            .map(|at| relative_time(at, false))
                            .unwrap_or_else(|| "—".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        80.,
                        pipeline
                            .deployed_unix
                            .map(|at| relative_time(at, false))
                            .unwrap_or_else(|| "—".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        80.,
                        pipeline
                            .last_run_unix
                            .map(|at| relative_time(at, false))
                            .unwrap_or_else(|| "never".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        60.,
                        if pipeline.running {
                            "running".into()
                        } else {
                            "idle".into()
                        },
                        if pipeline.running {
                            Color::Accent
                        } else {
                            Color::Muted
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new(("el-remote-run", ix), "Run")
                            .label_size(LabelSize::XSmall)
                            .disabled(pipeline.running)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let name = run_name.clone();
                                this.remote_action(
                                    move |client| client.start_run(&name).map(|_| ()),
                                    cx,
                                );
                            })),
                    ),
            );
        }

        let dragging = self.remote_split_drag.is_some();
        v_flex()
            .flex_1()
            .min_h_0()
            .when(dragging, |flex| {
                flex.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((start_y, start_split)) = this.remote_split_drag {
                        this.remote_split =
                            (start_split + f32::from(event.position.y) - start_y).clamp(56., 480.);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.remote_split_drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.remote_split_drag = None;
                        cx.notify();
                    }),
                )
            })
            .child(
                v_flex()
                    .id("el-remote-pipelines-pane")
                    .h(px(self.remote_split))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .child(pipeline_header)
                    .child(pipelines),
            )
            .child(
                // The splitter: drag to trade space between the tables.
                div()
                    .id("el-remote-split")
                    .w_full()
                    .h(px(5.))
                    .flex_shrink_0()
                    .cursor(gpui::CursorStyle::ResizeRow)
                    .border_t_1()
                    .border_color(if dragging {
                        colors.border_focused
                    } else {
                        colors.border
                    })
                    .hover(|style| style.border_color(colors.border_focused))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.remote_split_drag =
                                Some((f32::from(event.position.y), this.remote_split));
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div().px_2().pt_1().child(
                    Label::new("Recent runs")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(self.render_runs_table(None, cx))
            .into_any_element()
    }

    /// One pipeline's page: back button, its facts, its runs.
    fn render_remote_detail(
        &mut self,
        name: &SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let pipeline = self
            .remote_pipelines
            .iter()
            .find(|pipeline| pipeline.name == name.as_ref());
        let run_name = name.to_string();
        let running = pipeline.map(|pipeline| pipeline.running).unwrap_or(false);
        let meta: SharedString = match pipeline {
            None if self.remote_detail_pending => "waiting for the server to list it…".into(),
            None => "no longer on the server".into(),
            Some(pipeline) => {
                let streams = match &pipeline.profile {
                    Some(profile) => format!(
                        "{} stream{} · profile {profile}",
                        pipeline.streams,
                        if pipeline.streams == 1 { "" } else { "s" }
                    ),
                    None => format!(
                        "{} stream{}",
                        pipeline.streams,
                        if pipeline.streams == 1 { "" } else { "s" }
                    ),
                };
                let mut text = match (&pipeline.schedule, pipeline.next_run_unix) {
                    (Some(schedule), Some(next)) => format!(
                        "{streams} — runs on {schedule}, next {}",
                        relative_time(next, true)
                    ),
                    (Some(schedule), None) => format!("{streams} — runs on {schedule}"),
                    (None, _) => format!("{streams} — manual runs only"),
                };
                if let Some(at) = pipeline.created_unix {
                    text.push_str(&format!(" · created {}", relative_time(at, false)));
                }
                if let Some(at) = pipeline.deployed_unix {
                    text.push_str(&format!(" · deployed {}", relative_time(at, false)));
                }
                if let Some(at) = pipeline.last_run_unix {
                    text.push_str(&format!(" · last run {}", relative_time(at, false)));
                }
                text.into()
            }
        };
        let header = h_flex()
            .w_full()
            .px_1()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .child(
                IconButton::new("el-remote-back", IconName::ArrowLeft)
                    .icon_size(IconSize::Small)
                    .tooltip(ui::Tooltip::text("Back to pipelines"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_detail = None;
                        cx.notify();
                    })),
            )
            .child(Label::new(name.clone()).size(LabelSize::Default))
            .child(Label::new(meta).size(LabelSize::XSmall).color(Color::Muted))
            .child(div().flex_1())
            .children(running.then(|| {
                Label::new("running")
                    .size(LabelSize::XSmall)
                    .color(Color::Accent)
            }))
            .child(
                Button::new("el-remote-detail-run", "Run")
                    .label_size(LabelSize::XSmall)
                    .style(ButtonStyle::Filled)
                    .disabled(running || pipeline.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let name = run_name.clone();
                        this.remote_action(move |client| client.start_run(&name).map(|_| ()), cx);
                    })),
            );

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                div().px_2().pt_1().child(
                    Label::new("Runs of this pipeline")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(self.render_runs_table(Some(name.as_ref()), cx))
            .into_any_element()
    }

    /// One run's page: verdict facts and the per-stream breakdown
    /// reconstructed from its event log (live while it runs).
    fn render_remote_run_detail(
        &mut self,
        run_id: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let run = self.remote_runs.iter().find(|run| run.id == run_id);
        let is_running = run.is_some_and(|run| run.status == "running");
        let status = run
            .map(|run| run.status.clone())
            .unwrap_or_else(|| "?".into());
        let status_color = match status.as_str() {
            "ok" => Color::Success,
            "failed" => Color::Error,
            "cancelled" => Color::Warning,
            _ => Color::Accent,
        };
        let facts: SharedString = match run {
            None => "no longer in the server's history".into(),
            Some(run) => {
                let mut text = format!(
                    "started {} · {}",
                    relative_time(run.started_unix, false),
                    duration_text(run.started_unix, run.finished_unix)
                );
                if !is_running {
                    text.push_str(&format!(" · {} rows", run.rows_written));
                }
                if run.attempt > 0 {
                    text.push_str(&format!(" · retry {}", run.attempt));
                }
                text.into()
            }
        };
        let header = h_flex()
            .w_full()
            .px_1()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .child(
                IconButton::new("el-run-back", IconName::ArrowLeft)
                    .icon_size(IconSize::Small)
                    .tooltip(ui::Tooltip::text("Back"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_run_detail = None;
                        this.remote_run_error = None;
                        this.remote_history_for = None;
                        cx.notify();
                    })),
            )
            .child(
                Label::new(format!(
                    "run #{run_id} · {}",
                    run.map(|run| run.pipeline.clone()).unwrap_or_default()
                ))
                .size(LabelSize::Default),
            )
            .child(
                Label::new(status)
                    .size(LabelSize::XSmall)
                    .color(status_color),
            )
            .child(
                Label::new(facts)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .children(is_running.then(|| {
                Button::new("el-run-detail-cancel", "Cancel")
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_action(move |client| client.cancel(run_id), cx);
                    }))
            }));

        // The run's own error (a fatal message or the failed-stream
        // count): the runs table truncates it, so show it whole here,
        // and keep the events-page copy once the run leaves `/runs`.
        let run_error: Option<SharedString> = run
            .and_then(|run| run.error.clone().map(SharedString::from))
            .or_else(|| self.remote_run_error.clone());
        let error_block = run_error.map(|error| {
            h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .items_start()
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .id("el-run-detail-error")
                        .flex_1()
                        .min_w_0()
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .child(
                            Label::new(error.clone())
                                .size(LabelSize::XSmall)
                                .color(Color::Error),
                        ),
                )
                .child(
                    ui::CopyButton::new("el-run-detail-copy-error", error)
                        .icon_size(IconSize::XSmall)
                        .tooltip_label("Copy error"),
                )
        });

        // Fold the event log into per-stream rows.
        let (order, agg) = fold_stream_events(self.run_events(run_id));

        let head = |width: f32, text: &'static str| {
            div().w(px(width)).flex_shrink_0().child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Accent),
            )
        };
        let stream_header = h_flex()
            .w_full()
            .px_2()
            .gap_2()
            .child(head(150., "stream"))
            .child(head(80., "phase"))
            .child(head(90., "rows read"))
            .child(head(90., "rows written"))
            .child(head(80., "cast fails"))
            .child(
                Label::new("status")
                    .size(LabelSize::XSmall)
                    .color(Color::Accent),
            );
        let mut rows = v_flex()
            .id("el-run-streams")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_1();
        if order.is_empty() {
            rows = rows.child(
                div().px_1().py_1().child(
                    Label::new("Waiting for the run's events…")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            );
        }
        let cell = |width: f32, text: String, color: Color| {
            div().w(px(width)).flex_shrink_0().overflow_hidden().child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(color)
                    .truncate(),
            )
        };
        for (ix, stream) in order.iter().enumerate() {
            let cell_id =
                |col: &str| SharedString::from(format!("el-run-{run_id}-stream-{ix}-{col}"));
            let entry = agg.get(stream);
            let (phase, read, written, casts, failures, error, done) = entry
                .map(|entry| {
                    (
                        entry.phase.clone(),
                        entry.read,
                        entry.written,
                        entry.casts,
                        entry.failures.clone(),
                        entry.error.clone(),
                        entry.done,
                    )
                })
                .unwrap_or((None, 0, 0, 0, Vec::new(), None, false));
            // The count is a badge that opens the failing columns; an
            // older daemon sends the count alone, which stays a plain cell.
            let casts_cell: gpui::AnyElement = if casts > 0 && !failures.is_empty() {
                let stream_name = stream.clone();
                div()
                    .w(px(80.))
                    .flex_shrink_0()
                    .child(
                        Button::new(("el-remote-casts", ix), casts.to_string())
                            .label_size(LabelSize::XSmall)
                            .color(Color::Warning)
                            .tooltip(ui::Tooltip::text(
                                "Show the failing columns and sample values",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (columns, rows) = failures_table(&failures);
                                this.show_preview(failures_title(&stream_name), columns, rows, cx);
                            })),
                    )
                    .into_any_element()
            } else if casts > 0 {
                // A count with no columns has three causes, and only the
                // last one is the daemon's age.
                let why = if error.is_some() {
                    "The failing columns aren't reported for a stream that failed"
                } else if !done {
                    "The failing columns arrive when the stream finishes"
                } else {
                    "This server doesn't report failing columns — update the daemon to see them"
                };
                div()
                    .id(("el-remote-casts-count", ix))
                    .w(px(80.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .tooltip(ui::Tooltip::text(why))
                    .child(
                        Label::new(casts.to_string())
                            .size(LabelSize::XSmall)
                            .color(Color::Warning)
                            .truncate(),
                    )
                    .into_any_element()
            } else {
                cell(80., casts.to_string(), Color::Muted).into_any_element()
            };
            let (status_text, status_color) = match (&error, done, &phase) {
                (Some(error), _, _) => (error.clone(), Color::Error),
                (None, true, _) => ("done".to_owned(), Color::Success),
                (None, false, Some(_)) => ("running".to_owned(), Color::Accent),
                (None, false, None) => ("queued".to_owned(), Color::Muted),
            };
            rows = rows.child(
                h_flex()
                    .w_full()
                    .h(px(24.))
                    .px_1()
                    .gap_2()
                    .items_center()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .child(tip_cell(
                        cell_id("name"),
                        150.,
                        stream.clone(),
                        Color::Default,
                    ))
                    .child(cell(80., phase.unwrap_or_else(|| "—".into()), Color::Muted))
                    .child(cell(90., read.to_string(), Color::Muted))
                    .child(cell(90., written.to_string(), Color::Default))
                    .child(casts_cell)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .items_center()
                            .child(
                                tip_text(cell_id("status"), status_text, status_color)
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .children(error.map(|error| {
                                ui::CopyButton::new(cell_id("copy"), error)
                                    .icon_size(IconSize::XSmall)
                                    .tooltip_label("Copy error")
                            })),
                    ),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .children(error_block)
            .child(stream_header)
            .child(rows)
            .into_any_element()
    }

    /// The run-history table, striped; `filter` narrows to one pipeline.
    fn render_runs_table(
        &mut self,
        filter: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let col = |width: f32, element: gpui::AnyElement| {
            div()
                .w(px(width))
                .flex_shrink_0()
                .overflow_hidden()
                .child(element)
        };
        let head = |width: f32, text: &'static str| {
            col(
                width,
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Accent)
                    .into_any_element(),
            )
        };
        let show_pipeline = filter.is_none();
        let mut header = h_flex().w_full().px_2().gap_2().child(head(44., "run"));
        if show_pipeline {
            header = header.child(head(130., "pipeline"));
        }
        let header = header
            .child(head(70., "status"))
            .child(head(80., "started"))
            .child(head(64., "duration"))
            .child(head(70., "rows"))
            .child(head(64., "cast fails"))
            .child(head(50., "attempt"))
            .child(
                Label::new("error")
                    .size(LabelSize::XSmall)
                    .color(Color::Accent),
            );

        let mut runs = v_flex()
            .id("el-remote-runs")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_1();
        let mut shown = 0usize;
        for run in &self.remote_runs {
            if filter.is_some_and(|name| run.pipeline != name) {
                continue;
            }
            let stripe = shown % 2 == 1;
            shown += 1;
            let status_color = match run.status.as_str() {
                "ok" => Color::Success,
                "failed" => Color::Error,
                "cancelled" => Color::Warning,
                _ => Color::Accent,
            };
            let run_id = run.id;
            let is_running = run.status == "running";
            let cell = |width: f32, text: String, color: Color| {
                col(
                    width,
                    Label::new(text)
                        .size(LabelSize::XSmall)
                        .color(color)
                        .truncate()
                        .into_any_element(),
                )
            };
            let mut row = h_flex()
                .id(("el-run-row", run.id as usize))
                .w_full()
                .h(px(24.))
                .px_1()
                .gap_2()
                .items_center()
                .rounded_sm()
                .when(stripe, |row| row.bg(colors.element_background))
                .hover(|style| style.bg(colors.element_hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.remote_run_detail = Some(run_id);
                    this.remote_run_error = None;
                    // Live runs are already streaming in; an older one is
                    // history the socket does not carry.
                    this.ensure_run_events(run_id, cx);
                    cx.notify();
                }))
                .child(cell(44., format!("#{}", run.id), Color::Muted));
            if show_pipeline {
                row = row.child(tip_cell(
                    ("el-run-pipeline", run.id as usize),
                    130.,
                    run.pipeline.clone(),
                    Color::Default,
                ));
            }
            runs = runs.child(
                row.child(tip_cell(
                    ("el-run-status", run.id as usize),
                    70.,
                    run.status.clone(),
                    status_color,
                ))
                    .child(cell(
                        80.,
                        relative_time(run.started_unix, false),
                        Color::Muted,
                    ))
                    .child(cell(
                        64.,
                        duration_text(run.started_unix, run.finished_unix),
                        Color::Muted,
                    ))
                    .child(cell(
                        70.,
                        if is_running {
                            "…".to_owned()
                        } else {
                            run.rows_written.to_string()
                        },
                        Color::Default,
                    ))
                    .child(match run.cast_failures {
                        // An older daemon reports no count at all — say so
                        // rather than passing it off as a clean run.
                        None => col(
                            64.,
                            div()
                                .id(("el-run-casts", run.id as usize))
                                .tooltip(ui::Tooltip::text(
                                    "This server doesn't report cast failures — update the daemon to see them",
                                ))
                                .child(
                                    Label::new("?")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .into_any_element(),
                        ),
                        Some(0) => cell(64., "—".to_owned(), Color::Muted),
                        Some(count) => cell(64., count.to_string(), Color::Warning),
                    })
                    .child(cell(
                        50.,
                        if run.attempt == 0 {
                            "—".to_owned()
                        } else {
                            format!("retry {}", run.attempt)
                        },
                        if run.attempt == 0 { Color::Muted } else { Color::Warning },
                    ))
                    .child(
                        tip_text(
                            ("el-run-error", run.id as usize),
                            run.error.clone().unwrap_or_default(),
                            Color::Error,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .children(run.error.clone().map(|error| {
                        ui::CopyButton::new(("el-run-copy-error", run.id as usize), error)
                            .icon_size(IconSize::XSmall)
                            .tooltip_label("Copy error")
                    }))
                    .children(is_running.then(|| {
                        Button::new(("el-remote-cancel", run.id as usize), "Cancel")
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote_action(
                                    move |client| client.cancel(run_id),
                                    cx,
                                );
                            }))
                    })),
            );
        }
        if shown == 0 {
            runs = runs.child(
                div().px_1().py_1().child(
                    Label::new("No runs yet — press Run above.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(runs)
            .into_any_element()
    }
}

/// "in 1m 20s" / "3m ago" for a unix instant, relative to now.
pub(crate) fn relative_time(unix: u64, future: bool) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let delta = if future {
        unix.saturating_sub(now)
    } else {
        now.saturating_sub(unix)
    };
    let text = if delta >= 86_400 {
        format!("{}d {}h", delta / 86_400, (delta % 86_400) / 3600)
    } else if delta >= 3600 {
        format!("{}h {}m", delta / 3600, (delta % 3600) / 60)
    } else if delta >= 60 {
        format!("{}m {}s", delta / 60, delta % 60)
    } else {
        format!("{delta}s")
    };
    if future {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

fn duration_text(started: u64, finished: Option<u64>) -> String {
    let Some(finished) = finished else {
        return "…".to_owned();
    };
    let delta = finished.saturating_sub(started);
    if delta >= 60 {
        format!("{}m {}s", delta / 60, delta % 60)
    } else {
        format!("{delta}s")
    }
}

/// A tooltip wraps but has no height cap, so hover text stops here; the
/// copy action always takes the whole string.
const TOOLTIP_CAP: usize = 600;

/// The text a truncated cell shows on hover: the cell's own text, cut
/// to a readable length.
pub(super) fn tooltip_text(text: &str) -> SharedString {
    match text.char_indices().nth(TOOLTIP_CAP) {
        None => text.to_owned().into(),
        Some((cut, _)) => format!("{}…", &text[..cut]).into(),
    }
}

/// A truncating text cell that shows its full text on hover. Nothing
/// reports whether the label actually clipped, so every text cell
/// carries the tooltip; numeric cells keep the plain closure.
fn tip_text(
    id: impl Into<gpui::ElementId>,
    text: String,
    color: Color,
) -> gpui::Stateful<gpui::Div> {
    let tip = tooltip_text(&text);
    div()
        .id(id)
        .overflow_hidden()
        .when(!text.is_empty(), move |cell| {
            cell.tooltip(ui::Tooltip::text(tip))
        })
        .child(
            Label::new(text)
                .size(LabelSize::XSmall)
                .color(color)
                .truncate(),
        )
}

/// [`tip_text`] at a fixed column width.
fn tip_cell(
    id: impl Into<gpui::ElementId>,
    width: f32,
    text: String,
    color: Color,
) -> gpui::Stateful<gpui::Div> {
    tip_text(id, text, color).w(px(width)).flex_shrink_0()
}

/// The shared results grid: fixed-width columns, accent header, uniform
/// rows. Used by mapping previews and query results alike.
fn render_grid(
    table: &PreviewTable,
    scroll: &UniformListScrollHandle,
    list_id: &'static str,
    stripe: gpui::Hsla,
) -> gpui::AnyElement {
    const COL_WIDTH: f32 = 170.;
    let columns = table.columns.clone();
    let rows = table.rows.clone();
    let total = px(columns.len() as f32 * COL_WIDTH);
    let header = h_flex()
        .w(total)
        .flex_shrink_0()
        .children(columns.iter().map(|column| {
            div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
                Label::new(column.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Accent)
                    .truncate(),
            )
        }));
    let count = rows.len();
    let list =
        gpui::uniform_list(list_id, count, {
            move |range, _, _| {
                range
                    .filter_map(|ix| rows.get(ix).map(|row| (ix, row)))
                    .map(|(ix, row)| {
                        h_flex()
                            .w(total)
                            .h(px(24.))
                            .flex_shrink_0()
                            .when(ix % 2 == 1, |row| row.bg(stripe))
                            .children(row.iter().map(|cell| {
                                div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
                                    Label::new(cell.clone()).size(LabelSize::XSmall).truncate(),
                                )
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }
        })
        .flex_1()
        .track_scroll(scroll);

    div()
        .id(gpui::SharedString::from(format!("{list_id}-scroll")))
        .flex_1()
        .min_h_0()
        .overflow_x_scroll()
        .child(v_flex().w(total).h_full().child(header).child(list))
        .into_any_element()
}

impl EventEmitter<PanelEvent> for ElRunsPanel {}

impl Focusable for ElRunsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ElRunsPanel {
    fn persistent_name() -> &'static str {
        "EL Runs Panel"
    }

    fn panel_key() -> &'static str {
        "ElRunsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayFilled)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("EL Console")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElRunsFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |this, _, cx| this.refresh_connections(cx));
        }
    }
}

impl Render for ElRunsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let body: gpui::AnyElement = match &self.preview {
            Some(preview) => {
                let count = preview.rows.len();
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .w_full()
                            .p_1()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                IconButton::new("el-preview-back", IconName::ArrowLeft)
                                    .icon_size(IconSize::Small)
                                    .tooltip(ui::Tooltip::text("Back"))
                                    .on_click(cx.listener(|this, _, _, cx| this.show_runs(cx))),
                            )
                            .child(Label::new(preview.title.clone()).size(LabelSize::Default))
                            .child(
                                Label::new(format!("{count} rows"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(render_grid(
                        preview,
                        &self.preview_scroll,
                        "el-preview-rows",
                        colors.element_background,
                    ))
                    .into_any_element()
            }
            None => {
                let tab = |id: &'static str,
                           label: &'static str,
                           surface: Surface,
                           this: &Self,
                           cx: &mut Context<Self>| {
                    Button::new(id, label)
                        .label_size(LabelSize::Default)
                        .toggle_state(this.surface == surface)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.surface = surface;
                            match surface {
                                Surface::Query => this.refresh_connections(cx),
                                Surface::Remote => {
                                    this.refresh_connections(cx);
                                    this.start_remote_feed(cx);
                                }
                                Surface::Runs => {}
                            }
                            cx.notify();
                        }))
                };
                let header = h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(tab("el-console-runs", "Runs", Surface::Runs, self, cx))
                    .child(tab("el-console-query", "Query", Surface::Query, self, cx))
                    .children(
                        (!self.remotes.is_empty())
                            .then(|| tab("el-console-remote", "Remote", Surface::Remote, self, cx)),
                    );
                let content: gpui::AnyElement = match self.surface {
                    Surface::Runs => self.run_view.clone().into_any_element(),
                    Surface::Query => self.render_query(cx),
                    Surface::Remote => self.render_remote(window, cx),
                };
                v_flex()
                    .size_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(content))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElRunsPanel")
            .bg(colors.panel_background)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        REMOTE_LOG_CAP, TOOLTIP_CAP, append_log, merge_log_tail, merge_run, place_run_event,
        tooltip_text,
    };

    fn run(id: u64, status: &str) -> el_engine::server::RemoteRun {
        el_engine::server::RemoteRun {
            id,
            pipeline: "orders".to_owned(),
            status: status.to_owned(),
            attempt: 0,
            started_unix: 10,
            finished_unix: None,
            rows_written: 0,
            cast_failures: None,
            error: None,
        }
    }

    fn started(pipeline: &str) -> el_engine::ProgressEvent {
        el_engine::ProgressEvent::RunStarted {
            pipeline: pipeline.to_owned(),
            streams: Vec::new(),
        }
    }

    fn stream(name: &str) -> el_engine::ProgressEvent {
        el_engine::ProgressEvent::StreamStarted {
            stream: name.to_owned(),
        }
    }

    /// A run pushed twice is the same run, not two rows: the snapshot and
    /// the stream that follows it overlap by design.
    #[test]
    fn a_pushed_run_replaces_its_own_row_and_new_ones_land_on_top() {
        let mut runs = vec![run(2, "ok"), run(1, "ok")];
        merge_run(&mut runs, run(2, "failed"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].status, "failed");
        merge_run(&mut runs, run(3, "running"));
        assert_eq!(runs.iter().map(|run| run.id).collect::<Vec<_>>(), [3, 2, 1]);
    }

    #[test]
    fn progress_events_dedupe_and_a_gap_drops_the_list() {
        let mut events: HashMap<u64, Vec<el_engine::ProgressEvent>> = HashMap::new();

        // A list never starts part-way through.
        place_run_event(&mut events, 1, 3, stream("orders"));
        assert!(!events.contains_key(&1));

        place_run_event(&mut events, 1, 0, started("orders"));
        place_run_event(&mut events, 1, 1, stream("orders"));
        assert_eq!(events[&1].len(), 2);

        // The snapshot/stream overlap: an index already held is dropped.
        place_run_event(&mut events, 1, 1, stream("again"));
        assert_eq!(events[&1].len(), 2);
        assert!(matches!(
            events[&1][1],
            el_engine::ProgressEvent::StreamStarted { ref stream } if stream == "orders"
        ));

        // A gap cannot be folded into honest rows, so the list goes and
        // the detail view refetches the history.
        place_run_event(&mut events, 1, 5, stream("skipped"));
        assert!(!events.contains_key(&1), "a gap discards the partial list");
    }

    #[test]
    fn log_lines_dedupe_by_sequence_and_the_buffer_stays_capped() {
        let mut logs = Vec::new();
        let mut next = 0;
        assert!(append_log(&mut logs, &mut next, 0, "first".to_owned()));
        assert_eq!(next, 1);
        // Already held: no change, and the editor is not rebuilt.
        assert!(!append_log(&mut logs, &mut next, 0, "first".to_owned()));
        assert_eq!(logs.len(), 1);

        for seq in 1..=REMOTE_LOG_CAP as u64 + 10 {
            assert!(append_log(&mut logs, &mut next, seq, format!("line {seq}")));
        }
        assert_eq!(logs.len(), REMOTE_LOG_CAP);
        assert_eq!(
            logs.last().unwrap().as_ref(),
            format!("line {}", REMOTE_LOG_CAP + 10)
        );
    }

    /// The server retires a session every half hour and the client comes
    /// straight back with a fresh snapshot. That must extend the log the
    /// operator is reading, not cut it back to the tail the server sends.
    #[test]
    fn a_reconnect_extends_the_log_instead_of_truncating_it() {
        let mut logs = Vec::new();
        let mut next = 0;

        // First attach: the server's whole tail is new.
        assert!(merge_log_tail(
            &mut logs,
            &mut next,
            vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            3,
        ));
        assert_eq!(logs.len(), 3);
        assert_eq!(next, 3);

        // Reconnect: the same tail, plus a line that happened meanwhile.
        assert!(merge_log_tail(
            &mut logs,
            &mut next,
            vec!["two".to_owned(), "three".to_owned(), "four".to_owned()],
            4,
        ));
        assert_eq!(
            logs.iter().map(|line| line.as_ref()).collect::<Vec<_>>(),
            ["one", "two", "three", "four"],
            "the overlap is dropped and nothing already read is lost"
        );

        // And a snapshot with nothing new changes nothing at all.
        assert!(!merge_log_tail(
            &mut logs,
            &mut next,
            vec!["three".to_owned(), "four".to_owned()],
            4,
        ));
        assert_eq!(logs.len(), 4);
    }

    #[test]
    fn tooltip_text_keeps_short_text_and_caps_long_text() {
        assert_eq!(tooltip_text("boom").as_ref(), "boom");
        let long = "é".repeat(TOOLTIP_CAP + 50);
        let tip = tooltip_text(&long);
        assert_eq!(tip.chars().count(), TOOLTIP_CAP + 1);
        assert!(tip.ends_with('…'));
        let exact = "x".repeat(TOOLTIP_CAP);
        assert_eq!(tooltip_text(&exact).as_ref(), exact);
    }
}
