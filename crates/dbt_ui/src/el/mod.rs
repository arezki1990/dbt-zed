//! The EL (extract→load) UI: pipeline canvas, scaffolding, and spec IO.
//! The engine lives in `el_engine`; nothing here touches a warehouse.

pub mod builder;
pub mod canvas_item;
pub mod cli;
pub mod connection_modal;
pub mod deploy_modal;
pub mod dialog;
pub mod layout;
pub mod local_runner;
pub mod mapping_editor;
pub mod mcp;
pub mod panel;
pub mod remote_modal;
pub mod rename_stream_modal;
pub mod run_view;
pub mod runs_panel;
pub mod scaffold;
pub mod spec_io;

pub use canvas_item::ElPipelineCanvas;
pub use panel::ElPanel;
pub use runs_panel::ElRunsPanel;

use anyhow::Context as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{AppContext as _, Context, SharedString, Window};
use workspace::Workspace;

/// A table dragged out of the EL panel's explorer — dropped on a pipeline
/// canvas it becomes a stream.
#[derive(Clone)]
pub struct DraggedTable {
    pub connection: SharedString,
    pub schema: String,
    pub table: String,
}

/// A whole schema — or, with none, every table of a connection — dragged
/// out of the EL panel: dropped on a pipeline canvas, each table becomes a
/// stream. The canvas asks the panel for the tables when it lands.
#[derive(Clone)]
pub struct DraggedTables {
    pub connection: SharedString,
    pub schema: Option<String>,
    pub count: usize,
}

/// Kinds the explorer can browse — the worker's list/query support. The
/// panel lists their tables and queries them.
pub(crate) fn browsable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres" | "oracle" | "snowflake")
}

/// Kinds a pipeline extracts from, so their tables can become streams: the
/// canvas invites a drag from these. Snowflake is browsable but only ever a
/// destination.
pub(crate) fn extractable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres" | "oracle")
}

/// The vertical scrollbar of an EL scroll area: always on screen rather
/// than fading out, reading the handle the area scrolls with. Put it on a
/// box around the scrolling element, never on that element itself, or the
/// bar scrolls away with the content.
pub(crate) fn scrollbar<T: ui::ScrollableHandle>(handle: &T) -> ui::Scrollbars<T> {
    ui::Scrollbars::always_visible(ui::ScrollAxes::Vertical).tracked_scroll_handle(handle)
}

/// The EL directory for a project root — `el/` beside dbt_project.yml, or
/// standalone: EL projects need no dbt project at all.
pub fn el_dir(project_root: &Path) -> PathBuf {
    project_root.join("el")
}

/// The EL project root in this workspace: a dbt root when present, else
/// any worktree already holding `el/`, else the first worktree (so
/// Initialize can create a standalone EL project).
pub fn discover_el_root(workspace: &Workspace, cx: &gpui::App) -> Option<PathBuf> {
    if let Some(root) = crate::database_panel::discover_workspace_root(workspace, cx) {
        return Some(root);
    }
    let mut first = None;
    for worktree in workspace.project().read(cx).worktrees(cx) {
        let root = worktree.read(cx).abs_path().to_path_buf();
        if root.join("el").is_dir() {
            return Some(root);
        }
        first.get_or_insert(root);
    }
    first
}

/// Where the checkout's database operations run: the local worker binary,
/// or the daemon of a remote named in `remotes.yml`. A remote's worker
/// carries the drivers and client libraries this machine may lack (the
/// thick Oracle driver on a Mac, for one), and resolves connections under
/// the server's own connections.yml and profile.
#[derive(Debug)]
pub enum DbWorker {
    Local(PathBuf),
    Remote(String),
}

/// Where one connection's database operations run. A connection names its
/// workspace in connections.yml; absent, it belongs to this machine — the
/// local worker is the default nothing has to declare. Resolution goes
/// through the active profile, so a profile that repoints a name also
/// carries that connection's workspace.
pub fn db_worker(project_root: &Path, connection: &str) -> anyhow::Result<DbWorker> {
    let (connections, _) = el_engine::spec::load_active_connections(project_root)?;
    let workspace = connections
        .connections
        .get(connection)
        .ok_or_else(|| anyhow::anyhow!("connection {connection:?} is gone from connections.yml"))?
        .workspace()
        .map(str::to_owned);
    match workspace {
        Some(remote) => Ok(DbWorker::Remote(remote)),
        None => find_worker()
            .map(DbWorker::Local)
            .ok_or_else(|| anyhow::anyhow!(WORKER_MISSING)),
    }
}

