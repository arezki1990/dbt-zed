//! The EL console — the plugin's bottom-dock surface. Two views: Runs
//! (hosting [`super::run_view::ElRunView`]) and Query (ad-hoc SQL against
//! any EL connection, through the on-demand worker). Mapping-editor
//! previews overlay either view and return with Back.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use editor::Editor;
use gpui::{
    Animation, AnimationExt as _, App, AsyncWindowContext, Context, Entity, EventEmitter,
    FocusHandle, Focusable, ScrollHandle, Task, UniformListScrollHandle, WeakEntity, Window,
    pulsating_between,
};
use ui::{WithScrollbar as _, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::run_view::ElRunView;
use crate::ToggleElRunsFocus;

const QUERY_ROW_CAP: usize = 500;

/// How long the explorer lets a query run before it stops it — the
/// engine's limit, on this machine's worker and on a remote's alike. Said
/// in the panel because the engine's own words for it are "is the database
/// reachable?", which sends people looking at the network for a query
/// that was merely slow.
const QUERY_LIMIT_SECS: u64 = 90;

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
    preview: Option<ConsoleGrid>,
    preview_scroll: UniformListScrollHandle,
    preview_h_scroll: ScrollHandle,
    // Query surface state.
    sql: Entity<Editor>,
    /// (name, kind) — names only, credential posture as everywhere.
    connections: Vec<(SharedString, SharedString)>,
    /// Where each connection's queries run, by the same index: the remote
    /// its `workspace:` names, or None for this machine's own worker.
    connection_workspaces: Vec<Option<SharedString>>,
    selected: Option<usize>,
    root: Option<PathBuf>,
    running: bool,
    result: Option<ConsoleGrid>,
    result_scroll: UniformListScrollHandle,
    result_h_scroll: ScrollHandle,
    /// The value of the cell a grid opened whole.
    detail_scroll: ScrollHandle,
    elapsed: Option<Duration>,
    query_error: Option<SharedString>,
    /// When the query in flight was sent, for a clock that moves.
    query_started: Option<Instant>,
    /// Said once, after the operator stopped waiting for a query.
    query_notice: Option<SharedString>,
    _query: Task<()>,
    /// Redraws once a second while a query runs.
    _query_tick: Task<()>,
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
    /// New lines have arrived and the pane has not yet followed them.
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
    remote_pipelines_scroll: ScrollHandle,
    remote_runs_scroll: ScrollHandle,
    remote_logs_scroll: ScrollHandle,
    remote_streams_scroll: ScrollHandle,
    /// (run, stream) → recent (when, rows written) samples. The rate a
    /// run shows is measured over the last minute of them: an average
    /// since the start would keep looking healthy through a stall.
    rate_samples: HashMap<(u64, String), std::collections::VecDeque<(Instant, u64)>>,
    /// Pipelines whose streams are unfolded in the list, so one table can
    /// be launched without leaving it.
    remote_expanded: HashSet<String>,
    /// Streams ticked on a pipeline's page — what "Run selected" sends.
    remote_stream_picks: HashSet<String>,
    /// Log pane: its height (draggable), whether it sticks to the newest
    /// line, and whether it hides everything that isn't a problem.
    remote_log_height: f32,
    remote_log_drag: Option<(f32, f32)>,
    remote_logs_follow: bool,
    remote_logs_problems_only: bool,
    run_streams_scroll: ScrollHandle,
    run_error_scroll: ScrollHandle,
    /// Height of the SQL editor in the Query tab; its splitter drags it.
    query_split: f32,
    query_split_drag: Option<(f32, f32)>,
    /// Height of the pipelines section once the splitter has been dragged.
    /// Until then the section is as tall as what it holds — three
    /// pipelines over a fixed 170 px left a band of nothing above the runs.
    remote_split: f32,
    /// Whether the height above is the operator's. A double-click on the
    /// splitter hands it back to the content.
    remote_split_by_hand: bool,
    /// The height on screen right now, which is what a drag starts from.
    remote_split_shown: f32,
    /// (pointer y at drag start, split at drag start).
    remote_split_drag: Option<(f32, f32)>,
    remote_epoch: u64,
    /// The live socket's applying task, and the polling fallback for a
    /// server too old to have one. Only ever one of them is doing work.
    _remote_feed: Task<()>,
    _remote_poll: Task<()>,
    /// Redraws once a second while a run is in flight, so its elapsed
    /// time moves even when the server has nothing new to say.
    _remote_tick: Task<()>,
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

/// A table on screen in the console with what its grid keeps between
/// frames — the same things the dbt results grid keeps: the header sort
/// and the row order it gives, column widths (fitted to the data, then
/// dragged), a column resize in progress, and the cell opened whole.
struct ConsoleGrid {
    table: PreviewTable,
    /// (column, ascending).
    sort: Option<(usize, bool)>,
    /// Row indices in display order.
    order: std::sync::Arc<Vec<usize>>,
    widths: std::sync::Arc<Vec<f32>>,
    /// (column, pointer x, width) when the drag started.
    resize: Option<(usize, f32, f32)>,
    /// (column, row number, value) of the clicked cell.
    detail: Option<(SharedString, usize, SharedString)>,
}

impl ConsoleGrid {
    fn new(table: PreviewTable) -> Self {
        Self {
            order: std::sync::Arc::new((0..table.rows.len()).collect()),
            widths: std::sync::Arc::new(fit_widths(&table.columns, &table.rows)),
            table,
            sort: None,
            resize: None,
            detail: None,
        }
    }

    /// A header click: ascending, then descending, then the table's own order.
    fn cycle_sort(&mut self, column: usize) {
        self.sort = match self.sort {
            Some((sorted, true)) if sorted == column => Some((column, false)),
            Some((sorted, false)) if sorted == column => None,
            _ => Some((column, true)),
        };
        self.order = std::sync::Arc::new(sorted_rows(&self.table.rows, self.sort));
    }
}

/// Which of the console's two grids an action is for.
#[derive(Clone, Copy)]
enum GridSlot {
    Preview,
    Result,
}

impl GridSlot {
    fn id(self) -> &'static str {
        match self {
            GridSlot::Preview => "el-preview-grid",
            GridSlot::Result => "el-query-grid",
        }
    }
}

/// Column widths fitted to the data, as the dbt results grid fits them: the
/// longest of the header and the first rows' values, within bounds.
fn fit_widths(columns: &[SharedString], rows: &[Vec<SharedString>]) -> Vec<f32> {
    columns
        .iter()
        .enumerate()
        .map(|(column, name)| {
            let chars = rows
                .iter()
                .take(30)
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .fold(name.chars().count(), usize::max);
            (chars as f32 * 8.2 + 24.).clamp(90., 340.)
        })
        .collect()
}

