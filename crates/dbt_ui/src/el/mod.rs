//! The EL (extract→load) UI: pipeline canvas, scaffolding, and spec IO.
//! The engine lives in `el_engine`; nothing here touches a warehouse.

pub mod builder;
pub mod canvas_item;
pub mod cli;
pub mod connection_modal;
pub mod deploy_modal;
pub mod dialog;
pub mod layout;
pub mod listing_cache;
pub mod local_runner;
pub mod mapping_editor;
pub mod mcp;
pub mod panel;
pub mod platform;
pub mod push_dbt_modal;
pub mod remote_modal;
pub mod rename_stream_modal;
pub mod run_view;
pub mod runs_panel;
pub mod scaffold;
pub mod spec_io;

pub use canvas_item::ElPipelineCanvas;
pub use panel::ElPanel;
pub use runs_panel::ElRunsPanel;

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
    matches!(
        kind,
        "duckdb" | "postgres" | "oracle" | "snowflake" | "mssql"
    )
}

/// Kinds a pipeline extracts from, so their tables can become streams: the
/// canvas invites a drag from these. Snowflake among them: a warehouse's gold
/// tables are exported from it.
pub(crate) fn extractable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres" | "oracle" | "mssql" | "snowflake")
}

/// The vertical scrollbar of an EL scroll area: always on screen rather
/// than fading out, reading the handle the area scrolls with. Put it on a
/// box around the scrolling element, never on that element itself, or the
/// bar scrolls away with the content.
pub(crate) fn scrollbar<T: ui::ScrollableHandle>(handle: &T) -> ui::Scrollbars<T> {
    ui::Scrollbars::always_visible(ui::ScrollAxes::Vertical).tracked_scroll_handle(handle)
}

/// Kinds a pipeline loads INTO: the databases with a loader, the file
/// stores a target writes files to (`el_engine::spec::is_file_kind`), and
/// the APIs a target sends rows to (`el_engine::spec::is_rest_kind`). A
/// source kind that is not here can still be browsed and queried.
pub(crate) fn loadable(kind: &str) -> bool {
    matches!(kind, "snowflake" | "duckdb" | "oracle" | "mssql" | "postgres")
        || el_engine::spec::is_file_kind(kind)
        || el_engine::spec::is_rest_kind(kind)
}

/// The EL directory for a project root — `el/` beside dbt_project.yml, or
/// standalone: EL projects need no dbt project at all.
pub fn el_dir(project_root: &Path) -> PathBuf {
    project_root.join("el")
}

/// Whether a path is a pipeline spec — `<root>/el/pipelines/<name>.yml`.
/// Shape only: this is asked on every toolbar render, so it never opens
/// the file, and a deploy re-reads and re-validates the spec anyway.
pub fn is_pipeline_file(path: &Path) -> bool {
    let is_yaml = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "yml" || extension == "yaml");
    if !is_yaml {
        return false;
    }
    let mut ancestors = path.ancestors().skip(1);
    let in_pipelines = ancestors
        .next()
        .and_then(|dir| dir.file_name())
        .is_some_and(|name| name == "pipelines");
    let under_el = ancestors
        .next()
        .and_then(|dir| dir.file_name())
        .is_some_and(|name| name == "el");
    in_pipelines && under_el
}

/// The project root a pipeline spec belongs to: the directory holding the
/// `el/` its `pipelines/` sits in.
pub fn pipeline_project_root(path: &Path) -> Option<PathBuf> {
    is_pipeline_file(path)
        .then(|| path.ancestors().nth(3))
        .flatten()
        .map(Path::to_path_buf)
}

