//! The EL (extract→load) UI: pipeline canvas, scaffolding, and spec IO.
//! The engine lives in `el_engine`; nothing here touches a warehouse.

pub mod builder;
pub mod canvas_item;
pub mod cli;
pub mod layout;
pub mod mapping_editor;
pub mod run_view;
pub mod scaffold;
pub mod spec_io;

pub use canvas_item::ElPipelineCanvas;

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use workspace::Workspace;

/// The EL directory for a dbt project root — `el/` beside dbt_project.yml.
pub fn el_dir(project_root: &Path) -> PathBuf {
    project_root.join("el")
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
    let Some(root) = crate::database_panel::discover_workspace_root(workspace, cx) else {
        notify(workspace, "No dbt project found in this workspace.", cx);
        return;
    };
    let pipelines = el_engine::spec::list_pipelines(&el_dir(&root));
    match pipelines.first() {
        Some(path) => {
            canvas_item::ElPipelineCanvas::deploy(workspace, root, path.clone(), window, cx)
        }
        None => notify(
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
    let Some(root) = crate::database_panel::discover_workspace_root(workspace, cx) else {
        notify(workspace, "No dbt project found in this workspace.", cx);
        return;
    };
    match scaffold::initialize_el_workspace(&root) {
        Ok(created) => {
            notify(
                workspace,
                &format!("EL workspace ready — {} file(s) created under el/.", created.len()),
                cx,
            );
            let example = el_dir(&root).join("pipelines").join("example.yml");
            if example.is_file() {
                canvas_item::ElPipelineCanvas::deploy(workspace, root, example, window, cx);
            }
        }
        Err(error) => notify(workspace, &format!("EL init failed: {error:#}"), cx),
    }
}

fn notify(workspace: &mut Workspace, message: &str, cx: &mut Context<Workspace>) {
    struct ElNotification;
    workspace.show_toast(
        workspace::Toast::new(
            workspace::notifications::NotificationId::unique::<ElNotification>(),
            message.to_owned(),
        ),
        cx,
    );
}