pub const WORKER_MISSING: &str = "Connector worker not found — build zdbt-el-worker or set ZDBT_EL_WORKER, or pick a \
     remote's worker in the EL panel.";

fn local_connection(
    project_root: &Path,
    name: &str,
) -> anyhow::Result<(el_engine::spec::Connection, el_engine::env::EnvMap)> {
    let (connections, _) = el_engine::spec::load_active_connections(project_root)?;
    let connection = connections
        .connections
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("connection {name:?} is gone from connections.yml"))?;
    Ok((connection, el_engine::env::EnvMap::load(project_root, None)))
}

/// Lists a connection's tables on the checkout's database worker.
/// Blocking — call from a background thread.
pub fn list_tables(project_root: &Path, connection: &str) -> anyhow::Result<Vec<(String, String)>> {
    match db_worker(project_root, connection)? {
        DbWorker::Local(worker) => {
            let (connection, env) = local_connection(project_root, connection)?;
            el_engine::explore::list_tables(&worker, project_root, &connection, &env)
        }
        DbWorker::Remote(remote) => {
            el_engine::server::RemoteClient::connect(project_root, &remote)?
                .explore_tables(connection)
                .with_context(|| format!("on remote {remote}"))
        }
    }
}

/// The local worker for one level of a catalog tree. A remote's daemon
/// only lists a catalog whole, so its connections are grouped instead.
fn level_worker(
    project_root: &Path,
    name: &str,
) -> anyhow::Result<(PathBuf, el_engine::spec::Connection, el_engine::env::EnvMap)> {
    match db_worker(project_root, name)? {
        DbWorker::Local(worker) => {
            let (connection, env) = local_connection(project_root, name)?;
            Ok((worker, connection, env))
        }
        DbWorker::Remote(remote) => {
            anyhow::bail!("{name} runs on {remote}, which lists its catalog whole")
        }
    }
}

/// A Snowflake account's databases. Blocking.
pub fn list_databases(project_root: &Path, connection: &str) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, connection)?;
    el_engine::explore::list_databases(&worker, project_root, &connection, &env)
}

/// One Snowflake database's schemas. Blocking.
pub fn list_schemas(
    project_root: &Path,
    connection: &str,
    database: &str,
) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, connection)?;
    el_engine::explore::list_schemas(&worker, project_root, &connection, &env, database)
}

/// One Snowflake schema's tables and views, by name. Blocking.
pub fn list_schema_tables(
    project_root: &Path,
    connection: &str,
    database: &str,
    schema: &str,
) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, connection)?;
    Ok(el_engine::explore::list_schema_tables(
        &worker,
        project_root,
        &connection,
        &env,
        database,
        schema,
    )?
    .into_iter()
    .map(|(_, table)| table)
    .collect())
}

/// Runs an ad-hoc query on the checkout's database worker. Blocking.
pub fn run_query(
    project_root: &Path,
    connection: &str,
    sql: &str,
    limit: usize,
) -> anyhow::Result<el_engine::explore::QueryResult> {
    match db_worker(project_root, connection)? {
        DbWorker::Local(worker) => {
            let (connection, env) = local_connection(project_root, connection)?;
            el_engine::explore::run_query(&worker, project_root, &connection, &env, sql, limit)
        }
        DbWorker::Remote(remote) => {
            el_engine::server::RemoteClient::connect(project_root, &remote)?
                .explore_query(connection, sql, limit)
                .with_context(|| format!("on remote {remote}"))
        }
    }
}