/// The row order for a header sort: numbers by value, everything else as
/// text. Numbers sort before text, so a column that mixes the two still
/// has a total order (a sort may panic without one); ties keep the
/// table's own order.
fn sorted_rows(rows: &[Vec<SharedString>], sort: Option<(usize, bool)>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    let Some((column, ascending)) = sort else {
        return order;
    };
    let keys: Vec<(Option<f64>, &str)> = rows
        .iter()
        .map(|row| {
            let cell = row.get(column).map_or("", |cell| cell.as_ref());
            (cell.trim().parse::<f64>().ok(), cell)
        })
        .collect();
    order.sort_by(|&a, &b| {
        let ordering = match (keys[a], keys[b]) {
            ((Some(x), _), (Some(y), _)) => x.total_cmp(&y),
            ((Some(_), _), (None, _)) => std::cmp::Ordering::Less,
            ((None, _), (Some(_), _)) => std::cmp::Ordering::Greater,
            ((None, x), (None, y)) => x.cmp(y),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    order
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
    /// What the source said it holds, when it could say cheaply. The
    /// denominator of the progress bar; None leaves the bar off.
    pub(crate) estimated: Option<u64>,
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
            E::StreamStarted {
                stream,
                estimated_rows,
            } => {
                let entry = agg.entry(stream.clone()).or_default();
                entry.phase = Some("connect".to_owned());
                // Sent again once the source answers; the first one has
                // nothing to say and must not erase the second.
                if estimated_rows.is_some() {
                    entry.estimated = *estimated_rows;
                }
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
                preview_h_scroll: ScrollHandle::new(),
                sql,
                connections: Vec::new(),
                connection_workspaces: Vec::new(),
                selected: None,
                root: None,
                running: false,
                result: None,
                result_scroll: UniformListScrollHandle::new(),
                result_h_scroll: ScrollHandle::new(),
                detail_scroll: ScrollHandle::new(),
                elapsed: None,
                query_error: None,
                query_started: None,
                query_notice: None,
                _query: Task::ready(()),
                _query_tick: Task::ready(()),
                remotes: Vec::new(),
                selected_remote: None,
                remote_pipelines: Vec::new(),
                remote_runs: Vec::new(),
                remote_error: None,
                remote_action_error: None,
                remote_status: None,
                remote_logs: Vec::new(),
                remote_logs_dirty: false,
                remote_log_next: 0,
                remote_show_logs: false,
                remote_detail: None,
                remote_detail_pending: false,
                remote_run_detail: None,
                remote_run_events: HashMap::new(),
                remote_run_error: None,
                remote_history_for: None,
                remote_pipelines_scroll: ScrollHandle::new(),
                remote_runs_scroll: ScrollHandle::new(),
                remote_logs_scroll: ScrollHandle::new(),
                remote_streams_scroll: ScrollHandle::new(),
                rate_samples: HashMap::new(),
                remote_expanded: HashSet::new(),
                remote_stream_picks: HashSet::new(),
                remote_log_height: 170.,
                remote_log_drag: None,
                remote_logs_follow: true,
                remote_logs_problems_only: false,
                run_streams_scroll: ScrollHandle::new(),
                run_error_scroll: ScrollHandle::new(),
                query_split: 110.,
                query_split_drag: None,
                remote_split: 170.,
                remote_split_by_hand: false,
                remote_split_shown: 170.,
                remote_split_drag: None,
                remote_epoch: 0,
                _remote_feed: Task::ready(()),
                _remote_poll: Task::ready(()),
                _remote_tick: Task::ready(()),
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
        self.preview = Some(ConsoleGrid::new(PreviewTable {
            title,
            columns: std::sync::Arc::new(columns),
            rows: std::sync::Arc::new(rows),
        }));
        scroll_home(&self.preview_scroll, &self.preview_h_scroll);
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
        schema: Option<&str>,
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
        let kind = self
            .selected
            .and_then(|ix| self.connections.get(ix))
            .map(|(_, kind)| kind.as_ref())
            .unwrap_or("");
        let sql = select_sql(kind, schema, table);
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
        let loaded = el_engine::spec::load_active_connections(&root)
            .map(|(connections, _)| connections)
            .ok();
        self.connections = loaded
            .iter()
            .flat_map(|connections| connections.connections.iter())
            .map(|(name, connection)| (name.clone().into(), connection.kind().to_owned().into()))
            .collect();
        self.connection_workspaces = loaded
            .iter()
            .flat_map(|connections| connections.connections.values())
            .map(|connection| connection.workspace().map(|remote| remote.to_owned().into()))
            .collect();
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
        self._remote_tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        // Only a run in flight has anything that changes by
                        // the second; an idle panel is left alone.
                        if this.surface == Surface::Remote
                            && this.remote_runs.iter().any(|run| run.status == "running")
                        {
                            cx.notify();
                        }
                        this.remote_epoch == epoch
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        });
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

    /// Replaces the selected remote's connections with this project's,
    /// sealed to that daemon's key. The deploy popup does this too; here
    /// it stands alone, for a password that changed under a pipeline that
    /// is already deployed.
    fn send_connections_to_remote(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.remote_action(
            move |client| {
                let (key, _) = client.pubkey().map_err(|error| {
                    anyhow::anyhow!(
                        "this server can't be sent connections ({error}) — update the daemon"
                    )
                })?;
                let bundle = el_engine::secrets::ConnectionsBundle::read(&root)?;
                let sealed = bundle.seal(&key)?;
                client.send_connections(&sealed).map(|_| ())
            },
            cx,
        );
    }

    /// Records where a stream is now and answers how fast it is going,
    /// in rows per minute, measured across the last minute of samples.
    /// None until there are two far enough apart to divide by.
    fn sample_rate(&mut self, run_id: u64, stream: &str, rows: u64, done: bool) -> Option<f64> {
        const WINDOW: Duration = Duration::from_secs(60);
        let samples = self
            .rate_samples
            .entry((run_id, stream.to_owned()))
            .or_default();
        let now = Instant::now();
        // A finished stream keeps whatever it was doing when it ended;
        // sampling it again would only dilute the rate towards zero.
        if !done {
            match samples.back() {
                Some((_, last)) if *last == rows => {}
                _ => samples.push_back((now, rows)),
            }
        }
        while samples.len() > 2
            && samples
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > WINDOW)
        {
            samples.pop_front();
        }
        let (first_at, first_rows) = *samples.front()?;
        let (last_at, last_rows) = *samples.back()?;
        let elapsed = last_at.duration_since(first_at).as_secs_f64();
        (elapsed >= 2. && last_rows > first_rows)
            .then(|| (last_rows - first_rows) as f64 / elapsed * 60.)
    }

    /// Launches a run on the selected remote: the whole pipeline when
    /// `streams` is empty, or exactly the streams named.
    fn run_remote(&mut self, pipeline: String, streams: Vec<String>, cx: &mut Context<Self>) {
        self.remote_action(
            move |client| client.start_run(&pipeline, &streams).map(|_| ()),
            cx,
        );
    }

    /// One stream of a deployed pipeline: what it reads, where it lands,
    /// how it loads — and a Run that launches that stream by itself. The
    /// tick box appears on a pipeline's own page, where several streams
    /// can go in one run.
    fn render_stream_row(
        &self,
        scope: &'static str,
        pipeline: &str,
        stream: &el_engine::server::RemoteStream,
        ix: usize,
        pickable: bool,
        running: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let id = |part: &str| {
            SharedString::from(format!("{scope}-{pipeline}-{}-{part}", stream.name))
        };
        let incremental = stream.mode == "incremental";
        let picked = self.remote_stream_picks.contains(&stream.name);
        let pick_name = stream.name.clone();
        let run_name = stream.name.clone();
        let run_pipeline = pipeline.to_owned();
        h_flex()
            .id(id("row"))
            .w_full()
            .h(px(28.))
            .pl(px(if pickable { 6. } else { 24. }))
            .pr_1()
            .gap_2()
            .items_center()
            .rounded_sm()
            .when(ix % 2 == 1, |row| row.bg(colors.element_background))
            .hover(|style| style.bg(colors.element_hover))
            .when(pickable, |row| {
                row.child(
                    ui::Checkbox::new(id("pick"), picked.into()).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if !this.remote_stream_picks.remove(&pick_name) {
                                this.remote_stream_picks.insert(pick_name.clone());
                            }
                            cx.notify();
                        },
                    )),
                )
            })
            .child(tip_cell(
                id("name"),
                180.,
                stream.name.clone(),
                Color::Default,
            ))
            .child(tip_cell(
                id("source"),
                210.,
                stream.source.clone(),
                Color::Muted,
            ))
            .child(
                Icon::new(IconName::ArrowRight)
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            )
            .child(tip_cell(
                id("table"),
                210.,
                stream.table.clone(),
                Color::Default,
            ))
            .child(tip_cell(
                id("mode"),
                110.,
                if incremental {
                    "incremental".to_owned()
                } else {
                    "full refresh".to_owned()
                },
                if incremental {
                    Color::Accent
                } else {
                    Color::Muted
                },
            ))
            .child(div().flex_1())
            .child(
                Button::new(id("run"), "Run")
                    .label_size(LabelSize::Small)
                    .start_icon(Icon::new(IconName::PlayOutlined).size(IconSize::XSmall))
                    .disabled(running)
                    .tooltip(ui::Tooltip::text(if running {
                        format!("{run_pipeline} is already running")
                    } else {
                        format!("Run {run_name} on its own")
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.run_remote(run_pipeline.clone(), vec![run_name.clone()], cx);
                    })),
            )
            .into_any_element()
    }

    /// The column names over a stream table, aligned with its rows.
    fn stream_header(&self, pickable: bool) -> gpui::AnyElement {
        let head = column_head;
        h_flex()
            .w_full()
            .pl(px(if pickable { 6. } else { 24. }))
            .pr_1()
            .gap_2()
            .items_center()
            .when(pickable, |row| row.child(div().w(px(16.)).flex_shrink_0()))
            .child(head(180., "stream"))
            .child(head(210., "reads"))
            .child(div().w(px(12.)).flex_shrink_0())
            .child(head(210., "lands on"))
            .child(head(110., "mode"))
            .into_any_element()
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
        self.query_notice = None;
        self.query_started = Some(Instant::now());
        self._query_tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let running = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.running
                    })
                    .unwrap_or(false);
                if !running {
                    return;
                }
            }
        });
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
                this.query_started = None;
                match outcome {
                    Ok((result, elapsed)) => {
                        this.elapsed = Some(elapsed);
                        this.result = Some(ConsoleGrid::new(PreviewTable {
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
                        }));
                        scroll_home(&this.result_scroll, &this.result_h_scroll);
                    }
                    Err(error) => this.query_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Stops waiting for the query in flight. The wait is all that can be
    /// dropped from here: the worker call underneath blocks until the
    /// database answers or the explorer's own limit cuts it off, so the
    /// notice says what is true rather than "cancelled".
    fn stop_waiting(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        self._query = Task::ready(());
        self.running = false;
        self.query_started = None;
        self.query_notice = Some(
            format!(
                "Stopped waiting. The query itself ends on its own — the explorer cuts \
                 every query off at {QUERY_LIMIT_SECS} seconds."
            )
            .into(),
        );
        cx.notify();
    }

    fn render_query(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let selected = self
            .selected
            .and_then(|ix| self.connections.get(ix).cloned());
        let runs_on: Option<SharedString> = self
            .selected
            .and_then(|ix| self.connection_workspaces.get(ix).cloned().flatten());
        let where_text: SharedString = match &runs_on {
            Some(remote) => format!("on {remote}").into(),
            None => "on this machine".into(),
        };

        // The connection is chosen here as well as from the EL panel: it
        // used to be only stated, and changing it meant leaving the console.
        let picker: gpui::AnyElement = if self.connections.is_empty() {
            Label::new("No connections yet")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element()
        } else {
            let this = cx.entity();
            let connections = self.connections.clone();
            let current = self.selected;
            let label: SharedString = selected
                .as_ref()
                .map(|(name, _)| name.clone())
                .unwrap_or_else(|| "Pick a connection".into());
            ui::DropdownMenu::new(
                "el-query-connection",
                label,
                ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                    menu = menu.header("Query on");
                    for (ix, (name, kind)) in connections.iter().enumerate() {
                        let this = this.clone();
                        menu = menu.toggleable_entry(
                            format!("{name}  ·  {kind}"),
                            current == Some(ix),
                            ui::IconPosition::Start,
                            None,
                            move |_window, cx| {
                                this.update(cx, |this, cx| {
                                    this.selected = Some(ix);
                                    this.query_notice = None;
                                    cx.notify();
                                });
                            },
                        );
                    }
                    menu
                }),
            )
            .style(ui::DropdownStyle::Outlined)
            .trigger_tooltip(ui::Tooltip::text(
                "The connection this query runs on — any of el/connections.yml",
            ))
            .into_any_element()
        };

        let target = h_flex()
            .gap_2()
            .items_center()
            .child(picker)
            .children(selected.as_ref().map(|(_, kind)| {
                ui::Chip::new(kind.clone())
                    .label_size(LabelSize::Small)
                    .label_color(Color::Muted)
            }))
            .children(selected.as_ref().map(|(name, _)| {
                // Where it runs explains what the wait is made of, and why
                // a laptop with no route to the database still gets rows.
                let tip = match &runs_on {
                    Some(remote) => format!(
                        "{name} names the {remote} workspace in connections.yml: the query is \
                         sent to that server and runs there, next to the database"
                    ),
                    None => format!("{name} runs through this machine's own connector worker"),
                };
                h_flex()
                    .id("el-query-where")
                    .gap_1()
                    .items_center()
                    .when(runs_on.is_some(), |place| {
                        place.child(
                            Icon::new(IconName::Server)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        Label::new(format!("runs {where_text}"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .tooltip(ui::Tooltip::text(tip))
            }));

        let sql_focus = self.sql.focus_handle(cx);
        let action: gpui::AnyElement = if self.running {
            Button::new("el-query-stop", "Stop waiting")
                .label_size(LabelSize::Small)
                .style(ButtonStyle::Tinted(ui::TintColor::Warning))
                .start_icon(Icon::new(IconName::Stop).size(IconSize::Small))
                .tooltip(ui::Tooltip::text(format!(
                    "Give the console back. The query itself ends on its own, at the \
                     explorer's {QUERY_LIMIT_SECS}-second limit at the latest"
                )))
                .on_click(cx.listener(|this, _, _, cx| this.stop_waiting(cx)))
                .into_any_element()
        } else {
            let focus = sql_focus.clone();
            Button::new("el-query-run", "Run query")
                .label_size(LabelSize::Small)
                .style(ButtonStyle::Filled)
                .start_icon(Icon::new(IconName::PlayFilled).size(IconSize::Small))
                .disabled(selected.is_none())
                .tooltip(move |_window, cx| {
                    ui::Tooltip::for_action_in("Run query", &crate::RunQuery, &focus, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.run_query(cx)))
                .into_any_element()
        };

        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(target)
            .child(div().flex_1())
            .child(action);

        let dragging = self.query_split_drag.is_some();
        let editing = sql_focus.is_focused(window);
        let editor = div().w_full().px_1().child(
            div()
                .w_full()
                .h(px(self.query_split))
                .p_1()
                .rounded_sm()
                .border_1()
                // Which box has the keyboard is otherwise a blinking
                // cursor's worth of evidence.
                .border_color(if editing {
                    colors.border_focused
                } else {
                    colors.border
                })
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

        let run_hint = ui::KeyBinding::for_action_in(&crate::RunQuery, &sql_focus, cx);
        let body: gpui::AnyElement = if self.running {
            let waited = self
                .query_started
                .map(|started| started.elapsed().as_secs())
                .unwrap_or(0);
            v_flex()
                .flex_1()
                .p_3()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(live_dot("el-query-live", Color::Accent))
                        .child(
                            Label::new(format!("Running {where_text} · {}", span_text(waited)))
                                .size(LabelSize::Default),
                        ),
                )
                .child(
                    // How much of the explorer's patience is used up: a wait
                    // with an end in sight is a different wait.
                    div().w(px(260.)).child(ui::ProgressBar::new(
                        "el-query-limit",
                        waited.min(QUERY_LIMIT_SECS) as f32,
                        QUERY_LIMIT_SECS as f32,
                        cx,
                    )),
                )
                .child(
                    Label::new(format!(
                        "The explorer stops a query at {QUERY_LIMIT_SECS} seconds."
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .when(waited >= 15, |state| {
                    state.child(
                        Label::new(
                            "A long wait usually means the database is reading a whole table — \
                             a TOP / LIMIT, or a filter on an indexed column, answers sooner.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                })
                .into_any_element()
        } else if let Some(error) = self.query_error.clone() {
            // The engine's words for its own limit are "is the database
            // reachable?" — it usually is, and was busy.
            let timed_out = error.contains("timed out after");
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    ui::Callout::new()
                        .severity(ui::Severity::Error)
                        .icon(IconName::XCircle)
                        .title(if timed_out {
                            format!("The query did not finish in {QUERY_LIMIT_SECS} seconds")
                        } else {
                            "The query failed".to_owned()
                        })
                        .description_slot(
                            v_flex()
                                .gap_1()
                                .child(Label::new(error.clone()).size(LabelSize::Small))
                                .when(timed_out, |text| {
                                    text.child(
                                        Label::new(
                                            "That is the explorer's limit, not a network error: \
                                             the database was most likely busy reading. Add a \
                                             TOP / LIMIT, or filter on an indexed column.",
                                        )
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                    )
                                }),
                        )
                        .actions_slot(
                            ui::CopyButton::new("el-query-copy-error", error.to_string())
                                .icon_size(IconSize::Small)
                                .tooltip_label("Copy error"),
                        ),
                )
                .into_any_element()
        } else if let Some(result) = &self.result {
            let count = result.table.rows.len();
            let capped = count >= QUERY_ROW_CAP;
            let took = self.elapsed.map(|elapsed| elapsed.as_secs_f64());
            let result_bar = h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::Check)
                        .size(IconSize::Small)
                        .color(Color::Success),
                )
                .child(
                    Label::new(format!(
                        "{} {}",
                        super::thousands(count),
                        if count == 1 { "row" } else { "rows" }
                    ))
                    .size(LabelSize::Small),
                )
                .when(capped, |bar| {
                    bar.child(
                        div()
                            .id("el-query-capped")
                            .child(
                                Label::new(format!("first {QUERY_ROW_CAP} shown"))
                                    .size(LabelSize::Small)
                                    .color(Color::Warning),
                            )
                            .tooltip(ui::Tooltip::text(format!(
                                "The console shows at most {QUERY_ROW_CAP} rows. There may be \
                                 more: narrow the query to see the ones you are after"
                            ))),
                    )
                })
                .children(took.map(|took| {
                    Label::new(format!("{took:.2}s"))
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                }))
                .child(
                    Label::new(where_text.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
            v_flex()
                .flex_1()
                .min_h_0()
                .child(result_bar)
                .child(self.render_grid(GridSlot::Result, window, cx))
                .into_any_element()
        } else if self.connections.is_empty() {
            v_flex()
                .flex_1()
                .p_3()
                .gap_1p5()
                .child(Label::new("No connections yet").size(LabelSize::Default))
                .child(
                    Label::new(
                        "Run \"el: initialize workspace\" to create el/connections.yml, or add \
                         one from the EL panel.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            v_flex()
                .flex_1()
                .p_3()
                .gap_1p5()
                .children(self.query_notice.clone().map(|notice| {
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Info)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(Label::new(notice).size(LabelSize::Small))
                }))
                .child(Label::new("Write a query, then run it").size(LabelSize::Default))
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(run_hint)
                        .child(
                            Label::new(
                                "runs it — or click a table in the EL panel to start from its \
                                 first rows.",
                            )
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                        ),
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
        // Wraps: on a narrow dock the status and the buttons drop under the
        // remotes instead of being squeezed out of sight.
        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .flex_wrap()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(chips)
            .child(div().flex_1())
            .child(self.render_remote_status())
            .child(
                Button::new("el-remote-logs", "Server log")
                    .label_size(LabelSize::Small)
                    .start_icon(Icon::new(IconName::ListTree).size(IconSize::XSmall))
                    .toggle_state(self.remote_show_logs)
                    .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                    .tooltip(ui::Tooltip::text(
                        "What the daemon has been doing: deploys, runs, schedule fires",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_show_logs = !this.remote_show_logs;
                        // Opening it shows the newest line, not wherever
                        // the last reading left off.
                        this.remote_logs_dirty = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("el-remote-send-connections", "Send connections")
                    .label_size(LabelSize::Small)
                    .start_icon(Icon::new(IconName::Lock).size(IconSize::XSmall))
                    .tooltip(ui::Tooltip::text(
                        "Encrypt this project's connections to this server's own key and \
                         replace the ones it holds",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.send_connections_to_remote(cx);
                    })),
            );

        // Empty lists alone can't tell "still connecting" from "connected to a
        // server with nothing on it" — a fresh local runner is the latter. The
        // server's first answer (a snapshot, or health) is what sets the status.
        let holds_nothing = self.remote_pipelines.is_empty()
            && self.remote_runs.is_empty()
            && self.remote_error.is_none();
        let remote_name: SharedString = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix).cloned())
            .unwrap_or_else(|| "the remote".into());
        let nothing_held = self.remote_pipelines.is_empty() && self.remote_runs.is_empty();
        let body: gpui::AnyElement = if let (true, Some(error)) =
            (nothing_held, self.remote_error.clone())
        {
            self.render_remote_unreachable(&remote_name, error, cx)
        } else if holds_nothing && self.remote_status.is_none() {
            self.render_remote_connecting(&remote_name, cx)
        } else if holds_nothing {
            self.render_remote_empty(&remote_name, cx)
        } else if let Some(run_id) = self.remote_run_detail {
            self.render_remote_run_detail(run_id, window, cx)
        } else if let Some(detail) = self.remote_detail.clone() {
            self.render_remote_detail(&detail, window, cx)
        } else {
            self.render_remote_overview(window, cx)
        };

        let banners = self.render_remote_banners(nothing_held, cx);
        if self.remote_show_logs {
            let log_pane = self.render_remote_logs(window, cx);
            return v_flex()
                .size_full()
                .child(toolbar)
                .children(banners)
                .child(body)
                .child(log_pane)
                .into_any_element();
        }
        v_flex()
            .size_full()
            .child(toolbar)
            .children(banners)
            .child(body)
            .into_any_element()
    }

    /// The right of the toolbar: how the panel is attached, which profile
    /// the server runs under, how long it has been up, and whether it is
    /// busy. It used to be one green sentence; the profile — the fact that
    /// matters most before pressing Run — was its last three words. Built
    /// at render time, so the uptime is the real one rather than the one
    /// the last push carried.
    fn render_remote_status(&self) -> gpui::AnyElement {
        let Some(status) = &self.remote_status else {
            let (color, text) = if self.remote_error.is_some() {
                (Color::Error, "Offline")
            } else {
                (Color::Muted, "Connecting…")
            };
            return h_flex()
                .gap_1()
                .items_center()
                .child(ui::Indicator::dot().color(color))
                .child(Label::new(text).size(LabelSize::Small).color(Color::Muted))
                .into_any_element();
        };
        let (dot, how, how_tip) = if self.remote_error.is_some() {
            (
                Color::Error,
                "Offline",
                "The connection dropped — what is shown is the last known state",
            )
        } else if status.live {
            (
                Color::Success,
                "Live",
                "Attached to the server's socket: runs, progress and log lines arrive as they happen",
            )
        } else {
            (
                Color::Warning,
                "Polling",
                "This server predates the live socket, so the panel asks it every two seconds",
            )
        };
        let running = self
            .remote_runs
            .iter()
            .filter(|run| run.status == "running")
            .count();
        let started = status.started_unix;
        let profile = status.profile.clone();
        h_flex()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .id("el-remote-link")
                    .gap_1()
                    .items_center()
                    .child(ui::Indicator::dot().color(dot))
                    .child(Label::new(how).size(LabelSize::Small))
                    .tooltip(move |_window, cx| ui::Tooltip::with_meta(how, None, how_tip, cx)),
            )
            .children(profile.map(|profile| {
                // A production profile is dressed as a warning: the same
                // Run button writes to very different places.
                let production = profile.to_lowercase().contains("prod");
                ui::Chip::new(format!("profile {profile}"))
                    .label_size(LabelSize::Small)
                    .label_color(if production {
                        Color::Warning
                    } else {
                        Color::Accent
                    })
                    .tooltip(move |_window, cx| {
                        ui::Tooltip::with_meta(
                            format!("This server runs under its {profile} profile"),
                            None,
                            "A pipeline deployed with its own profile says so in its row",
                            cx,
                        )
                    })
            }))
            .child(
                div()
                    .id("el-remote-uptime")
                    .child(
                        Label::new(format!("up {}", elapsed_text(started)))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .tooltip(ui::Tooltip::text(format!(
                        "Started {}",
                        absolute_time(started)
                    ))),
            )
            .when(running > 0, |cluster| {
                cluster.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(live_dot("el-remote-busy", Color::Accent))
                        .child(
                            Label::new(format!("{running} running"))
                                .size(LabelSize::Small)
                                .color(Color::Accent),
                        ),
                )
            })
            .child(div().w(px(4.)))
            .into_any_element()
    }

    /// Waiting for the server's first word. Rows in the shape of what is
    /// coming hold the layout and say "loading" without a sentence doing
    /// it; the sentence is there for anyone who wants the name.
    fn render_remote_connecting(
        &self,
        remote: &SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let fill = cx.theme().colors().element_hover;
        let bar = move |width: f32| div().h(px(9.)).w(px(width)).rounded_sm().bg(fill);
        v_flex()
            .flex_1()
            .p_2()
            .gap_2()
            .child(
                Label::new(format!("Connecting to {remote}…"))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                v_flex()
                    .gap_2p5()
                    .children((0..3).map(|row| {
                        h_flex()
                            .gap_4()
                            .child(bar(130. - row as f32 * 18.))
                            .child(bar(34.))
                            .child(bar(84.))
                            .child(bar(150.))
                            .child(bar(110. + row as f32 * 24.))
                    }))
                    .with_animation(
                        "el-remote-skeleton",
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(pulsating_between(0.35, 0.9)),
                        |rows, delta| rows.opacity(delta),
                    ),
            )
            .into_any_element()
    }

    /// Connected, and the server holds nothing: say what to do about it.
    fn render_remote_empty(
        &self,
        remote: &SharedString,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        v_flex()
            .flex_1()
            .p_3()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::Server)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(format!("{remote} is up, and nothing is deployed on it yet"))
                            .size(LabelSize::Small),
                    ),
            )
            .child(
                Label::new(
                    "Open a pipeline on its canvas and press Deploy. The popup shows what \
                     would change on this server before anything does.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(
                h_flex().pt_1().child(
                    Button::new("el-remote-empty-open", "Open pipeline canvas")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Outlined)
                        .start_icon(Icon::new(IconName::ArrowRightLeft).size(IconSize::XSmall))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::OpenPipelines), cx);
                        }),
                ),
            )
            .into_any_element()
    }

    /// The server did not answer and there is nothing older to show. The
    /// error used to be a red sentence squeezed between the remote chips
    /// and the buttons; it gets the room to be read, the likeliest cause,
    /// and a way to try again.
    fn render_remote_unreachable(
        &self,
        remote: &SharedString,
        error: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let local = ["127.0.0.1", "localhost", "[::1]"]
            .iter()
            .any(|host| error.contains(host));
        let hint = if local {
            "This remote is a port on this machine — a tunnel, or a local runner. Check that \
             it is up, then retry."
        } else {
            "Check that the server is running and reachable from this machine, then retry."
        };
        v_flex()
            .flex_1()
            .p_3()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::Disconnected)
                            .size(IconSize::Small)
                            .color(Color::Error),
                    )
                    .child(Label::new(format!("Can't reach {remote}")).size(LabelSize::Small)),
            )
            .child(
                Label::new(error.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(hint).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .pt_1()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("el-remote-retry", "Retry")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Outlined)
                            .start_icon(Icon::new(IconName::RotateCw).size(IconSize::XSmall))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remote_error = None;
                                this.start_remote_feed(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::CopyButton::new("el-remote-copy-error", error.to_string())
                            .icon_size(IconSize::XSmall)
                            .tooltip_label("Copy error"),
                    ),
            )
            .into_any_element()
    }

    /// What went wrong, between the toolbar and the tables, where it has
    /// the width to be read: a connection that dropped while there is
    /// still something to show, and the last action the server refused.
    fn render_remote_banners(
        &self,
        nothing_held: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut banners = Vec::new();
        if let (false, Some(error)) = (nothing_held, self.remote_error.clone()) {
            banners.push(
                ui::Callout::new()
                    .severity(ui::Severity::Warning)
                    .icon(IconName::Disconnected)
                    .title("Connection lost — this is the last known state")
                    .description(error)
                    .actions_slot(
                        Button::new("el-remote-banner-retry", "Retry")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_remote_feed(cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.remote_action_error.clone() {
            banners.push(
                ui::Callout::new()
                    .severity(ui::Severity::Error)
                    .icon(IconName::XCircle)
                    .title(error)
                    .dismiss_action(
                        IconButton::new("el-remote-banner-dismiss", IconName::Close)
                            .icon_size(IconSize::XSmall)
                            .tooltip(ui::Tooltip::text("Dismiss"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remote_action_error = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        banners
    }

    /// The daemon's own log: one line per thing it did, newest at the
    /// bottom, the problems colored as such and reachable on their own.
    /// Drag its top edge to trade height with the tables above.
    fn render_remote_logs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        // Newest-last: following shows what just happened rather than
        // where the reader last scrolled. The lines that arrived since
        // the last frame have no measured height yet, so this reaches the
        // previous bottom; the frame after, with their heights known,
        // reaches the newest line.
        if self.remote_logs_dirty && self.remote_logs_follow {
            self.remote_logs_dirty = false;
            self.remote_logs_scroll.scroll_to_bottom();
            cx.on_next_frame(window, |this, _, cx| {
                if this.remote_logs_follow {
                    this.remote_logs_scroll.scroll_to_bottom();
                    cx.notify();
                }
            });
        }
        let problems = self
            .remote_logs
            .iter()
            .filter(|line| log_color(line) != Color::Muted)
            .count();
        let shown: Vec<SharedString> = self
            .remote_logs
            .iter()
            .filter(|line| !self.remote_logs_problems_only || log_color(line) != Color::Muted)
            .cloned()
            .collect();
        let copyable = shown
            .iter()
            .map(|line| line.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        let dragging = self.remote_log_drag.is_some();

        let filter_chip = |id: &'static str, label: SharedString, on: bool| {
            Button::new(id, label)
                .label_size(LabelSize::Small)
                .toggle_state(on)
                .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
        };
        let header = h_flex()
            .w_full()
            .px_2()
            .py_0p5()
            .gap_2()
            .items_center()
            .flex_shrink_0()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                Label::new("Server log")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(
                Label::new(format!(
                    "{} line{}",
                    self.remote_logs.len(),
                    if self.remote_logs.len() == 1 { "" } else { "s" }
                ))
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(
                filter_chip("el-remote-log-all", "All".into(), !self.remote_logs_problems_only)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_logs_problems_only = false;
                        this.remote_logs_dirty = true;
                        cx.notify();
                    })),
            )
            .child(
                filter_chip(
                    "el-remote-log-problems",
                    format!("Problems ({problems})").into(),
                    self.remote_logs_problems_only,
                )
                .disabled(problems == 0)
                .tooltip(ui::Tooltip::text(
                    "Only the lines that report something failing",
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.remote_logs_problems_only = true;
                    this.remote_logs_dirty = true;
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("el-remote-log-follow", IconName::ArrowDown)
                    .icon_size(IconSize::XSmall)
                    .toggle_state(self.remote_logs_follow)
                    .tooltip(ui::Tooltip::text(if self.remote_logs_follow {
                        "Following the newest line — click to stay put"
                    } else {
                        "Follow the newest line"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_logs_follow = !this.remote_logs_follow;
                        this.remote_logs_dirty = this.remote_logs_follow;
                        cx.notify();
                    })),
            )
            .child(
                ui::CopyButton::new("el-remote-log-copy", copyable)
                    .icon_size(IconSize::XSmall)
                    .tooltip_label("Copy these lines"),
            )
            .child(
                IconButton::new("el-remote-log-clear", IconName::Eraser)
                    .icon_size(IconSize::XSmall)
                    .tooltip(ui::Tooltip::text(
                        "Clear what's shown here — the server keeps its own log",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_logs.clear();
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("el-remote-log-hide", IconName::Close)
                    .icon_size(IconSize::XSmall)
                    .tooltip(ui::Tooltip::text("Hide the log"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_show_logs = false;
                        cx.notify();
                    })),
            );

        let mut lines = v_flex()
            .id("el-remote-log-lines")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.remote_logs_scroll)
            .px_1()
            .py_0p5();
        if shown.is_empty() {
            lines = lines.child(
                div().px_1().py_1().child(
                    Label::new(if self.remote_logs.is_empty() {
                        "Nothing yet — deploys, runs and schedule fires appear here as the                          server does them."
                    } else {
                        "No problems in what the server has logged."
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            );
        }
        for (ix, line) in shown.iter().enumerate() {
            let color = log_color(line);
            let (stamp, message) = split_log_line(line);
            lines = lines.child(
                h_flex()
                    .group("el-remote-log-line")
                    .w_full()
                    .px_1()
                    .gap_2()
                    .items_start()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .hover(|style| style.bg(colors.element_hover))
                    .child(
                        div().w(px(56.)).flex_shrink_0().child(
                            Label::new(stamp.to_owned())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(message.to_owned()).size(LabelSize::Small).color(
                                if color == Color::Muted {
                                    Color::Default
                                } else {
                                    color
                                },
                            ),
                        ),
                    )
                    .child(
                        ui::CopyButton::new(("el-remote-log-copy-line", ix), message.to_owned())
                            .icon_size(IconSize::XSmall)
                            .tooltip_label("Copy this line")
                            .visible_on_hover("el-remote-log-line"),
                    ),
            );
        }

        v_flex()
            .h(px(self.remote_log_height))
            .flex_shrink_0()
            .bg(colors.editor_background)
            .when(dragging, |pane| {
                pane.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((start_y, start_height)) = this.remote_log_drag {
                        this.remote_log_height =
                            (start_height - (f32::from(event.position.y) - start_y))
                                .clamp(80., 520.);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.remote_log_drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.remote_log_drag = None;
                        cx.notify();
                    }),
                )
            })
            .child(
                // The pane's top edge doubles as its resize handle.
                div()
                    .id("el-remote-log-split")
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
                            this.remote_log_drag =
                                Some((f32::from(event.position.y), this.remote_log_height));
                            cx.notify();
                        }),
                    ),
            )
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(lines)
                    .custom_scrollbars(super::scrollbar(&self.remote_logs_scroll), window, cx),
            )
            .into_any_element()
    }

    /// The overview: pipelines as a striped table (click a row for its
    /// detail view), recent runs below.
    fn render_remote_overview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        // Cloned: the rows hold listeners that borrow `self` mutably.
        let deployed = self.remote_pipelines.clone();
        // A column nobody fills is noise: "manual", "—" and "default" down
        // three rows pushed the run and its button off to the far edge.
        let any_schedule = deployed.iter().any(|pipeline| pipeline.schedule.is_some());
        let any_profile = deployed.iter().any(|pipeline| pipeline.profile.is_some());
        let slot = |width: f32| div().w(px(width)).flex_shrink_0().overflow_hidden();
        let pipeline_header = h_flex()
            .w_full()
            .h(px(26.))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(div().w(px(16.)).flex_shrink_0())
            .child(column_head(200., "pipeline"))
            .child(column_head(64., "streams"))
            .when(any_schedule, |header| header.child(column_head(200., "schedule")))
            .when(any_profile, |header| header.child(column_head(90., "profile")))
            .child(column_head(110., "deployed"))
            .child(column_head(200., "last run"))
            .child(column_head(80., "now"));
        let mut pipelines = v_flex().w_full().px_1();
        let mut stream_rows = 0usize;
        for (ix, pipeline) in deployed.iter().enumerate() {
            let run_name = pipeline.name.clone();
            let expanded = self.remote_expanded.contains(&pipeline.name);
            let toggle_name = pipeline.name.clone();
            // The run to stop, when one of this pipeline's is in flight.
            let in_flight = self
                .remote_runs
                .iter()
                .find(|run| run.pipeline == pipeline.name && run.status == "running")
                .map(|run| (run.id, run.started_unix));
            // How the last finished run went. The list is newest first, and
            // only as long as the server's memory: after a restart there is
            // a time and no outcome, which is what gets shown.
            let last_finished = self
                .remote_runs
                .iter()
                .find(|run| run.pipeline == pipeline.name && run.status != "running")
                .cloned();
            let open_name: SharedString = pipeline.name.clone().into();
            let cell_id = |col: &str| {
                SharedString::from(format!("el-remote-pipeline-{col}-{}", pipeline.name))
            };

            let last_run_cell = match (&last_finished, pipeline.last_run_unix) {
                (Some(run), _) => {
                    let (icon, color) = status_mark(&run.status);
                    let title = format!(
                        "Run #{} {} — {} · {} rows",
                        run.id,
                        run.status,
                        duration_text(run.started_unix, run.finished_unix),
                        super::thousands(run.rows_written as usize),
                    );
                    let meta = match &run.error {
                        Some(error) if *error != run.status => {
                            format!("Started {}\n{error}", absolute_time(run.started_unix))
                        }
                        _ => format!("Started {}", absolute_time(run.started_unix)),
                    };
                    slot(200.).child(
                        h_flex()
                            .id(cell_id("last"))
                            .gap_1()
                            .items_center()
                            .child(Icon::new(icon).size(IconSize::Small).color(color))
                            .child(
                                Label::new(run.status.clone())
                                    .size(LabelSize::Small)
                                    .color(color),
                            )
                            .child(
                                Label::new(relative_time(run.started_unix, false))
                                    .size(LabelSize::Small)
                                    .truncate(),
                            )
                            .tooltip(move |_window, cx| {
                                ui::Tooltip::with_meta(title.clone(), None, meta.clone(), cx)
                            }),
                    )
                }
                (None, Some(at)) => slot(200.).child(
                    div()
                        .id(cell_id("last"))
                        .child(
                            Label::new(relative_time(at, false))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .tooltip(ui::Tooltip::text(format!(
                            "Started {} — the server no longer holds how it went",
                            absolute_time(at)
                        ))),
                ),
                (None, None) => slot(200.).child(
                    Label::new("never")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            };

            // What it is doing this second, without opening the run: the
            // phase, the rows, and how long it has been at it.
            let now_cell = match in_flight {
                Some((run_id, started)) => {
                    let doing = live_progress(self.run_events(run_id))
                        .map(|progress| progress.summary())
                        .unwrap_or_else(|| "running".to_owned());
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .items_center()
                        .child(live_dot(("el-remote-live", ix), Color::Accent))
                        .child(
                            Label::new(format!("{doing} · {}", elapsed_text(started)))
                                .size(LabelSize::Small)
                                .color(Color::Accent)
                                .truncate(),
                        )
                }
                None => h_flex().flex_1().min_w_0().child(
                    Label::new("idle")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            };

            let deployed_tip = match (pipeline.deployed_unix, pipeline.created_unix) {
                (Some(deployed), Some(created)) if created != deployed => format!(
                    "Deployed {} — first deployed {}",
                    absolute_time(deployed),
                    absolute_time(created)
                ),
                (Some(deployed), _) => format!("Deployed {}", absolute_time(deployed)),
                _ => "This server does not say when it was deployed".to_owned(),
            };

            pipelines = pipelines.child(
                h_flex()
                    .id(("el-remote-pipeline", ix))
                    .w_full()
                    .h(px(32.))
                    .px_1()
                    .gap_2()
                    .items_center()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .hover(|style| style.bg(colors.element_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_detail = Some(open_name.clone());
                        this.remote_stream_picks.clear();
                        cx.notify();
                    }))
                    .child(
                        // Streams unfold in place, so one table can be
                        // launched without leaving the list.
                        div().w(px(16.)).flex_shrink_0().children(
                            (!pipeline.stream_info.is_empty()).then(|| {
                                IconButton::new(
                                    ("el-remote-expand", ix),
                                    if expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    },
                                )
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(ui::Tooltip::text(if expanded {
                                    "Hide the streams"
                                } else {
                                    "Show the streams, to run one on its own"
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if !this.remote_expanded.remove(&toggle_name) {
                                        this.remote_expanded.insert(toggle_name.clone());
                                    }
                                    cx.notify();
                                }))
                            }),
                        ),
                    )
                    .child(
                        // The name is what a row is for: one size up, and
                        // the only full-strength text on it.
                        slot(200.).child(
                            div()
                                .id(cell_id("name"))
                                .tooltip(ui::Tooltip::text(format!(
                                    "{} — open its streams and runs",
                                    pipeline.name
                                )))
                                .child(
                                    Label::new(pipeline.name.clone())
                                        .size(LabelSize::Default)
                                        .truncate(),
                                ),
                        ),
                    )
                    .child(
                        slot(64.).child(
                            Label::new(pipeline.streams.to_string()).size(LabelSize::Small),
                        ),
                    )
                    .when(any_schedule, |row| {
                        let (text, tip) = match (&pipeline.schedule, pipeline.next_run_unix) {
                            (Some(schedule), Some(next)) => (
                                format!("{schedule} · {}", relative_time(next, true)),
                                format!(
                                    "Next run {}{}",
                                    absolute_time(next),
                                    pipeline
                                        .timezone
                                        .as_ref()
                                        .map(|zone| format!(" — schedule in {zone}"))
                                        .unwrap_or_default()
                                ),
                            ),
                            (Some(schedule), None) => (schedule.clone(), schedule.clone()),
                            (None, _) => (
                                "manual".to_owned(),
                                "No schedule: it runs when someone presses Run".to_owned(),
                            ),
                        };
                        row.child(
                            slot(200.).child(
                                div()
                                    .id(cell_id("schedule"))
                                    .tooltip(ui::Tooltip::text(tip))
                                    .child(
                                        Label::new(text)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate(),
                                    ),
                            ),
                        )
                    })
                    .when(any_profile, |row| {
                        row.child(tip_cell(
                            cell_id("profile"),
                            90.,
                            pipeline.profile.clone().unwrap_or_else(|| "server's".into()),
                            if pipeline.profile.is_some() {
                                Color::Accent
                            } else {
                                Color::Muted
                            },
                        ))
                    })
                    .child(
                        slot(110.).child(
                            div()
                                .id(cell_id("deployed"))
                                .tooltip(ui::Tooltip::text(deployed_tip))
                                .child(
                                    Label::new(
                                        pipeline
                                            .deployed_unix
                                            .map(|at| relative_time(at, false))
                                            .unwrap_or_else(|| "—".into()),
                                    )
                                    .size(LabelSize::Small)
                                    .truncate(),
                                ),
                        ),
                    )
                    .child(last_run_cell)
                    .child(now_cell)
                    .children(in_flight.map(|(run_id, _)| {
                        Button::new(("el-remote-cancel", ix), "Cancel")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Tinted(ui::TintColor::Warning))
                            .start_icon(Icon::new(IconName::Stop).size(IconSize::XSmall))
                            .tooltip(ui::Tooltip::text(
                                "Stop the run in progress, and the query it has open on the source",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remote_action(move |client| client.cancel(run_id), cx);
                            }))
                    }))
                    .children(in_flight.is_none().then(|| {
                        Button::new(("el-remote-run", ix), "Run")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Outlined)
                            .start_icon(Icon::new(IconName::PlayOutlined).size(IconSize::XSmall))
                            .disabled(pipeline.running)
                            .tooltip(ui::Tooltip::text("Run every stream of this pipeline"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.run_remote(run_name.clone(), Vec::new(), cx);
                            }))
                    })),
            );
            if expanded {
                for (stream_ix, stream) in pipeline.stream_info.iter().enumerate() {
                    stream_rows += 1;
                    pipelines = pipelines.child(self.render_stream_row(
                        "el-remote-list-stream",
                        &pipeline.name,
                        stream,
                        stream_ix,
                        false,
                        pipeline.running,
                        cx,
                    ));
                }
            }
        }

        // As tall as what it holds, until the operator says otherwise.
        let natural = 26. + 32. * deployed.len() as f32 + 28. * stream_rows as f32 + 8.;
        let shown = if self.remote_split_by_hand {
            self.remote_split
        } else {
            natural.clamp(56., 320.)
        };
        self.remote_split_shown = shown;
        let run_count = self.remote_runs.len();

        let dragging = self.remote_split_drag.is_some();
        v_flex()
            .flex_1()
            .min_h_0()
            .when(dragging, |flex| {
                flex.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((start_y, start_split)) = this.remote_split_drag {
                        this.remote_split =
                            (start_split + f32::from(event.position.y) - start_y).clamp(56., 480.);
                        this.remote_split_by_hand = true;
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
                div()
                    .h(px(shown))
                    .flex_shrink_0()
                    .child(
                        v_flex()
                            .id("el-remote-pipelines-pane")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.remote_pipelines_scroll)
                            .child(pipeline_header)
                            .child(pipelines),
                    )
                    .custom_scrollbars(super::scrollbar(&self.remote_pipelines_scroll), window, cx),
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
                    .tooltip(ui::Tooltip::text(
                        "Drag to resize — double-click to fit the pipelines again",
                    ))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count >= 2 {
                                this.remote_split_by_hand = false;
                                this.remote_split_drag = None;
                            } else {
                                this.remote_split_drag =
                                    Some((f32::from(event.position.y), this.remote_split_shown));
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                h_flex()
                    .px_2()
                    .pt_1()
                    .pb_0p5()
                    .gap_1p5()
                    .items_center()
                    .child(Label::new("Recent runs").size(LabelSize::Default))
                    .when(run_count > 0, |title| {
                        title.child(
                            Label::new(run_count.to_string())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(self.render_runs_table(None, window, cx))
            .into_any_element()
    }

    /// One pipeline's page: back button, its facts, its runs.
    fn render_remote_detail(
        &mut self,
        name: &SharedString,
        window: &mut Window,
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
        // Ticked streams, in the pipeline's own order rather than the
        // order they happened to be clicked in.
        let picked: Vec<String> = pipeline
            .map(|pipeline| {
                pipeline
                    .stream_info
                    .iter()
                    .map(|stream| stream.name.clone())
                    .filter(|name| self.remote_stream_picks.contains(name))
                    .collect()
            })
            .unwrap_or_default();
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
                        this.remote_stream_picks.clear();
                        cx.notify();
                    })),
            )
            .child(Label::new(name.clone()).size(LabelSize::Default))
            .child(Label::new(meta).size(LabelSize::Small).color(Color::Muted))
            .child(div().flex_1())
            .children(running.then(|| {
                Label::new("running")
                    .size(LabelSize::Small)
                    .color(Color::Accent)
            }))
            .children(picked.is_empty().then(|| {
                Button::new("el-remote-detail-run", "Run pipeline")
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .start_icon(Icon::new(IconName::PlayFilled).size(IconSize::XSmall))
                    .disabled(running || pipeline.is_none())
                    .tooltip(ui::Tooltip::text("Run every stream"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_remote(run_name.clone(), Vec::new(), cx);
                    }))
            }))
            .children((!picked.is_empty()).then(|| {
                let picked_names = picked.clone();
                let picked_pipeline = name.to_string();
                Button::new(
                    "el-remote-detail-run-picked",
                    format!(
                        "Run {} stream{}",
                        picked.len(),
                        if picked.len() == 1 { "" } else { "s" }
                    ),
                )
                .label_size(LabelSize::Small)
                .style(ButtonStyle::Filled)
                .start_icon(Icon::new(IconName::PlayFilled).size(IconSize::XSmall))
                .disabled(running)
                .tooltip(ui::Tooltip::text(format!(
                    "Run only: {}",
                    picked_names.join(", ")
                )))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let picked = picked_names.clone();
                    this.run_remote(picked_pipeline.clone(), picked, cx);
                }))
            }));

        // The streams as a table: tick several and send them in one run,
        // or run one on its own from its row.
        let streams = pipeline
            .map(|pipeline| pipeline.stream_info.clone())
            .unwrap_or_default();
        let picked_count = picked.len();
        let streams_block: gpui::AnyElement = if streams.is_empty() {
            div()
                .px_2()
                .pt_1()
                .child(
                    Label::new(match pipeline {
                        Some(pipeline) if pipeline.streams > 0 => {
                            "This server doesn't report a pipeline's streams — update the \
                             daemon to run them one at a time."
                        }
                        Some(_) => "This pipeline has no streams.",
                        None => "",
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            let all_picked = picked_count == streams.len();
            let every_name: Vec<String> =
                streams.iter().map(|stream| stream.name.clone()).collect();
            let mut rows = v_flex()
                .id("el-remote-detail-streams")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.remote_streams_scroll)
                .px_1();
            for (ix, stream) in streams.iter().enumerate() {
                rows = rows.child(self.render_stream_row(
                    "el-remote-detail-stream",
                    name.as_ref(),
                    stream,
                    ix,
                    true,
                    running,
                    cx,
                ));
            }
            v_flex()
                .flex_shrink_0()
                .max_h(px(200.))
                .child(
                    h_flex()
                        .w_full()
                        .px_2()
                        .pt_1()
                        .gap_2()
                        .items_center()
                        .child(
                            Label::new(format!("Streams ({})", streams.len()))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new(
                                "el-remote-pick-all",
                                if all_picked { "Clear" } else { "Select all" },
                            )
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote_stream_picks.clear();
                                if !all_picked {
                                    this.remote_stream_picks.extend(every_name.iter().cloned());
                                }
                                cx.notify();
                            })),
                        ),
                )
                .child(self.stream_header(true))
                .child(div().flex_1().min_h_0().child(rows).custom_scrollbars(
                    super::scrollbar(&self.remote_streams_scroll),
                    window,
                    cx,
                ))
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(streams_block)
            .child(
                div().px_2().pt_1().child(
                    Label::new("Runs of this pipeline")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(self.render_runs_table(Some(name.as_ref()), window, cx))
            .into_any_element()
    }

    /// One run's page: verdict facts and the per-stream breakdown
    /// reconstructed from its event log (live while it runs).
    fn render_remote_run_detail(
        &mut self,
        run_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        // Cloned: measuring the rate below wants `self` mutably, and one
        // run's facts are a handful of fields.
        let run_snapshot = self
            .remote_runs
            .iter()
            .find(|run| run.id == run_id)
            .cloned();
        let run = run_snapshot.as_ref();
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
        // Fold the event log into per-stream rows.
        let (order, agg) = fold_stream_events(self.run_events(run_id));
        // What a re-run would be for: the streams that reported an error,
        // and the ones a stopped run never finished.
        let failed_streams: Vec<String> = order
            .iter()
            .filter(|stream| {
                agg.get(*stream)
                    .is_some_and(|entry| entry.error.is_some() || !entry.done)
            })
            .cloned()
            .collect();
        let all_errored = failed_streams
            .iter()
            .all(|stream| agg.get(stream).is_some_and(|entry| entry.error.is_some()));
        let pipeline_name = run.map(|run| run.pipeline.clone()).unwrap_or_default();
        // The run's own rate, measured the same way as each stream's, so
        // the header says how fast this is going and not merely how long
        // it has been going.
        let moved: u64 = agg.values().map(|entry| entry.written).sum();
        let run_rate = self.sample_rate(run_id, "", moved, !is_running);

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
                if let Some(rate) = run_rate {
                    text.push_str(&format!(" · {}", rate_text(rate)));
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
                    .size(LabelSize::Small)
                    .color(status_color),
            )
            .child(
                Label::new(facts)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .children(is_running.then(|| {
                Button::new("el-run-detail-cancel", "Cancel")
                    .label_size(LabelSize::Small)
                    .color(Color::Warning)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_action(move |client| client.cancel(run_id), cx);
                    }))
            }))
            // What a failed run is usually followed by: the streams that
            // failed, again, without the ones that already landed.
            .children((!is_running && !failed_streams.is_empty()).then(|| {
                let pipeline = run
                    .map(|run| run.pipeline.clone())
                    .unwrap_or_default();
                let streams = failed_streams.clone();
                Button::new(
                    "el-run-detail-retry",
                    format!(
                        "Re-run {} {} stream{}",
                        streams.len(),
                        if all_errored { "failed" } else { "unfinished" },
                        if streams.len() == 1 { "" } else { "s" }
                    ),
                )
                .label_size(LabelSize::Small)
                .style(ButtonStyle::Filled)
                .start_icon(Icon::new(IconName::RotateCw).size(IconSize::XSmall))
                .tooltip(ui::Tooltip::text(format!("Runs only: {}", streams.join(", "))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_remote(pipeline.clone(), streams.clone(), cx);
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
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .id("el-run-detail-error")
                                .max_h(px(96.))
                                .pr_3()
                                .overflow_y_scroll()
                                .track_scroll(&self.run_error_scroll)
                                .child(
                                    Label::new(error.clone())
                                        .size(LabelSize::Small)
                                        .color(Color::Error),
                                ),
                        )
                        .custom_scrollbars(super::scrollbar(&self.run_error_scroll), window, cx),
                )
                .child(
                    ui::CopyButton::new("el-run-detail-copy-error", error)
                        .icon_size(IconSize::XSmall)
                        .tooltip_label("Copy error"),
                )
        });

        let head = column_head;
        let stream_header = h_flex()
            .w_full()
            .px_2()
            .gap_2()
            .child(head(180., "stream"))
            .child(head(80., "phase"))
            .child(head(140., "progress"))
            .child(head(110., "rows read"))
            .child(head(110., "rows written"))
            .child(head(90., "rows/min"))
            .child(head(84., "cast fails"))
            .child(
                Label::new("status")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        let mut rows = v_flex()
            .id("el-run-streams")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.run_streams_scroll)
            .px_1();
        if order.is_empty() {
            rows = rows.child(
                div().px_1().py_1().child(
                    Label::new("Waiting for the run's events…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            );
        }
        let cell = |width: f32, text: String, color: Color| {
            div().w(px(width)).flex_shrink_0().overflow_hidden().child(
                Label::new(text)
                    .size(LabelSize::Small)
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
            let estimated = entry.and_then(|entry| entry.estimated);
            // Measured, not averaged since the start: a stall shows up.
            let rate = self.sample_rate(run_id, stream, written, done);
            let progress_cell: gpui::AnyElement = match estimated {
                Some(total) if total > 0 => {
                    let fraction = (read as f64 / total as f64).clamp(0., 1.);
                    let eta = (!done)
                        .then(|| rate.and_then(|rate| eta_text(total.saturating_sub(read), rate)))
                        .flatten();
                    let tip = format!(
                        "{} of about {} rows{}",
                        super::thousands(read as usize),
                        super::thousands(total as usize),
                        eta.map(|eta| format!(" · {eta}")).unwrap_or_default()
                    );
                    div()
                        .id(cell_id("progress"))
                        .w(px(140.))
                        .flex_shrink_0()
                        .tooltip(ui::Tooltip::text(tip))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(
                                    // The track has to be visible empty,
                                    // or a run that has just started looks
                                    // like a stray dot; and the fill needs
                                    // a floor, or one percent is invisible.
                                    div()
                                        .w(px(84.))
                                        .h(px(8.))
                                        .rounded_sm()
                                        .bg(colors.element_background)
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(
                                            div()
                                                .h_full()
                                                .min_w(px(3.))
                                                .w(gpui::relative(fraction as f32))
                                                .rounded_sm()
                                                .bg(if done {
                                                    colors.text_accent
                                                } else {
                                                    colors.text_accent.opacity(0.85)
                                                }),
                                        ),
                                )
                                .child(
                                    Label::new(format!("{:.0}%", fraction * 100.))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                        .into_any_element()
                }
                // A source that cannot say how much it holds gets no bar
                // rather than a made-up one.
                _ => cell(140., "—".to_owned(), Color::Muted).into_any_element(),
            };
            let casts_cell: gpui::AnyElement = if casts > 0 && !failures.is_empty() {
                let stream_name = stream.clone();
                div()
                    .w(px(84.))
                    .flex_shrink_0()
                    .child(
                        Button::new(("el-remote-casts", ix), casts.to_string())
                            .label_size(LabelSize::Small)
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
                    .w(px(70.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .tooltip(ui::Tooltip::text(why))
                    .child(
                        Label::new(casts.to_string())
                            .size(LabelSize::Small)
                            .color(Color::Warning)
                            .truncate(),
                    )
                    .into_any_element()
            } else {
                cell(84., casts.to_string(), Color::Muted).into_any_element()
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
                    .h(px(28.))
                    .px_1()
                    .gap_2()
                    .items_center()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .child(tip_cell(
                        cell_id("name"),
                        180.,
                        stream.clone(),
                        Color::Default,
                    ))
                    .child(cell(80., phase.unwrap_or_else(|| "—".into()), Color::Default))
                    .child(progress_cell)
                    .child(cell(110., super::thousands(read as usize), Color::Default))
                    .child(cell(
                        110.,
                        super::thousands(written as usize),
                        Color::Default,
                    ))
                    .child(cell(
                        90.,
                        rate.map(rate_text).unwrap_or_else(|| "—".to_owned()),
                        if rate.is_some() {
                            Color::Accent
                        } else {
                            Color::Muted
                        },
                    ))
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
                            }))
                            .child({
                                // Re-running one stream is how a run that
                                // half-failed gets finished.
                                let pipeline = pipeline_name.clone();
                                let stream = stream.clone();
                                IconButton::new(cell_id("rerun"), IconName::PlayOutlined)
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(Color::Muted)
                                    .disabled(is_running)
                                    .tooltip(ui::Tooltip::text(format!(
                                        "Run {stream} again, on its own"
                                    )))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.run_remote(
                                            pipeline.clone(),
                                            vec![stream.clone()],
                                            cx,
                                        );
                                    }))
                            }),
                    ),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .children(error_block)
            .child(stream_header)
            .child(div().flex_1().min_h_0().child(rows).custom_scrollbars(
                super::scrollbar(&self.run_streams_scroll),
                window,
                cx,
            ))
            .into_any_element()
    }

    /// The run-history table, striped; `filter` narrows to one pipeline.
    fn render_runs_table(
        &mut self,
        filter: Option<&str>,
        window: &mut Window,
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
        let show_pipeline = filter.is_none();
        let listed = |run: &&el_engine::server::RemoteRun| {
            filter.is_none_or(|name| run.pipeline == name)
        };
        // Columns that would be a dash on every line stay away until a run
        // gives them something to say.
        let any_casts = self
            .remote_runs
            .iter()
            .filter(listed)
            .any(|run| run.cast_failures != Some(0));
        let any_retry = self
            .remote_runs
            .iter()
            .filter(listed)
            .any(|run| run.attempt > 0);
        let header = h_flex()
            .w_full()
            .h(px(26.))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(column_head(52., "run"))
            .when(show_pipeline, |header| header.child(column_head(170., "pipeline")))
            .child(column_head(190., "status"))
            .child(column_head(120., "started"))
            .child(column_head(90., "took"))
            .child(column_head(100., "rows"))
            .when(any_casts, |header| header.child(column_head(84., "cast fails")))
            .when(any_retry, |header| header.child(column_head(70., "attempt")))
            .child(
                Label::new("error")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

        let mut runs = v_flex()
            .id("el-remote-runs")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.remote_runs_scroll)
            .px_1();
        let mut shown = 0usize;
        for run in &self.remote_runs {
            if filter.is_some_and(|name| run.pipeline != name) {
                continue;
            }
            let stripe = shown % 2 == 1;
            shown += 1;
            let run_id = run.id;
            let is_running = run.status == "running";
            let progress = is_running
                .then(|| live_progress(self.run_events(run_id)))
                .flatten();
            let cell = |width: f32, text: String, color: Color| {
                col(
                    width,
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(color)
                        .truncate()
                        .into_any_element(),
                )
            };
            // Status: a mark and a word. A run in flight says what it is
            // doing instead of the word "running", which the dot already is.
            let status_cell = if is_running {
                let doing = progress
                    .as_ref()
                    .and_then(|progress| progress.phase.clone())
                    .unwrap_or_else(|| "running".to_owned());
                col(
                    190.,
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(live_dot(("el-run-live", run.id as usize), Color::Accent))
                        .child(
                            Label::new(doing)
                                .size(LabelSize::Small)
                                .color(Color::Accent)
                                .truncate(),
                        )
                        .into_any_element(),
                )
            } else {
                let (icon, color) = status_mark(&run.status);
                col(
                    190.,
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(Icon::new(icon).size(IconSize::Small).color(color))
                        .child(
                            Label::new(run.status.clone())
                                .size(LabelSize::Small)
                                .color(color),
                        )
                        .into_any_element(),
                )
            };
            let rows_text = match (&progress, is_running) {
                (Some(progress), _) if progress.rows_written > 0 => {
                    super::thousands(progress.rows_written as usize)
                }
                (Some(progress), _) => format!("{} read", super::thousands(progress.rows_read as usize)),
                (None, true) => "…".to_owned(),
                (None, false) => super::thousands(run.rows_written as usize),
            };
            // "cancelled — cancelled" says nothing twice.
            let error_text = run
                .error
                .clone()
                .filter(|error| *error != run.status)
                .unwrap_or_default();
            let mut row = h_flex()
                .id(("el-run-row", run.id as usize))
                .w_full()
                .h(px(30.))
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
                .child(cell(52., format!("#{}", run.id), Color::Muted));
            if show_pipeline {
                row = row.child(tip_cell(
                    ("el-run-pipeline", run.id as usize),
                    170.,
                    run.pipeline.clone(),
                    Color::Default,
                ));
            }
            runs = runs.child(
                row.child(status_cell)
                    .child(col(
                        120.,
                        div()
                            .id(("el-run-started", run.id as usize))
                            .tooltip(ui::Tooltip::text(absolute_time(run.started_unix)))
                            .child(
                                Label::new(relative_time(run.started_unix, false))
                                    .size(LabelSize::Small)
                                    .truncate(),
                            )
                            .into_any_element(),
                    ))
                    .child(cell(
                        90.,
                        if is_running {
                            elapsed_text(run.started_unix)
                        } else {
                            duration_text(run.started_unix, run.finished_unix)
                        },
                        if is_running { Color::Accent } else { Color::Default },
                    ))
                    .child(cell(
                        100.,
                        rows_text,
                        if is_running { Color::Accent } else { Color::Default },
                    ))
                    .when(any_casts, |row| {
                        row.child(match run.cast_failures {
                            // An older daemon reports no count at all — say so
                            // rather than passing it off as a clean run.
                            None => col(
                                84.,
                                div()
                                    .id(("el-run-casts", run.id as usize))
                                    .tooltip(ui::Tooltip::text(
                                        "This server doesn't report cast failures — update the daemon to see them",
                                    ))
                                    .child(
                                        Label::new("?")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .into_any_element(),
                            ),
                            Some(0) => cell(84., "—".to_owned(), Color::Muted),
                            Some(count) => cell(
                                84.,
                                super::thousands(count as usize),
                                Color::Warning,
                            ),
                        })
                    })
                    .when(any_retry, |row| {
                        row.child(cell(
                            70.,
                            if run.attempt == 0 {
                                "—".to_owned()
                            } else {
                                format!("retry {}", run.attempt)
                            },
                            if run.attempt == 0 { Color::Muted } else { Color::Warning },
                        ))
                    })
                    .child(
                        tip_text(
                            ("el-run-error", run.id as usize),
                            error_text.clone(),
                            Color::Error,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .children((!error_text.is_empty()).then(|| {
                        ui::CopyButton::new(
                            ("el-run-copy-error", run.id as usize),
                            error_text.clone(),
                        )
                        .icon_size(IconSize::XSmall)
                        .tooltip_label("Copy error")
                    }))
                    .children(is_running.then(|| {
                        Button::new(("el-remote-cancel", run.id as usize), "Cancel")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Tinted(ui::TintColor::Warning))
                            .start_icon(Icon::new(IconName::Stop).size(IconSize::XSmall))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
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
                h_flex()
                    .px_1()
                    .py_2()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::PlayOutlined)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(if filter.is_some() {
                            "This pipeline has not run here yet — Run starts it."
                        } else {
                            "Nothing has run on this server since it started — press Run on a pipeline."
                        })
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().child(runs).custom_scrollbars(
                super::scrollbar(&self.remote_runs_scroll),
                window,
                cx,
            ))
            .into_any_element()
    }
}

/// A column title. Muted, not accent: the accent is the loudest colour
/// the panel has and was being spent on its least important words — and
/// lowercase blue text reads as a link, which a column title is not.
fn column_head(width: f32, text: &'static str) -> gpui::Div {
    div().w(px(width)).flex_shrink_0().child(
        Label::new(text)
            .size(LabelSize::Small)
            .color(Color::Muted),
    )
}

/// How a run's status is drawn: a mark and a colour, never the colour
/// alone. "ok" in green and "cancelled" in orange are the same grey word
/// to someone who cannot tell the two apart.
fn status_mark(status: &str) -> (IconName, Color) {
    match status {
        "ok" => (IconName::Check, Color::Success),
        "failed" => (IconName::XCircle, Color::Error),
        "cancelled" => (IconName::Stop, Color::Warning),
        _ => (IconName::Circle, Color::Muted),
    }
}

/// A dot that breathes: something is happening right now. The one moving
/// thing on the panel, and only while a run is in flight.
fn live_dot(id: impl Into<gpui::ElementId>, color: Color) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .child(ui::Indicator::dot().color(color))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.)),
            |dot, delta| dot.opacity(delta),
        )
}

/// A unix instant as the local wall clock reads it, for hover text: "3h
/// ago" is quick to read and useless for lining a run up with a log.
fn absolute_time(unix: u64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_opt(unix as i64, 0).single() {
        Some(at) => at.format("%a %-d %b %Y, %H:%M:%S").to_string(),
        None => format!("unix {unix}"),
    }
}

/// How long something that started at `started` has been going.
fn elapsed_text(started: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    span_text(now.saturating_sub(started))
}

/// A span of seconds in the two units that matter at its size.
fn span_text(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// What a run in flight is doing, folded from its events: the phase of
/// the stream being moved, the rows landed so far, and how many streams
/// are through. None until the first event arrives — and for a server
/// that is only being polled, which sends no events unasked.
fn live_progress(events: &[el_engine::ProgressEvent]) -> Option<LiveProgress> {
    let (order, streams) = fold_stream_events(events);
    if order.is_empty() {
        return None;
    }
    let current = order
        .iter()
        .filter_map(|name| streams.get(name))
        .find(|stream| !stream.done && stream.error.is_none() && stream.phase.is_some());
    Some(LiveProgress {
        phase: current.and_then(|stream| stream.phase.clone()),
        rows_read: streams.values().map(|stream| stream.read).sum(),
        rows_written: streams.values().map(|stream| stream.written).sum(),
        streams_done: streams.values().filter(|stream| stream.done).count(),
        streams_total: order.len(),
    })
}

struct LiveProgress {
    phase: Option<String>,
    rows_read: u64,
    rows_written: u64,
    streams_done: usize,
    streams_total: usize,
}

impl LiveProgress {
    /// "extract · 8,412 read" while nothing has landed, "stage · 1,200,000
    /// rows" once it has; with "· 2/10 streams" when there is more than one.
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(phase) = &self.phase {
            parts.push(phase.clone());
        }
        if self.rows_written > 0 {
            parts.push(format!("{} rows", super::thousands(self.rows_written as usize)));
        } else {
            parts.push(format!("{} read", super::thousands(self.rows_read as usize)));
        }
        if self.streams_total > 1 {
            parts.push(format!("{}/{} streams", self.streams_done, self.streams_total));
        }
        parts.join(" · ")
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
                .size(LabelSize::Small)
                .color(color)
                .truncate(),
        )
}

/// A cell's value on one line, since grid rows are one line tall: runs of
/// whitespace — the newlines of a JSON VARIANT or a CLOB — become single
/// spaces, and a very long value is cut. A click on the cell shows it whole.
fn one_line(value: &str) -> SharedString {
    const MAX_CHARS: usize = 300;
    let folded = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() > MAX_CHARS {
        format!("{}…", folded.chars().take(MAX_CHARS).collect::<String>()).into()
    } else {
        folded.into()
    }
}

/// [`tip_text`] at a fixed column width.
/// Rows per minute as text, in the unit that reads best at that size.
fn rate_text(rows_per_minute: f64) -> String {
    if rows_per_minute >= 1_000_000. {
        format!("{:.1}M/min", rows_per_minute / 1_000_000.)
    } else if rows_per_minute >= 1_000. {
        format!("{:.0}k/min", rows_per_minute / 1_000.)
    } else {
        format!("{rows_per_minute:.0}/min")
    }
}

/// How long `rows` more take at `rows_per_minute`, as text. None when
/// nothing is moving — a countdown from a stalled rate is a lie.
fn eta_text(remaining: u64, rows_per_minute: f64) -> Option<String> {
    if rows_per_minute <= 0. {
        return None;
    }
    let minutes = remaining as f64 / rows_per_minute;
    Some(if minutes >= 90. {
        format!("{:.1} h left", minutes / 60.)
    } else if minutes >= 1.5 {
        format!("{minutes:.0} min left")
    } else {
        format!("{:.0} s left", minutes * 60.)
    })
}

/// A daemon log line as the daemon writes it: "HH:MM:SS  what it did".
/// Anything else is left whole rather than guessed at.
fn split_log_line(line: &str) -> (&str, &str) {
    match line.split_once("  ") {
        Some((stamp, rest))
            if stamp.len() == 8
                && stamp
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ':') =>
        {
            (stamp, rest.trim_start())
        }
        _ => ("", line),
    }
}

/// What a log line is worth noticing as. Problems are what a person
/// scans a server log for; everything else is ordinary progress and
/// stays quiet.
fn log_color(line: &str) -> Color {
    let line = line.to_ascii_lowercase();
    const FAILURES: [&str; 8] = [
        "failed",
        "error",
        "refused",
        "not delivered",
        "not run",
        "panic",
        "cannot",
        "can't",
    ];
    const WARNINGS: [&str; 5] = ["cancelled", "skipped", "retry", "warning", "exited"];
    if FAILURES.iter().any(|word| line.contains(word)) {
        Color::Error
    } else if WARNINGS.iter().any(|word| line.contains(word)) {
        Color::Warning
    } else {
        Color::Muted
    }
}

fn tip_cell(
    id: impl Into<gpui::ElementId>,
    width: f32,
    text: String,
    color: Color,
) -> gpui::Stateful<gpui::Div> {
    tip_text(id, text, color).w(px(width)).flex_shrink_0()
}

/// The query a clicked table opens: its first rows, on the connection's
/// own terms. Oracle has no LIMIT — ROWNUM works on every version (FETCH
/// FIRST only from 12c, and the thick driver reaches 10g / 11g). A
/// Snowflake schema arrives as DATABASE.SCHEMA, each part its own
/// identifier, and a table with no schema is read by its bare name.
fn select_sql(kind: &str, schema: Option<&str>, table: &str) -> String {
    let quote = |ident: &str| match kind {
        // SQL Server brackets its identifiers; a double quote only works
        // there when QUOTED_IDENTIFIER is on, which is not ours to assume.
        "mssql" => format!("[{}]", ident.replace(']', "]]")),
        _ => format!("\"{}\"", ident.replace('"', "\"\"")),
    };
    let (head, cap) = match kind {
        "oracle" => ("SELECT *", "WHERE ROWNUM <= 200"),
        "mssql" => ("SELECT TOP 200 *", ""),
        _ => ("SELECT *", "LIMIT 200"),
    };
    let qualified = match schema.filter(|schema| !schema.is_empty()) {
        None => quote(table),
        Some(schema) => {
            let schema = match schema.split_once('.') {
                Some((database, schema)) if kind == "snowflake" => {
                    format!("{}.{}", quote(database), quote(schema))
                }
                _ => quote(schema),
            };
            format!("{schema}.{}", quote(table))
        }
    };
    format!("{head} FROM {qualified} {cap}")
        .trim_end()
        .to_owned()
}

/// A new table starts at its top-left corner.
fn scroll_home(scroll: &UniformListScrollHandle, h_scroll: &ScrollHandle) {
    scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
    h_scroll.set_offset(gpui::Point::default());
}

impl ElRunsPanel {
    fn grid(&self, slot: GridSlot) -> Option<&ConsoleGrid> {
        match slot {
            GridSlot::Preview => self.preview.as_ref(),
            GridSlot::Result => self.result.as_ref(),
        }
    }

    fn grid_mut(&mut self, slot: GridSlot) -> Option<&mut ConsoleGrid> {
        match slot {
            GridSlot::Preview => self.preview.as_mut(),
            GridSlot::Result => self.result.as_mut(),
        }
    }

    /// The console's grid, built like the dbt results grid: a row-number
    /// column, a header on the raised surface whose labels sort and whose
    /// edges resize, striped rows that light up under the pointer, and
    /// cells that open whole on a click. Mapping previews and query
    /// results share it.
    fn render_grid(
        &self,
        slot: GridSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        const ROW_NUMBER_WIDTH: f32 = 56.;
        const HEADER_HEIGHT: f32 = 30.;
        const ROW_HEIGHT: f32 = 26.;
        /// A regular scrollbar's thickness with its padding.
        const GUTTER: f32 = 14.;
        let Some(grid) = self.grid(slot) else {
            return div().into_any_element();
        };
        let (scroll, h_scroll) = match slot {
            GridSlot::Preview => (&self.preview_scroll, &self.preview_h_scroll),
            GridSlot::Result => (&self.result_scroll, &self.result_h_scroll),
        };
        let id = slot.id();
        let colors = cx.theme().colors();
        // The scrollbar track is opaque and always painted, so the bars are
        // always on screen. A bar mixes its thumb into the track's color but
        // keeps the track's alpha, and every bundled theme's own track color
        // is transparent — which left the thumbs invisible.
        let (border, surface, hover, track) = (
            colors.border,
            colors.elevated_surface_background,
            colors.element_hover,
            colors.editor_background,
        );
        // Zebra striping for row scanning, as faint as the dbt grid's.
        let mut stripe = colors.text;
        stripe.a = 0.03;
        let columns = grid.table.columns.clone();
        let widths = grid.widths.clone();
        let total = px(ROW_NUMBER_WIDTH + widths.iter().sum::<f32>());

        let header = h_flex()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .bg(surface)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .w(px(ROW_NUMBER_WIDTH))
                    .flex_none()
                    .px_2()
                    .child(Label::new("#").size(LabelSize::Small).color(Color::Muted)),
            )
            .children(columns.iter().enumerate().map(|(column, name)| {
                let arrow = match grid.sort {
                    Some((sorted, true)) if sorted == column => Some("▲"),
                    Some((sorted, false)) if sorted == column => Some("▼"),
                    _ => None,
                };
                div()
                    .w(px(widths[column]))
                    .flex_none()
                    .h_full()
                    .relative()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .id(("sort", column))
                            .size_full()
                            .px_2()
                            .gap_1()
                            .cursor_pointer()
                            .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                            .children(arrow.map(|arrow| Label::new(arrow).size(LabelSize::Small)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(grid) = this.grid_mut(slot) {
                                    grid.cycle_sort(column);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        div()
                            .id(("resize", column))
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(px(5.))
                            .h_full()
                            .cursor(gpui::CursorStyle::ResizeColumn)
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(grid) = this.grid_mut(slot) {
                                        let width = grid.widths[column];
                                        grid.resize =
                                            Some((column, f32::from(event.position.x), width));
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
            }));

        let rows = grid.table.rows.clone();
        let order = grid.order.clone();
        let panel = cx.entity().downgrade();
        let list = gpui::uniform_list(SharedString::from(format!("{id}-rows")), order.len(), {
            move |range, _, _| {
                range
                    .filter_map(|position| {
                        let original = *order.get(position)?;
                        let row = rows.get(original)?;
                        let cells = row.iter().enumerate().map(|(column, cell)| {
                            let panel = panel.clone();
                            let name = columns.get(column).cloned().unwrap_or_default();
                            let value = cell.clone();
                            h_flex()
                                .id(column)
                                .w(px(widths.get(column).copied().unwrap_or(90.)))
                                .h_full()
                                .flex_none()
                                .px_2()
                                .overflow_hidden()
                                .cursor_pointer()
                                .child(Label::new(one_line(cell)).size(LabelSize::Small).truncate())
                                .on_click(move |_, _, cx| {
                                    panel
                                        .update(cx, |this, cx| {
                                            if let Some(grid) = this.grid_mut(slot) {
                                                grid.detail = Some((
                                                    name.clone(),
                                                    original + 1,
                                                    value.clone(),
                                                ));
                                            }
                                            this.detail_scroll.set_offset(gpui::Point::default());
                                            cx.notify();
                                        })
                                        .ok();
                                })
                        });
                        Some(
                            h_flex()
                                .id(original)
                                .w_full()
                                .h(px(ROW_HEIGHT))
                                .overflow_hidden()
                                .when(position % 2 == 1, |row| row.bg(stripe))
                                .hover(|row| row.bg(hover))
                                .child(
                                    div().w(px(ROW_NUMBER_WIDTH)).flex_none().px_2().child(
                                        Label::new((original + 1).to_string())
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                                )
                                .children(cells)
                                .into_any_element(),
                        )
                    })
                    .collect()
            }
        })
        .flex_1()
        .track_scroll(scroll);

        // Each scrollbar lives in a gutter of its own at the grid's edge — the
        // vertical one beside the rows and below the header, the horizontal one
        // under the rows — so neither ever covers a cell. A bar draws along its
        // gutter from the handle it reads, and each gutter is exactly as long as
        // what its handle scrolls. A bar keeps the handle it first read under
        // its id, so each grid's bars carry ids of their own: sharing one would
        // leave the preview's bars reading the query grid's scroll.
        let resizing = grid.resize.is_some();
        v_flex()
            .id(id)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .when(resizing, |grid| {
                grid.on_mouse_move(
                    cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                        let Some(grid) = this.grid_mut(slot) else {
                            return;
                        };
                        if let Some((column, start_x, start_width)) = grid.resize {
                            let width = (start_width + f32::from(event.position.x) - start_x)
                                .clamp(60., 900.);
                            std::sync::Arc::make_mut(&mut grid.widths)[column] = width;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(grid) = this.grid_mut(slot) {
                            grid.resize = None;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(grid) = this.grid_mut(slot) {
                            grid.resize = None;
                            cx.notify();
                        }
                    }),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-scroll")))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_x_scroll()
                            // Sideways wheel motion only; vertical goes to the rows'
                            // list so the two axes don't fight.
                            .restrict_scroll_to_axis()
                            .track_scroll(h_scroll)
                            // At least as wide as the grid, so the header band and
                            // the stripes reach its edge when the columns don't.
                            .child(
                                v_flex()
                                    .min_w_full()
                                    .w(total)
                                    .h_full()
                                    .child(header)
                                    .child(list),
                            ),
                    )
                    .child(
                        v_flex()
                            .w(px(GUTTER))
                            .h_full()
                            .flex_none()
                            // The header band runs on over the gutter.
                            .child(
                                div()
                                    .h(px(HEADER_HEIGHT))
                                    .flex_none()
                                    .bg(surface)
                                    .border_b_1()
                                    .border_color(border),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .bg(track)
                                    .child(div())
                                    .custom_scrollbars(
                                        ui::Scrollbars::always_visible(ui::ScrollAxes::Vertical)
                                            .id(SharedString::from(format!("{id}-vertical-bar")))
                                            .tracked_scroll_handle(scroll)
                                            .with_track_along(ui::ScrollAxes::Vertical, track),
                                        window,
                                        cx,
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(GUTTER))
                    .flex_none()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(track)
                            .child(div())
                            .custom_scrollbars(
                                ui::Scrollbars::always_visible(ui::ScrollAxes::Horizontal)
                                    .id(SharedString::from(format!("{id}-horizontal-bar")))
                                    .tracked_scroll_handle(h_scroll)
                                    .with_track_along(ui::ScrollAxes::Horizontal, track),
                                window,
                                cx,
                            ),
                    )
                    .child(div().w(px(GUTTER)).h_full().flex_none().bg(track)),
            )
            .into_any_element()
    }

    /// The clicked cell, whole, over the console — the dbt results grid's
    /// card: its column and row, the value wrapped and scrollable, and Copy.
    fn render_cell_detail(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let slot = match (&self.preview, self.surface) {
            (Some(_), _) => GridSlot::Preview,
            (None, Surface::Query) => GridSlot::Result,
            (None, _) => return None,
        };
        let (column, row, value) = self.grid(slot)?.detail.clone()?;
        let close = move |this: &mut Self, cx: &mut Context<Self>| {
            if let Some(grid) = this.grid_mut(slot) {
                grid.detail = None;
                cx.notify();
            }
        };
        let colors = cx.theme().colors().clone();
        Some(
            div()
                .id("el-cell-detail-backdrop")
                .absolute()
                .inset_0()
                .p_2()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                .on_click(cx.listener(move |this, _, _, cx| close(this, cx)))
                .child(
                    v_flex()
                        .id("el-cell-detail-card")
                        .w(px(560.))
                        .max_w_full()
                        .max_h_full()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .p_3()
                        .gap_2()
                        .occlude()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_2()
                                .child(
                                    Label::new(format!("{column} · row {row}"))
                                        .size(LabelSize::Small)
                                        .color(Color::Accent)
                                        .truncate(),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .flex_none()
                                        .child(
                                            Button::new("el-cell-copy", "Copy")
                                                .label_size(LabelSize::Small)
                                                .on_click({
                                                    let value = value.clone();
                                                    move |_, _, cx| {
                                                        cx.write_to_clipboard(
                                                            gpui::ClipboardItem::new_string(
                                                                value.to_string(),
                                                            ),
                                                        );
                                                    }
                                                }),
                                        )
                                        .child(
                                            Button::new("el-cell-close", "✕")
                                                .label_size(LabelSize::Small)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    close(this, cx)
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_h_0()
                                .max_h(px(360.))
                                .child(
                                    div()
                                        .id("el-cell-detail-value")
                                        .flex_1()
                                        .min_h_0()
                                        .pr_3()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.detail_scroll)
                                        .child(
                                            Label::new(value)
                                                .size(LabelSize::Small)
                                                .buffer_font(cx),
                                        ),
                                )
                                .custom_scrollbars(
                                    super::scrollbar(&self.detail_scroll),
                                    window,
                                    cx,
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
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
                let count = preview.table.rows.len();
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
                            .child(Label::new(preview.table.title.clone()).size(LabelSize::Default))
                            .child(
                                Label::new(format!("{count} rows"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(self.render_grid(GridSlot::Preview, window, cx))
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
                    Surface::Query => self.render_query(window, cx),
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
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElRunsPanel")
            .on_action(cx.listener(|this, _: &crate::RunQuery, _, cx| {
                // Bound inside the SQL box; the Query surface is the only
                // place that box is on screen.
                if this.surface == Surface::Query {
                    this.run_query(cx);
                }
            }))
            .bg(colors.panel_background)
            .child(body)
            .children(self.render_cell_detail(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gpui::SharedString;

    use super::{
        Color, ConsoleGrid, IconName, PreviewTable, REMOTE_LOG_CAP, TOOLTIP_CAP, absolute_time,
        append_log, eta_text, fit_widths, live_progress, log_color, merge_log_tail, merge_run,
        one_line, place_run_event, rate_text, select_sql, sorted_rows, span_text, split_log_line,
        status_mark, tooltip_text,
    };

    #[test]
    fn clicking_a_table_selects_its_first_rows_on_the_connections_own_terms() {
        assert_eq!(
            select_sql("postgres", Some("public"), "orders"),
            "SELECT * FROM \"public\".\"orders\" LIMIT 200"
        );
        // Oracle reaches back to 10g, where LIMIT does not exist.
        assert_eq!(
            select_sql("oracle", Some("ART_REF"), "ZONE_TARIFAIRE"),
            "SELECT * FROM \"ART_REF\".\"ZONE_TARIFAIRE\" WHERE ROWNUM <= 200"
        );
        // A Snowflake schema carries its database.
        assert_eq!(
            select_sql("snowflake", Some("DATALAKE.ACQ"), "ZONE_TARIFAIRE"),
            "SELECT * FROM \"DATALAKE\".\"ACQ\".\"ZONE_TARIFAIRE\" LIMIT 200"
        );
        // Only Snowflake splits on the dot; elsewhere it is part of the name.
        assert_eq!(
            select_sql("postgres", Some("odd.schema"), "t"),
            "SELECT * FROM \"odd.schema\".\"t\" LIMIT 200"
        );
        // SQL Server has no LIMIT, brackets its identifiers, and caps a
        // result before the select list.
        assert_eq!(
            select_sql("mssql", Some("sales"), "orders"),
            "SELECT TOP 200 * FROM [sales].[orders]"
        );
        assert_eq!(
            select_sql("mssql", None, "we]ird"),
            "SELECT TOP 200 * FROM [we]]ird]"
        );
        // A DuckDB table needs no schema, and a quote in a name is doubled.
        assert_eq!(
            select_sql("duckdb", None, "orders"),
            "SELECT * FROM \"orders\" LIMIT 200"
        );
        assert_eq!(
            select_sql("duckdb", Some(""), "we\"ird"),
            "SELECT * FROM \"we\"\"ird\" LIMIT 200"
        );
    }

    fn table(rows: &[&[&str]]) -> Vec<Vec<SharedString>> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|cell| SharedString::from(cell.to_string()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_header_sort_orders_numbers_by_value_and_puts_text_after_them() {
        let rows = table(&[&["10"], &["9"], &[""], &["1a"], &["-2.5"], &["9"]]);
        assert_eq!(sorted_rows(&rows, None), [0, 1, 2, 3, 4, 5]);
        // Ties (the two 9s) keep the table's order both ways.
        assert_eq!(sorted_rows(&rows, Some((0, true))), [4, 1, 5, 0, 2, 3]);
        assert_eq!(sorted_rows(&rows, Some((0, false))), [3, 2, 0, 1, 5, 4]);
        // A column past a short row reads as empty rather than panicking.
        assert_eq!(sorted_rows(&rows, Some((3, true))), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn header_clicks_cycle_ascending_descending_and_back_to_the_table_order() {
        let rows = table(&[&["b", "2"], &["a", "10"], &["c", "1"]]);
        let mut grid = ConsoleGrid::new(PreviewTable {
            title: "t".into(),
            columns: std::sync::Arc::new(vec!["name".into(), "n".into()]),
            rows: std::sync::Arc::new(rows),
        });
        grid.cycle_sort(1);
        assert_eq!(
            (grid.sort, grid.order.as_slice()),
            (Some((1, true)), &[2, 0, 1][..])
        );
        grid.cycle_sort(1);
        assert_eq!(
            (grid.sort, grid.order.as_slice()),
            (Some((1, false)), &[1, 0, 2][..])
        );
        grid.cycle_sort(1);
        assert_eq!((grid.sort, grid.order.as_slice()), (None, &[0, 1, 2][..]));
        grid.cycle_sort(1);
        grid.cycle_sort(0);
        assert_eq!(
            (grid.sort, grid.order.as_slice()),
            (Some((0, true)), &[1, 0, 2][..])
        );
    }

    #[test]
    fn columns_fit_their_longest_value_within_bounds() {
        let columns = vec![
            SharedString::from("id"),
            SharedString::from("a_rather_long_column_name"),
            SharedString::from("payload"),
        ];
        let rows = table(&[&["1", "x", &"y".repeat(500)]]);
        let widths = fit_widths(&columns, &rows);
        assert_eq!(widths[0], 90.);
        assert_eq!(widths[1], 25. * 8.2 + 24.);
        assert_eq!(widths[2], 340.);
    }

    #[test]
    fn grid_cells_fold_onto_one_line() {
        assert_eq!(
            one_line("{\n  \"brand\": \"bzb\",\n  \"amount\": \"-80\"\n}").as_ref(),
            "{ \"brand\": \"bzb\", \"amount\": \"-80\" }"
        );
        assert_eq!(one_line("plain").as_ref(), "plain");
        assert_eq!(one_line(&"x".repeat(400)).chars().count(), 301);
    }

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
            estimated_rows: None,
        }
    }

    fn chunk(name: &str, phase: el_engine::Phase, read: u64, written: u64) -> el_engine::ProgressEvent {
        el_engine::ProgressEvent::Chunk {
            stream: name.to_owned(),
            phase,
            rows_read: read,
            rows_written: written,
            cast_failures: 0,
        }
    }

    /// A status is a shape as well as a colour, and a status the panel has
    /// never heard of is neutral rather than dressed as a success.
    #[test]
    fn every_status_has_its_own_mark() {
        let marks: Vec<IconName> = ["ok", "failed", "cancelled"]
            .iter()
            .map(|status| status_mark(status).0)
            .collect();
        assert_eq!(marks.len(), 3);
        assert!(marks[0] != marks[1] && marks[1] != marks[2] && marks[0] != marks[2]);
        assert!(matches!(status_mark("ok").1, Color::Success));
        assert!(matches!(status_mark("failed").1, Color::Error));
        assert!(matches!(status_mark("cancelled").1, Color::Warning));
        assert!(matches!(status_mark("paused").1, Color::Muted));
    }

    #[test]
    fn a_span_reads_in_the_two_units_that_matter_at_its_size() {
        assert_eq!(span_text(9), "9s");
        assert_eq!(span_text(61), "1m 01s");
        assert_eq!(span_text(33 * 60 + 32), "33m 32s");
        assert_eq!(span_text(2 * 3600 + 51 * 60 + 7), "2h 51m");
    }

    /// The row of a run in flight says what the run is doing: a source
    /// that is being read shows rows read, not "0 rows", and once rows
    /// land it is the landed count that counts.
    #[test]
    fn a_run_in_flight_is_summed_up_from_its_events() {
        assert!(live_progress(&[]).is_none(), "nothing to say before the first event");

        let reading = [
            el_engine::ProgressEvent::RunStarted {
                pipeline: "p".into(),
                streams: vec!["orders".into()],
            },
            stream("orders"),
            chunk("orders", el_engine::Phase::Extract, 8_412, 0),
        ];
        let progress = live_progress(&reading).expect("events arrived");
        assert_eq!(progress.summary(), "extract · 8,412 read");

        let landing = [
            el_engine::ProgressEvent::RunStarted {
                pipeline: "p".into(),
                streams: vec!["orders".into(), "lines".into()],
            },
            stream("orders"),
            chunk("orders", el_engine::Phase::Stage, 500_000, 500_000),
            el_engine::ProgressEvent::StreamFinished {
                stream: "orders".into(),
                rows_read: 500_000,
                rows_written: 500_000,
                cast_failures: 0,
                column_failures: Vec::new(),
            },
            stream("lines"),
            chunk("lines", el_engine::Phase::Stage, 700_000, 700_000),
        ];
        let progress = live_progress(&landing).expect("events arrived");
        assert_eq!(progress.summary(), "stage · 1,200,000 rows · 1/2 streams");
    }

    #[test]
    fn an_absolute_time_names_the_day_and_the_second() {
        let text = absolute_time(1_789_920_751);
        assert!(text.contains("2026"), "{text}");
        assert_eq!(text.matches(':').count(), 2, "{text}");
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
            el_engine::ProgressEvent::StreamStarted { ref stream, .. } if stream == "orders"
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

    /// The log pane reads the daemon's own format, and leaves anything
    /// else alone rather than cutting it at the first double space.
    /// What the run header and the rows/min column print.
    #[test]
    fn a_rate_reads_in_the_unit_that_suits_its_size() {
        assert_eq!(rate_text(0.), "0/min");
        assert_eq!(rate_text(940.), "940/min");
        assert_eq!(rate_text(15_400.), "15k/min");
        assert_eq!(rate_text(928_000.), "928k/min");
        assert_eq!(rate_text(2_500_000.), "2.5M/min");
    }

    /// A countdown is only shown when something is actually moving.
    #[test]
    fn an_eta_needs_a_rate_to_count_down_from() {
        assert_eq!(eta_text(1_000, 0.), None);
        assert_eq!(eta_text(1_000, -1.), None);
        assert_eq!(eta_text(30_000, 60_000.), Some("30 s left".to_owned()));
        assert_eq!(eta_text(600_000, 60_000.), Some("10 min left".to_owned()));
        assert_eq!(eta_text(60_000_000, 600_000.), Some("1.7 h left".to_owned()));
    }

    #[test]
    fn a_log_line_splits_into_its_time_and_what_happened() {
        assert_eq!(
            split_log_line("14:02:58  run 7 (orders) finished"),
            ("14:02:58", "run 7 (orders) finished")
        );
        assert_eq!(split_log_line("no stamp  here"), ("", "no stamp  here"));
        assert_eq!(split_log_line("plain line"), ("", "plain line"));
        // A message that itself contains a double space keeps it whole.
        assert_eq!(
            split_log_line("09:00:00  deployed 2 pipeline(s):  a, b"),
            ("09:00:00", "deployed 2 pipeline(s):  a, b")
        );
    }

    /// What "Problems" filters to: the lines that report something the
    /// daemon could not do.
    #[test]
    fn a_log_line_is_colored_by_what_it_reports() {
        assert_eq!(
            log_color("14:02:58  run 7 (orders) failed: connection refused"),
            Color::Error
        );
        assert_eq!(
            log_color("14:02:58  failure webhook not delivered: status 500"),
            Color::Error
        );
        assert_eq!(log_color("14:02:58  run 7 (orders) cancelled"), Color::Warning);
        assert_eq!(log_color("14:02:58  run 7 (orders) finished"), Color::Muted);
        assert_eq!(log_color("14:02:58  deployed 1 pipeline(s): orders"), Color::Muted);
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
