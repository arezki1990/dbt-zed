//! The EL (extract→load) UI: pipeline canvas, scaffolding, and spec IO.
//! The engine lives in `el_engine`; nothing here touches a warehouse.

pub mod builder;
pub mod canvas_item;
pub mod cli;
pub mod connection_modal;
pub mod deploy_modal;
pub mod remote_modal;
pub mod rename_stream_modal;
pub mod layout;
pub mod mapping_editor;
pub mod panel;
pub mod run_view;
pub mod runs_panel;
pub mod scaffold;
pub mod spec_io;

pub use canvas_item::ElPipelineCanvas;
pub use panel::ElPanel;
pub use runs_panel::ElRunsPanel;

use anyhow::Context as _;
use std::path::{Path, PathBuf};

use gpui::{Context, SharedString, Window};
use workspace::Workspace;

/// A table dragged out of the EL panel's explorer — dropped on a pipeline
/// canvas it becomes a stream.
#[derive(Clone)]
pub struct DraggedTable {
    pub connection: SharedString,
    pub schema: String,
    pub table: String,
}

/// Kinds the explorer can browse — the worker's list/query support. The
/// panel lists their tables; the canvas invites a drag from them.
pub(crate) fn browsable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres" | "oracle")
}

/// The EL directory for a project root — `el/` beside dbt_project.yml, or
/// standalone: EL projects need no dbt project at all.
pub fn el_dir(project_root: &Path) -> PathBuf {
    project_root.join("el")
}

/// The EL project root in this workspace: a dbt root when present, else
/// any worktree already holding `el/`, else the first worktree (so
/// Initialize can create a standalone EL project).
pub fn discover_el_root(
    workspace: &Workspace,
    cx: &gpui::App,
) -> Option<PathBuf> {
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
pub enum DbWorker {
    Local(PathBuf),
    Remote(String),
}

pub fn db_worker(project_root: &Path) -> anyhow::Result<DbWorker> {
    if let Some(remote) = el_engine::spec::active_remote_worker(project_root) {
        return Ok(DbWorker::Remote(remote));
    }
    find_worker()
        .map(DbWorker::Local)
        .ok_or_else(|| anyhow::anyhow!(WORKER_MISSING))
}

pub const WORKER_MISSING: &str =
    "Connector worker not found — build zdbt-el-worker or set ZDBT_EL_WORKER, or pick a \
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
    match db_worker(project_root)? {
        DbWorker::Local(worker) => {
            let (connection, env) = local_connection(project_root, connection)?;
            el_engine::explore::list_tables(&worker, project_root, &connection, &env)
        }
        DbWorker::Remote(remote) => el_engine::server::RemoteClient::connect(project_root, &remote)?
            .explore_tables(connection)
            .with_context(|| format!("on remote {remote}")),
    }
}

/// Runs an ad-hoc query on the checkout's database worker. Blocking.
pub fn run_query(
    project_root: &Path,
    connection: &str,
    sql: &str,
    limit: usize,
) -> anyhow::Result<el_engine::explore::QueryResult> {
    match db_worker(project_root)? {
        DbWorker::Local(worker) => {
            let (connection, env) = local_connection(project_root, connection)?;
            el_engine::explore::run_query(&worker, project_root, &connection, &env, sql, limit)
        }
        DbWorker::Remote(remote) => el_engine::server::RemoteClient::connect(project_root, &remote)?
            .explore_query(connection, sql, limit)
            .with_context(|| format!("on remote {remote}")),
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
    match db_worker(project_root)? {
        DbWorker::Local(worker) => el_engine::preview_stream(
            project_root,
            pipeline,
            stream,
            limit,
            Some(&worker),
            cancel,
        ),
        DbWorker::Remote(remote) => el_engine::server::RemoteClient::connect(project_root, &remote)?
            .explore_preview(&el_engine::spec::to_canonical_yaml(pipeline), stream, limit)
            .with_context(|| format!("on remote {remote}")),
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
pub fn open_pipelines(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
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
    match scaffold::initialize_el_workspace(&root) {
        Ok(created) => {
            toast(
                workspace,
                &format!("EL workspace ready — {} file(s) created under el/.", created.len()),
                cx,
            );
            let example = el_dir(&root).join("pipelines").join("example.yml");
            if example.is_file() {
                canvas_item::ElPipelineCanvas::deploy(workspace, root, example, window, cx);
            }
        }
        Err(error) => toast_error(workspace, &format!("EL init failed: {error:#}"), None, cx),
    }
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
