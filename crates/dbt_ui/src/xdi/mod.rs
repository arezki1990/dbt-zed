//! Semarchy xDI projects in the IDE: their lineage and an inventory, read
//! by `el_import` from the engine's repository. A left-dock panel lists the
//! mappings, processes, tables and problems; a tab draws the lineage around
//! a mapping, a table or a process; another tab holds the inventory. The
//! files themselves are never written, and a server's credentials are
//! never read.

pub mod inventory_view;
pub mod layout;
pub mod lineage_view;
pub mod panel;
pub mod store;

pub use inventory_view::XdiInventoryView;
pub use lineage_view::XdiLineageView;
pub use panel::XdiPanel;
pub use store::XdiStore;

use std::path::{Path, PathBuf};

use gpui::{Entity, Window, actions};
use workspace::Workspace;

actions!(
    xdi,
    [
        /// Toggles focus on the xDI panel.
        TogglePanelFocus,
        /// Opens the inventory of the xDI project.
        OpenInventory,
        /// Opens the lineage of the xDI mapping in the active editor.
        ShowLineage,
        /// Reads the xDI project again.
        Refresh,
    ]
);

/// What a lineage tab is drawn around.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    /// A node of the graph: a mapping's, a table's.
    Node(String),
    /// A process: the mappings it runs, and what they read and write.
    Process(String),
}

/// The xDI projects among the workspace's folders: a folder that is one,
/// or the folders right under it, each once.
pub fn detect(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut projects = Vec::new();
    for root in roots {
        for project in el_import::xdi::discover(root) {
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
    }
    projects
}

/// A count with its thousands grouped: `1,446`.
pub fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A mapping file, which the editor offers to show the lineage of.
pub fn is_mapping_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "map")
}

/// Whether `path` is a mapping of the workspace's xDI project: what the
/// editor offers the lineage of (other `.map` files, a JavaScript source
/// map, are not).
pub fn is_project_mapping(workspace: &Workspace, path: &Path, cx: &gpui::App) -> bool {
    is_mapping_file(path)
        && store(workspace, cx).is_some_and(|store| {
            store
                .read(cx)
                .active()
                .is_some_and(|project| path.starts_with(project))
        })
}

/// The id `el_import` gives the mapping at `path` in `project`.
pub fn job_id(project: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project).ok()?;
    let rel: Vec<String> = rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("job:xdi:{}", rel.join("/")))
}

/// The workspace's xDI store: the panel's.
pub fn store(workspace: &Workspace, cx: &gpui::App) -> Option<Entity<XdiStore>> {
    workspace
        .panel::<XdiPanel>(cx)
        .map(|panel| panel.read(cx).store.clone())
}

/// Opens (or recenters) the lineage tab of `store`'s project on `focus`.
/// The store is passed, not looked up: the panel calls this while it is
/// itself being updated, and reading it back then would panic.
pub fn show_lineage(
    workspace: &mut Workspace,
    store: Entity<XdiStore>,
    focus: Focus,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    XdiLineageView::deploy(workspace, store, focus, window, cx);
}

/// The `ShowLineage` action: the mapping in the active editor, else what
/// the lineage tab showed last.
pub fn show_lineage_for_active_editor(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let Some(store) = store(workspace, cx) else {
        return;
    };
    let path = workspace
        .active_item(cx)
        .and_then(|item| item.act_as::<editor::Editor>(cx))
        .and_then(|editor| {
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let file = buffer.read(cx).file()?.as_local()?.abs_path(cx);
            Some(file)
        });
    let Some(project) = store.read(cx).active().map(Path::to_path_buf) else {
        return;
    };
    let focus = path
        .filter(|path| is_mapping_file(path))
        .and_then(|path| job_id(&project, &path))
        .map(Focus::Node);
    match focus {
        Some(focus) => XdiLineageView::deploy(workspace, store, focus, window, cx),
        None => XdiLineageView::reveal(workspace, store, window, cx),
    }
}

pub fn open_inventory(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    if let Some(store) = store(workspace, cx) {
        XdiInventoryView::deploy(workspace, store, window, cx);
    }
}

pub fn refresh(workspace: &mut Workspace, _window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    if let Some(store) = store(workspace, cx) {
        store.update(cx, |store, cx| store.reload(cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_found_in_the_workspace_folders() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = el_import::fixtures::write(dir.path()).unwrap();
        // The folder holding both: the project one level down, the
        // metadata project (no mapping) left out.
        let project = std::slice::from_ref(&fixture.project);
        assert_eq!(detect(&[dir.path().to_path_buf()]), project);
        // The project itself, named twice: once.
        assert_eq!(
            detect(&[fixture.project.clone(), fixture.project.clone()]),
            project
        );
        assert!(detect(std::slice::from_ref(&fixture.metadata)).is_empty());
        let map = fixture.project.join("Orders/Mappings/LOAD_ORDERS.map");
        assert!(is_mapping_file(&map));
        assert!(!is_mapping_file(
            &fixture.project.join("Orders/Processes/EXPORT.proc")
        ));
        assert_eq!(
            job_id(&fixture.project, &map).as_deref(),
            Some("job:xdi:Orders/Mappings/LOAD_ORDERS.map")
        );
        assert_eq!(
            job_id(&fixture.project, Path::new("/elsewhere/x.map")),
            None
        );
        assert_eq!(
            [0, 7, 999, 1_446, 17_567, 1_000_000].map(grouped),
            ["0", "7", "999", "1,446", "17,567", "1,000,000"]
        );
    }
}
