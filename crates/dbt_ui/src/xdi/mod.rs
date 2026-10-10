//! Semarchy xDI projects in the IDE: their lineage and an inventory, read
//! by `el_import` from the engine's repository. A left-dock panel lists the
//! mappings, processes, tables and problems; a tab draws the lineage around
//! a mapping, a table or a process; another tab holds the inventory. A
//! project converts to an Ellode workspace in a folder beside it. Its own
//! files are never written, and a server's credentials are never read.

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

use gpui::{Entity, PromptLevel, Window, actions};
use settings::Settings as _;
use workspace::Workspace;

use crate::dbt_settings::DbtSettings;

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
        /// Converts the xDI project to an Ellode workspace, in a folder
        /// beside it.
        ConvertToEllode,
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

/// Where `project` converts to: a folder beside it, `<name>_ellode`.
pub fn ellode_folder(project: &Path) -> PathBuf {
    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "xdi".to_owned());
    project.with_file_name(format!("{name}_ellode"))
}

/// What a conversion wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub folder: PathBuf,
    pub models: usize,
    pub pipelines: usize,
    pub flows: usize,
    /// The mappings it did not convert.
    pub left: usize,
}

impl Converted {
    /// `Converted to Sales_ellode: 4 dbt models, 4 pipelines, 2 flows; 1
    /// mapping left to convert by hand.`
    pub fn summary(&self) -> String {
        let folder = self
            .folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let plural = |count: usize, one: &str, many: &str| {
            format!("{} {}", grouped(count), if count == 1 { one } else { many })
        };
        let mut summary = format!(
            "Converted to {folder}: {}, {}, {}",
            plural(self.models, "dbt model", "dbt models"),
            plural(self.pipelines, "pipeline", "pipelines"),
            plural(self.flows, "flow", "flows"),
        );
        if self.left > 0 {
            summary.push_str(&format!(
                "; {} left to convert by hand",
                plural(self.left, "mapping", "mappings")
            ));
        }
        summary.push('.');
        summary
    }
}

/// Converts `project` into `folder`, as `options` say: what `el_import`
/// makes of it, written there over the files of the same name.
pub fn convert(
    project: &el_import::model::Project,
    options: &el_import::convert::Options,
    folder: &Path,
) -> std::io::Result<Converted> {
    let conversion = el_import::convert::convert(project, options);
    el_import::convert::write(&conversion, folder)?;
    let count = |prefix: &str, suffix: &str| {
        conversion
            .files
            .iter()
            .filter(|file| file.path.starts_with(prefix) && file.path.ends_with(suffix))
            .count()
    };
    Ok(Converted {
        folder: folder.to_path_buf(),
        models: count("models/", ".sql"),
        pipelines: count("el/pipelines/", ".yml"),
        flows: count("el/flows/", ".yml"),
        left: conversion.report.count(el_import::convert::Outcome::is_gap),
    })
}

/// The `ConvertToEllode` action.
pub fn convert_to_ellode(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    if let Some(store) = store(workspace, cx) {
        convert_project(workspace, store, window, cx);
    }
}

/// Converts `store`'s project, as last read, into the folder beside it —
/// asked first when that folder holds files — and opens its CONVERSION.md.
/// The store is passed, not looked up, as for [`show_lineage`].
pub fn convert_project(
    workspace: &mut Workspace,
    store: Entity<XdiStore>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let store = store.read(cx);
    let (Some(project), Some(snapshot)) = (
        store.active().map(Path::to_path_buf),
        store.snapshot().cloned(),
    ) else {
        crate::el::toast(workspace, "The xDI project is not read yet.", cx);
        return;
    };
    let settings = DbtSettings::get_global(cx);
    let options = el_import::convert::Options {
        warehouse: settings.xdi_warehouse.clone(),
        dbt_schema: settings.xdi_dbt_schema.clone(),
        raw_schema: settings.xdi_raw_schema.clone(),
    };
    let folder = ellode_folder(&project);
    cx.spawn_in(window, async move |workspace, cx| {
        let probe = folder.clone();
        let occupied = cx
            .background_executor()
            .spawn(async move {
                std::fs::read_dir(&probe).is_ok_and(|mut entries| entries.next().is_some())
            })
            .await;
        if occupied {
            let Ok(answer) = workspace.update_in(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    &format!("Convert into {}?", folder.display()),
                    Some(
                        "The folder holds files already: the conversion writes its own over \
                         those of the same name, and leaves the others.",
                    ),
                    &["Convert", "Cancel"],
                    cx,
                )
            }) else {
                return;
            };
            if answer.await != Ok(0) {
                return;
            }
        }
        let target = folder.clone();
        let converted = cx
            .background_executor()
            .spawn(async move { convert(&snapshot.project, &options, &target) })
            .await;
        workspace
            .update_in(cx, |workspace, window, cx| match converted {
                Ok(converted) => {
                    crate::el::toast(workspace, &converted.summary(), cx);
                    workspace
                        .open_abs_path(
                            converted.folder.join("CONVERSION.md"),
                            workspace::OpenOptions {
                                focus: Some(true),
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                        .detach();
                }
                Err(error) => crate::el::toast_error(
                    workspace,
                    &format!("The conversion could not be written: {error}"),
                    None,
                    cx,
                ),
            })
            .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_converts_into_a_folder_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let (root, snapshot) = store::fixture_snapshot(dir.path());
        let folder = ellode_folder(&root);
        assert_eq!(folder, root.with_file_name("Sales_ellode"));
        let converted = convert(
            &snapshot.project,
            &el_import::convert::Options::default(),
            &folder,
        )
        .unwrap();
        assert_eq!(
            (
                converted.models,
                converted.pipelines,
                converted.flows,
                converted.left
            ),
            (4, 4, 2, 1)
        );
        assert!(folder.join("CONVERSION.md").is_file());
        assert!(folder.join("el/connections.yml").is_file());
        assert!(folder.join("el/flows/run_orders.yml").is_file());
        assert_eq!(
            converted.summary(),
            "Converted to Sales_ellode: 4 dbt models, 4 pipelines, 2 flows; 1 mapping left to \
             convert by hand."
        );
    }

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