/// The pipeline spec open in the active editor, if that is what it is.
pub fn active_pipeline_path(workspace: &Workspace, cx: &gpui::App) -> Option<PathBuf> {
    let editor = workspace
        .active_item(cx)
        .and_then(|item| item.act_as::<editor::Editor>(cx))?;
    let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
    let path = buffer.read(cx).file()?.as_local()?.abs_path(cx);
    is_pipeline_file(&path).then_some(path)
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

pub const WORKER_MISSING: &str = "Connector worker not found — build ellode-worker or set ELLODE_WORKER, or pick a \
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

/// What a remote worker's failure reads as. A daemon that does not know
/// the connection is not broken — it has never been sent this project's
/// connections, and the Remote tab is where that is done. Saying so beats
/// a bare 404.
fn on_remote(remote: &str, error: anyhow::Error) -> anyhow::Error {
    let text = format!("{error:#}");
    if text.contains("is not defined on this server") {
        return anyhow::anyhow!(
            "on remote {remote}: {text} — send this project's connections to it first \
             (Remote tab → Send connections)"
        );
    }
    error.context(format!("on remote {remote}"))
}

/// Lists a connection's tables on the checkout's database worker.
/// Blocking — call from a background thread.
pub fn list_tables(project_root: &Path, connection: &str) -> anyhow::Result<Vec<(String, String)>> {
    let (definition, env) = local_connection(project_root, connection)?;
    listing_cache::read_through(
        project_root,
        connection,
        &definition,
        listing_cache::Level::Tables,
        || match db_worker(project_root, connection)? {
            DbWorker::Local(worker) => {
                el_engine::explore::list_tables(&worker, project_root, &definition, &env)
            }
            DbWorker::Remote(remote) => {
                el_engine::server::RemoteClient::connect(project_root, &remote)?
                    .explore_tables(connection)
                    .map_err(|error| on_remote(&remote, error))
            }
        },
    )
}

/// Drops cached listings so the next ones go to the database. Blocking.
pub fn invalidate_listings(
    project_root: &Path,
    scope: &listing_cache::Scope,
) -> anyhow::Result<()> {
    listing_cache::invalidate(project_root, scope)
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
pub fn list_databases(project_root: &Path, name: &str) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, name)?;
    listing_cache::read_through(
        project_root,
        name,
        &connection,
        listing_cache::Level::Databases,
        || el_engine::explore::list_databases(&worker, project_root, &connection, &env),
    )
}

/// One Snowflake database's schemas. Blocking.
pub fn list_schemas(
    project_root: &Path,
    name: &str,
    database: &str,
) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, name)?;
    listing_cache::read_through(
        project_root,
        name,
        &connection,
        listing_cache::Level::Schemas { database },
        || el_engine::explore::list_schemas(&worker, project_root, &connection, &env, database),
    )
}

/// One Snowflake schema's tables and views, by name. Blocking.
pub fn list_schema_tables(
    project_root: &Path,
    name: &str,
    database: &str,
    schema: &str,
) -> anyhow::Result<Vec<String>> {
    let (worker, connection, env) = level_worker(project_root, name)?;
    listing_cache::read_through(
        project_root,
        name,
        &connection,
        listing_cache::Level::SchemaTables { database, schema },
        || {
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
        },
    )
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
                .map_err(|error| on_remote(&remote, error))
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
                .map_err(|error| on_remote(&remote, error))
        }
    }
}

/// Locates the on-demand connector worker binary: an explicit env
/// override, then a sibling of the running executable (dev builds and
/// bundles), then the managed install dir (the future download target).
pub fn find_worker() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ELLODE_WORKER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("ellode-worker");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    let managed = paths::data_dir().join("el-worker").join("ellode-worker");
    managed.is_file().then_some(managed)
}