/// Previews a stream on the checkout's database worker. Blocking.
pub fn preview_stream(
    project_root: &Path,
    pipeline: &el_engine::spec::Pipeline,
    stream: &str,
    limit: usize,
    cancel: &el_engine::CancelFlag,
) -> anyhow::Result<el_engine::preview::PreviewResult> {
    // A preview reads through the pipeline's source, so it belongs to that
    // connection's workspace — not to whichever one the tree last touched.
    match db_worker(project_root, &pipeline.source)? {
        DbWorker::Local(worker) => {
            el_engine::preview_stream(project_root, pipeline, stream, limit, Some(&worker), cancel)
        }
        DbWorker::Remote(remote) => {
            el_engine::server::RemoteClient::connect(project_root, &remote)?
                .explore_preview(&el_engine::spec::to_canonical_yaml(pipeline), stream, limit)
                .with_context(|| format!("on remote {remote}"))
        }
    }
}

/// Locates the on-demand connector worker binary: an explicit env
/// override, then a sibling of the running executable (dev builds and
/// bundles), then the managed install dir (the future download target).
pub fn find_worker() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZDBT_EL_WORKER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("zdbt-el-worker");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    let managed = paths::data_dir().join("el-worker").join("zdbt-el-worker");
    managed.is_file().then_some(managed)
}

/// `el::OpenPipelines`: opens the canvas for the project's pipeline(s) —
/// the first (alphabetically) when several exist; the database panel's
/// pipeline list is the picker for the rest.
pub fn open_pipelines(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(root) = discover_el_root(workspace, cx) else {
        toast(workspace, "No project folder open in this workspace.", cx);
        return;
    };
    let pipelines = el_engine::spec::list_pipelines(&el_dir(&root));
    match pipelines.first() {
        Some(path) => {
            canvas_item::ElPipelineCanvas::deploy(workspace, root, path.clone(), window, cx)
        }
        None => toast(
            workspace,
            "No EL pipelines yet — run `el: initialize workspace` to scaffold el/.",
            cx,
        ),
    }
}

/// `el::InitializeWorkspace`: scaffolds `el/` and opens the example
/// pipeline on the canvas.
pub fn initialize_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(root) = discover_el_root(workspace, cx) else {
        toast(workspace, "No project folder open in this workspace.", cx);
        return;
    };
    // Scaffolding writes files and may run the worker to seed a demo
    // database, so it stays off the UI thread.
    let scaffold = cx.background_spawn({
        let root = root.clone();
        async move { scaffold::initialize_el_workspace(&root) }
    });
    cx.spawn_in(window, async move |workspace, cx| {
        let result = scaffold.await;
        workspace
            .update_in(cx, |workspace, window, cx| match result {
                Ok(created) => {
                    toast(
                        workspace,
                        &format!(
                            "EL workspace ready — {} file(s) created under el/.",
                            created.len()
                        ),
                        cx,
                    );
                    let example = el_dir(&root).join("pipelines").join("example.yml");
                    if example.is_file() {
                        canvas_item::ElPipelineCanvas::deploy(workspace, root, example, window, cx);
                    }
                }
                Err(error) => {
                    toast_error(workspace, &format!("EL init failed: {error:#}"), None, cx)
                }
            })
            .ok();
    })
    .detach();
}

/// A success/info toast: auto-hides, and never dismisses a standing error
/// (errors live under their own notification id).
pub(crate) fn toast(workspace: &mut Workspace, message: &str, cx: &mut Context<Workspace>) {
    struct ElOk;
    workspace.show_toast(
        workspace::Toast::new(
            workspace::notifications::NotificationId::unique::<ElOk>(),
            message.to_owned(),
        )
        .autohide(),
        cx,
    );
}

/// An error toast: stays until dismissed, optionally with a next step
/// ("Open YAML") that opens the file the error is about.
pub(crate) fn toast_error(
    workspace: &mut Workspace,
    message: &str,
    open: Option<PathBuf>,
    cx: &mut Context<Workspace>,
) {
    struct ElError;
    let mut toast = workspace::Toast::new(
        workspace::notifications::NotificationId::unique::<ElError>(),
        message.to_owned(),
    );
    if let Some(path) = open {
        let workspace_handle = cx.entity().downgrade();
        toast = toast.on_click("Open YAML", move |window, cx| {
            let path = path.clone();
            workspace_handle
                .update(cx, |workspace, cx| {
                    workspace
                        .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                        .detach();
                })
                .ok();
        });
    }
    workspace.show_toast(toast, cx);
}

/// Each table as lowercase `schema.table`: folded once, so a search per
/// keystroke only compares.
pub(crate) fn fold_table_names(tables: &[(String, String)]) -> Vec<String> {
    tables
        .iter()
        .map(|(schema, table)| format!("{schema}.{table}").to_lowercase())
        .collect()
}

pub(crate) fn search_terms(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

pub(crate) fn matching_tables(haystack: &[String], terms: &[String]) -> Vec<usize> {
    haystack
        .iter()
        .enumerate()
        .filter(|(_, name)| terms.iter().all(|term| name.contains(term.as_str())))
        .map(|(index, _)| index)
        .collect()
}

/// Where each search term first appears in `label`, merged where they
/// overlap. Folding only ASCII case keeps the byte offsets valid for the
/// label itself; a term that matched through wider Unicode folding just
/// goes unhighlighted.
pub(crate) fn highlight_ranges(label: &str, terms: &[String]) -> Vec<Range<usize>> {
    let folded = label.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = terms
        .iter()
        .filter_map(|term| {
            let start = folded.find(term.as_str())?;
            let end = start + term.len();
            (label.is_char_boundary(start) && label.is_char_boundary(end)).then_some(start..end)
        })
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

pub(crate) fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a project whose two connections sit on different workspaces.
    fn project(yaml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(el_dir(dir.path())).unwrap();
        std::fs::write(el_dir(dir.path()).join("connections.yml"), yaml).unwrap();
        dir
    }

    /// The whole point of the per-connection binding: one connection can
    /// name a remote while its neighbour stays on this machine. Resolving
    /// one worker for the checkout meant a dead remote took every
    /// connection down with it, browsable local files included.
    #[test]
    fn each_connection_resolves_its_own_workspace() {
        let dir = project(
            r#"
version: 1
connections:
  pg:
    type: postgres
    url: postgres://localhost/db
    workspace: vm
  wh:
    type: duckdb
    path: el/demo.duckdb
"#,
        );

        assert!(
            matches!(db_worker(dir.path(), "pg"), Ok(DbWorker::Remote(name)) if name == "vm"),
            "a declared workspace sends the connection to that remote"
        );
        // `wh` declares none, so it belongs to the local worker. Whether a
        // worker binary is present here decides between Ok(Local) and the
        // WORKER_MISSING error — either way it never reaches the remote.
        match db_worker(dir.path(), "wh") {
            Ok(DbWorker::Local(_)) => {}
            Err(error) => assert!(format!("{error:#}").contains("Connector worker not found")),
            Ok(DbWorker::Remote(name)) => panic!("an undeclared workspace went to {name:?}"),
        }
    }

    #[test]
    fn an_unknown_connection_says_so_instead_of_guessing() {
        let dir = project("version: 1\nconnections: {}\n");
        let error = db_worker(dir.path(), "ghost").unwrap_err();
        assert!(format!("{error:#}").contains("ghost"), "{error:#}");
    }

    #[test]
    fn search_needs_every_term_in_any_case() {
        let haystack: Vec<String> = [
            "adm_dba.dba_nb_process",
            "art_ref.article_zone_tarif",
            "art_ref.article",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            matching_tables(&haystack, &search_terms("ART zone")),
            vec![1]
        );
        assert_eq!(
            matching_tables(&haystack, &search_terms("article")),
            vec![1, 2]
        );
        assert_eq!(
            matching_tables(&haystack, &search_terms("  ")),
            vec![0, 1, 2]
        );
        assert!(matching_tables(&haystack, &search_terms("orders")).is_empty());
    }

    #[test]
    fn highlights_merge_and_keep_to_char_boundaries() {
        assert_eq!(
            highlight_ranges("ART_REF.ARTICLE", &search_terms("art ref")),
            vec![0..3, 4..7]
        );
        assert_eq!(
            highlight_ranges("ART_REF.ARTICLE", &search_terms("art_r rt_ref")),
            vec![0..7]
        );
        assert!(highlight_ranges("CAFÉ.MENU", &search_terms("é")).is_empty());
        assert_eq!(
            highlight_ranges("café.menu", &search_terms("é menu")),
            vec![3..5, 6..10]
        );
    }

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(100), "100");
        assert_eq!(thousands(9_331), "9,331");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