/// `el::OpenPipelines`: opens the pipeline canvas. With a pipeline spec in
/// the active editor it is that pipeline's — the switch from the YAML to
/// its graph, which is where Run and Deploy are. Otherwise the project's
/// first (alphabetically); the database panel's pipeline list is the
/// picker for the rest.
pub fn open_pipelines(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(spec_path) = active_pipeline_path(workspace, cx) {
        open_active_pipeline(workspace, spec_path, window, cx);
        return;
    }
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

/// Opens the canvas for the spec in the active editor. The canvas draws
/// the file, not the buffer, and refuses to write under a dirty one — so
/// unsaved edits are saved first: the graph is then the YAML that was on
/// screen, and the first edit made on it goes through.
fn open_active_pipeline(
    workspace: &mut Workspace,
    spec_path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(root) = pipeline_project_root(&spec_path) else {
        return;
    };
    let unsaved = workspace
        .active_item(cx)
        .is_some_and(|item| item.is_dirty(cx));
    if !unsaved {
        canvas_item::ElPipelineCanvas::deploy(workspace, root, spec_path, window, cx);
        return;
    }
    let save = workspace.save_active_item(workspace::SaveIntent::Save, window, cx);
    cx.spawn_in(window, async move |workspace, cx| {
        let saved = save.await;
        workspace
            .update_in(cx, |workspace, window, cx| match saved {
                // A save the user cancelled is `Ok` too: the canvas then
                // shows the file as it is, and the tab still says unsaved.
                Ok(()) => {
                    canvas_item::ElPipelineCanvas::deploy(workspace, root, spec_path, window, cx)
                }
                Err(error) => toast_error(
                    workspace,
                    &format!("Couldn't save the pipeline to open its canvas: {error:#}"),
                    None,
                    cx,
                ),
            })
            .ok();
    })
    .detach();
}

/// `el::InitializeWorkspace`: scaffolds `el/` and opens the example
/// pipeline on the canvas.
/// Pushes the project's dbt code to one of its Ellode platform remotes, as
/// the bundle that project's dbt runs take.
pub fn push_dbt_project(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(root) = discover_el_root(workspace, cx) else {
        toast(workspace, "No project folder open in this workspace.", cx);
        return;
    };
    push_dbt_modal::ElPushDbtModal::deploy(workspace, root, window, cx);
}

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
    /// Which way data moves through each kind: Snowflake gold is exported,
    /// Postgres and the file stores are loaded into.
    #[test]
    fn kinds_say_which_way_data_moves() {
        for kind in ["duckdb", "postgres", "oracle", "mssql", "snowflake"] {
            assert!(super::extractable(kind), "{kind}");
        }
        for kind in ["snowflake", "duckdb", "oracle", "mssql", "postgres", "s3", "gcs", "azure", "local"] {
            assert!(super::loadable(kind), "{kind}");
        }
        assert!(!super::loadable("mysql"));
        assert!(!super::extractable("s3"));
        // A target sends rows to an API, but its streams read endpoints: it
        // has no tables to list.
        assert!(super::loadable("rest"));
        assert!(!super::extractable("rest"));
    }

    /// What the toolbar asks on every render, and what a deploy resolves
    /// the project root from.
    #[test]
    fn a_pipeline_spec_is_known_by_where_it_sits() {
        use std::path::Path;
        assert!(super::is_pipeline_file(Path::new(
            "/work/shop/el/pipelines/orders.yml"
        )));
        assert!(super::is_pipeline_file(Path::new(
            "/work/shop/el/pipelines/orders.yaml"
        )));
        assert_eq!(
            super::pipeline_project_root(Path::new("/work/shop/el/pipelines/orders.yml")),
            Some(Path::new("/work/shop").to_path_buf())
        );

        // Connections live beside pipelines, and are not one.
        assert!(!super::is_pipeline_file(Path::new(
            "/work/shop/el/connections.yml"
        )));
        // A pipelines directory that is not EL's.
        assert!(!super::is_pipeline_file(Path::new(
            "/work/shop/ci/pipelines/deploy.yml"
        )));
        // The canvas's own file, and anything that is not YAML.
        assert!(!super::is_pipeline_file(Path::new(
            "/work/shop/el/pipelines/orders.sql"
        )));
        assert_eq!(super::pipeline_project_root(Path::new("orders.yml")), None);
    }

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
