//! The xDI panel, in the left dock when the workspace holds an xDI project:
//! its mappings by folder, its processes and their steps in the order they
//! run, its tables by use, and what could not be read. A click opens the
//! lineage around the item.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use editor::Editor;
use el_import::model::{IssueKind, LinkOn, Severity, StepKind};
use gpui::{
    App, AsyncWindowContext, ClipboardItem, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, MouseButton, Pixels, Point, Subscription, UniformListScrollHandle,
    WeakEntity, Window, anchored, deferred, px,
};
use ui::{ContextMenu, Indicator, Tooltip, WithScrollbar, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::store::{Snapshot, XdiStore};
use super::{Focus, TogglePanelFocus};

pub struct XdiPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    pub store: Entity<XdiStore>,
    filter_editor: Entity<Editor>,
    /// Folders and processes opened; sections closed.
    expanded: HashSet<SharedString>,
    closed: HashSet<Section>,
    scroll: UniformListScrollHandle,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Mappings,
    Processes,
    Tables,
    Problems,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Mappings => "Mappings",
            Section::Processes => "Processes",
            Section::Tables => "Tables",
            Section::Problems => "Problems",
        }
    }
}

#[derive(Clone)]
enum RowAction {
    Section(Section),
    Toggle(SharedString),
    Mapping {
        job: String,
        path: String,
    },
    Process {
        process: String,
        path: String,
        key: SharedString,
    },
    Step {
        job: Option<String>,
    },
    Table {
        node: String,
    },
    Problem {
        job: Option<String>,
        message: String,
    },
    Note,
}

/// One visible row, everything it draws precomputed.
struct Row {
    depth: usize,
    /// Rows that open and close: whether open.
    expanded: Option<bool>,
    icon: Option<IconName>,
    label: SharedString,
    detail: Option<SharedString>,
    tooltip: Option<SharedString>,
    muted: bool,
    dot: Option<Color>,
    action: RowAction,
}

/// So many tables listed at most, unless filtered.
const MAX_TABLES: usize = 200;

impl XdiPanel {
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
        let project = workspace.project().clone();
        cx.new(|cx| {
            let store = cx.new(|cx| XdiStore::new(project, cx));
            let filter_editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Filter mappings, processes, tables…", window, cx);
                editor
            });
            let subscriptions = vec![
                cx.observe(&store, |_, _, cx| cx.notify()),
                cx.subscribe(&filter_editor, |_, _, event: &editor::EditorEvent, cx| {
                    if matches!(event, editor::EditorEvent::BufferEdited) {
                        cx.notify();
                    }
                }),
            ];
            Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                store,
                filter_editor,
                expanded: HashSet::default(),
                closed: HashSet::from([Section::Tables, Section::Problems]),
                scroll: UniformListScrollHandle::new(),
                context_menu: None,
                _subscriptions: subscriptions,
            }
        })
    }

    fn toggle(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.closed.remove(&section) {
            self.closed.insert(section);
        }
        cx.notify();
    }

    fn show_lineage(&mut self, focus: Focus, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.workspace
            .update(cx, |workspace, cx| {
                super::show_lineage(workspace, store, focus, window, cx)
            })
            .ok();
    }

    fn open_file(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.store.read(cx).active().map(|root| root.to_path_buf()) else {
            return;
        };
        let path = root.join(rel);
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(
                        path,
                        workspace::OpenOptions {
                            focus: Some(true),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                    .detach();
            })
            .ok();
    }

    fn open_inventory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.workspace
            .update(cx, |workspace, cx| {
                super::XdiInventoryView::deploy(workspace, store, window, cx);
            })
            .ok();
    }

    /// The rows the panel shows, for the filter typed.
    fn visible_rows(&self, snapshot: &Snapshot, cx: &App) -> Vec<Row> {
        let query = self.filter_editor.read(cx).text(cx).trim().to_lowercase();
        let filtering = !query.is_empty();
        let matches = |text: &str| !filtering || text.to_lowercase().contains(&query);
        let project = &snapshot.project;
        let mut rows = Vec::new();

        // Which files have issues, and how bad the worst is.
        let mut worst: BTreeMap<&str, Severity> = BTreeMap::new();
        for issue in &project.issues {
            if let Some(path) = &issue.path {
                let entry = worst.entry(path.as_str()).or_insert(issue.severity);
                *entry = (*entry).min(issue.severity);
            }
        }
        let dot = |path: &str| {
            worst.get(path).map(|severity| match severity {
                Severity::Error => Color::Error,
                Severity::Warning => Color::Warning,
                Severity::Info => Color::Muted,
            })
        };

        let section = |rows: &mut Vec<Row>, section: Section, count: usize| -> bool {
            let open = filtering || !self.closed.contains(&section);
            rows.push(Row {
                depth: 0,
                expanded: Some(open),
                icon: None,
                label: section.label().into(),
                detail: Some(count.to_string().into()),
                tooltip: None,
                muted: false,
                dot: None,
                action: RowAction::Section(section),
            });
            open
        };

        // Mappings: by folder, or a flat list of what matches.
        let jobs: Vec<&el_import::model::Job> = project
            .jobs
            .iter()
            .filter(|job| matches(&job.path))
            .collect();
        if (!filtering || !jobs.is_empty()) && section(&mut rows, Section::Mappings, jobs.len()) {
            let mapping_row = |job: &el_import::model::Job, depth: usize| Row {
                depth,
                expanded: None,
                icon: Some(IconName::ArrowRightLeft),
                label: job.name.clone().into(),
                detail: Some(format!("{}→{}", job.sources.len(), job.targets.len()).into()),
                tooltip: Some(job.path.clone().into()),
                muted: job.copy_like,
                dot: dot(&job.path),
                action: RowAction::Mapping {
                    job: job.id.clone(),
                    path: job.path.clone(),
                },
            };
            if filtering {
                rows.extend(jobs.iter().map(|job| mapping_row(job, 1)));
            } else {
                let mut tree = Folder::default();
                for job in &jobs {
                    tree.insert(&job.folder, job);
                }
                tree.rows(&mut rows, "", 1, &self.expanded, &mapping_row);
            }
        }

        // Processes, each opening on its steps in the order they run.
        let processes: Vec<&el_import::model::Process> = project
            .processes
            .iter()
            .filter(|process| matches(&process.path))
            .collect();
        if (!filtering || !processes.is_empty())
            && section(&mut rows, Section::Processes, processes.len())
        {
            for process in processes {
                let key: SharedString = format!("p/{}", process.id).into();
                let open = self.expanded.contains(&key);
                rows.push(Row {
                    depth: 1,
                    expanded: Some(open),
                    icon: Some(IconName::PlayOutlined),
                    label: process.name.clone().into(),
                    detail: Some(format!("{} steps", process.steps.len()).into()),
                    tooltip: Some(process.path.clone().into()),
                    muted: process.copy_like,
                    dot: dot(&process.path),
                    action: RowAction::Process {
                        process: process.id.clone(),
                        path: process.path.clone(),
                        key: key.clone(),
                    },
                });
                if !open {
                    continue;
                }
                for step in process.steps_in_order() {
                    let reached: Vec<LinkOn> = process
                        .links
                        .iter()
                        .filter(|link| link.to == step.element_id)
                        .map(|link| link.on)
                        .collect();
                    let prefix = if reached.contains(&LinkOn::Ko)
                        && !reached.iter().any(|on| *on != LinkOn::Ko)
                    {
                        "KO › "
                    } else {
                        ""
                    };
                    let job = match &step.kind {
                        StepKind::RunMapping { job, .. } => job.clone(),
                        _ => None,
                    };
                    let unresolved = matches!(
                        &step.kind,
                        StepKind::RunMapping { job: None, .. }
                            | StepKind::RunProcess { process: None, .. }
                    );
                    let detail = match &step.kind {
                        StepKind::Sql {
                            connection: Some(connection),
                        } => format!("SQL · {connection}"),
                        kind => kind.label().to_owned(),
                    };
                    rows.push(Row {
                        depth: 2,
                        expanded: None,
                        icon: None,
                        label: format!("{prefix}{}", step.name).into(),
                        detail: Some(detail.into()),
                        tooltip: unresolved.then(|| "Not found in this project".into()),
                        muted: !step.enabled,
                        dot: unresolved.then_some(Color::Warning),
                        action: RowAction::Step { job },
                    });
                }
            }
        }

        // Tables, by how many mappings read and write them.
        let mut tables: Vec<(&el_import::lineage::Node, usize, usize)> = snapshot
            .index
            .graph
            .nodes
            .iter()
            .filter(|node| node.kind != "job" && matches(&node.label))
            .map(|node| {
                let reads = snapshot.index.downstream(&node.id).len();
                let writes = snapshot.index.upstream(&node.id).len();
                (node, reads, writes)
            })
            .collect();
        tables.sort_by(|a, b| {
            (b.1 + b.2)
                .cmp(&(a.1 + a.2))
                .then(a.0.label.cmp(&b.0.label))
        });
        if (!filtering || !tables.is_empty()) && section(&mut rows, Section::Tables, tables.len()) {
            let shown = if filtering { tables.len() } else { MAX_TABLES };
            for (node, reads, writes) in tables.iter().take(shown) {
                let technology = node
                    .detail
                    .get("technology")
                    .and_then(|technology| technology.as_str())
                    .unwrap_or_default();
                rows.push(Row {
                    depth: 1,
                    expanded: None,
                    icon: Some(if node.kind == "table" {
                        IconName::Table
                    } else {
                        IconName::FileCode
                    }),
                    label: node.label.clone().into(),
                    detail: Some(format!("{reads} read · {writes} write").into()),
                    tooltip: Some(
                        match &node.connection {
                            Some(server) => format!("{technology} · {server}"),
                            None => technology.to_owned(),
                        }
                        .into(),
                    ),
                    muted: node.detail.get("resolved") == Some(&serde_json::Value::Bool(false)),
                    dot: None,
                    action: RowAction::Table {
                        node: node.id.clone(),
                    },
                });
            }
            if tables.len() > shown {
                rows.push(Row {
                    depth: 1,
                    expanded: None,
                    icon: None,
                    label: format!("{} more — filter to find them", tables.len() - shown).into(),
                    detail: None,
                    tooltip: None,
                    muted: true,
                    dot: None,
                    action: RowAction::Note,
                });
            }
        }

        // What could not be read.
        let problems: Vec<&el_import::inventory::Problem> = snapshot
            .inventory
            .problems
            .iter()
            .filter(|problem| {
                matches(&problem.message) || problem.path.as_deref().is_some_and(matches)
            })
            .collect();
        if (!filtering || !problems.is_empty())
            && section(&mut rows, Section::Problems, problems.len())
        {
            for problem in problems {
                let job = problem
                    .path
                    .as_ref()
                    .filter(|path| path.ends_with(".map"))
                    .map(|path| format!("job:xdi:{path}"));
                rows.push(Row {
                    depth: 1,
                    expanded: None,
                    icon: None,
                    label: problem
                        .path
                        .clone()
                        .unwrap_or_else(|| problem.message.clone())
                        .into(),
                    detail: Some(problem.kind.label().into()),
                    tooltip: Some(problem.message.clone().into()),
                    muted: false,
                    dot: Some(match problem.severity {
                        Severity::Error => Color::Error,
                        Severity::Warning => Color::Warning,
                        Severity::Info => Color::Muted,
                    }),
                    action: RowAction::Problem {
                        job: job.filter(|_| problem.kind != IssueKind::Malformed),
                        message: problem.message.clone(),
                    },
                });
            }
        }
        rows
    }

    fn deploy_menu(
        &mut self,
        action: RowAction,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            let menu = menu.context(self.focus_handle.clone());
            match action {
                RowAction::Mapping { job, path }
                | RowAction::Process {
                    process: job, path, ..
                } => {
                    let is_process = job.starts_with("process:");
                    let lineage = if is_process {
                        Focus::Process(job)
                    } else {
                        Focus::Node(job)
                    };
                    let open = path.clone();
                    menu.entry("Open file", None, {
                        let panel = panel.clone();
                        move |window, cx| {
                            panel
                                .update(cx, |this, cx| this.open_file(&open, window, cx))
                                .ok();
                        }
                    })
                    .entry(
                        if is_process {
                            "Show process lineage"
                        } else {
                            "Show lineage"
                        },
                        None,
                        {
                            let panel = panel.clone();
                            move |window, cx| {
                                panel
                                    .update(cx, |this, cx| {
                                        this.show_lineage(lineage.clone(), window, cx)
                                    })
                                    .ok();
                            }
                        },
                    )
                    .entry("Copy path", None, move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                    })
                }
                RowAction::Table { node } => menu
                    .entry("Show lineage", None, {
                        let panel = panel.clone();
                        let node = node.clone();
                        move |window, cx| {
                            panel
                                .update(cx, |this, cx| {
                                    this.show_lineage(Focus::Node(node.clone()), window, cx)
                                })
                                .ok();
                        }
                    })
                    .entry("Copy location", None, move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(node.clone()));
                    }),
                RowAction::Problem { message, .. } => {
                    menu.entry("Copy message", None, move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(message.clone()));
                    })
                }
                _ => menu,
            }
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn deploy_project_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let projects = store.read(cx).projects().to_vec();
        let menu = ContextMenu::build(window, cx, |mut menu, _, _| {
            menu = menu.context(self.focus_handle.clone());
            for project in projects {
                let label = project
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| project.display().to_string());
                let store = store.clone();
                menu = menu.entry(label, None, move |_, cx| {
                    store.update(cx, |store, cx| store.set_active(project.clone(), cx));
                });
            }
            menu
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn deploy_metadata_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let picked = store.read(cx).has_picked();
        let project = store.read(cx).active().map(PathBuf::from);
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            let menu =
                menu.context(self.focus_handle.clone())
                    .entry("Choose metadata folders…", None, {
                        let store = store.clone();
                        move |_, cx| store.update(cx, |store, cx| store.pick_metadata_folders(cx))
                    });
            match (picked, project) {
                (true, Some(project)) => menu.entry(
                    "Use the folders found beside the project",
                    None,
                    move |_, cx| {
                        store.update(cx, |store, cx| {
                            store.set_metadata_folders(project.clone(), None, cx)
                        })
                    },
                ),
                _ => menu,
            }
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn render_row(&self, row: &Row, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let mut item = h_flex()
            .id(("xdi-row", ix))
            .h(px(24.))
            .w_full()
            .flex_shrink_0()
            .pl(px(6. + row.depth as f32 * 14.))
            .pr_2()
            .gap_1()
            .items_center()
            .hover(|style| style.bg(colors.element_hover));
        match row.expanded {
            Some(open) => {
                item = item.child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(IconSize::Small)
                    .color(Color::Muted),
                );
            }
            None => item = item.child(div().w(px(14.)).flex_shrink_0()),
        }
        if let Some(icon) = row.icon {
            item = item.child(Icon::new(icon).size(IconSize::Small).color(Color::Muted));
        }
        let section = matches!(row.action, RowAction::Section(_));
        item = item.child(
            div().min_w_0().flex_1().child(
                Label::new(row.label.clone())
                    .size(LabelSize::Small)
                    .color(if row.muted {
                        Color::Muted
                    } else {
                        Color::Default
                    })
                    .when(section, |label| label.weight(gpui::FontWeight::SEMIBOLD))
                    .truncate(),
            ),
        );
        if let Some(color) = row.dot {
            item = item.child(Indicator::dot().color(color));
        }
        if let Some(detail) = &row.detail {
            item = item.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(colors.text_muted)
                    .child(detail.clone()),
            );
        }
        if let Some(tooltip) = row.tooltip.clone() {
            item = item.tooltip(Tooltip::text(tooltip));
        }
        let action = row.action.clone();
        item = item
            .cursor_pointer()
            .on_click(cx.listener({
                let action = action.clone();
                move |this, _, window, cx| match &action {
                    RowAction::Section(section) => this.toggle_section(*section, cx),
                    RowAction::Toggle(key) => this.toggle(key.clone(), cx),
                    RowAction::Mapping { job, .. } => {
                        this.show_lineage(Focus::Node(job.clone()), window, cx)
                    }
                    RowAction::Process { key, .. } => this.toggle(key.clone(), cx),
                    RowAction::Step { job: Some(job) } => {
                        this.show_lineage(Focus::Node(job.clone()), window, cx)
                    }
                    RowAction::Table { node } => {
                        this.show_lineage(Focus::Node(node.clone()), window, cx)
                    }
                    RowAction::Problem { job: Some(job), .. } => {
                        this.show_lineage(Focus::Node(job.clone()), window, cx)
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.deploy_menu(action.clone(), event.position, window, cx);
                }),
            );
        item.into_any_element()
    }

    fn render_header(
        &self,
        snapshot: Option<&Arc<Snapshot>>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let store = self.store.read(cx);
        let colors = cx.theme().colors();
        let name = store
            .active()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let several = store.projects().len() > 1;
        let loading = store.loading();
        let mut header = v_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_0p5()
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div().min_w_0().flex_1().child(
                            Label::new(name)
                                .size(LabelSize::Small)
                                .weight(gpui::FontWeight::SEMIBOLD)
                                .truncate(),
                        ),
                    )
                    .when(several, |row| {
                        row.child(
                            IconButton::new("xdi-projects", IconName::ChevronDown)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Another xDI project of the workspace"))
                                .on_click(cx.listener(
                                    |this, event: &gpui::ClickEvent, window, cx| {
                                        this.deploy_project_menu(event.position(), window, cx)
                                    },
                                )),
                        )
                    })
                    .when(loading, |row| {
                        row.child(
                            Label::new("Reading…")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        IconButton::new("xdi-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Read the project again"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.reload(cx));
                            })),
                    )
                    .child(
                        IconButton::new("xdi-inventory", IconName::ListTree)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Inventory"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_inventory(window, cx)),
                            ),
                    )
                    .child(
                        IconButton::new("xdi-metadata", IconName::Folder)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Metadata folders"))
                            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                                this.deploy_metadata_menu(event.position(), window, cx)
                            })),
                    ),
            );
        if let Some(snapshot) = snapshot {
            let project = &snapshot.project;
            let stats = &project.stats;
            header = header.child(
                Label::new(format!(
                    "{} mappings · {} processes · {} datasets",
                    super::grouped(project.jobs.len()),
                    super::grouped(project.processes.len()),
                    super::grouped(project.datasets.len())
                ))
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            );
            let roots: Vec<String> = project
                .metadata
                .iter()
                .map(|root| {
                    let name = root
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.path.display().to_string());
                    match root.status {
                        el_import::model::RootStatus::Ok => name,
                        el_import::model::RootStatus::Missing => format!("{name} (not there)"),
                        el_import::model::RootStatus::NotMetadata => {
                            format!("{name} (no metadata)")
                        }
                    }
                })
                .collect();
            let missing = project
                .metadata
                .iter()
                .any(|root| root.status != el_import::model::RootStatus::Ok);
            let total = stats.datastores + stats.path_datastores;
            let resolved = stats.resolved + stats.path_resolved;
            header = header.child(
                Label::new(if roots.is_empty() {
                    "Metadata: none found beside the project".to_owned()
                } else {
                    format!(
                        "Metadata: {} · {}/{} resolved",
                        roots.join(", "),
                        super::grouped(resolved),
                        super::grouped(total)
                    )
                })
                .size(LabelSize::XSmall)
                .color(if missing || roots.is_empty() {
                    Color::Warning
                } else {
                    Color::Muted
                }),
            );
        }
        header.into_any_element()
    }
}

/// Mappings by folder, for the tree.
#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<String, Folder<'a>>,
    jobs: Vec<&'a el_import::model::Job>,
}

impl<'a> Folder<'a> {
    fn insert(&mut self, folder: &str, job: &'a el_import::model::Job) {
        let mut node = self;
        for part in folder.split('/').filter(|part| !part.is_empty()) {
            node = node.folders.entry(part.to_owned()).or_default();
        }
        node.jobs.push(job);
    }

    fn count(&self) -> usize {
        self.jobs.len() + self.folders.values().map(Folder::count).sum::<usize>()
    }

    fn rows(
        &self,
        rows: &mut Vec<Row>,
        path: &str,
        depth: usize,
        expanded: &HashSet<SharedString>,
        mapping: &dyn Fn(&el_import::model::Job, usize) -> Row,
    ) {
        for (name, folder) in &self.folders {
            let folder_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            let key: SharedString = format!("m/{folder_path}").into();
            let open = expanded.contains(&key);
            rows.push(Row {
                depth,
                expanded: Some(open),
                icon: Some(if open {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                }),
                label: name.clone().into(),
                detail: Some(folder.count().to_string().into()),
                tooltip: None,
                muted: false,
                dot: None,
                action: RowAction::Toggle(key),
            });
            if open {
                folder.rows(rows, &folder_path, depth + 1, expanded, mapping);
            }
        }
        for job in &self.jobs {
            rows.push(mapping(job, depth));
        }
    }
}

impl EventEmitter<PanelEvent> for XdiPanel {}

impl Focusable for XdiPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for XdiPanel {
    fn persistent_name() -> &'static str {
        "xDI Panel"
    }

    fn panel_key() -> &'static str {
        "XdiPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _window: &Window, cx: &App) -> Option<IconName> {
        // Only a workspace with an xDI project shows the button.
        (!self.store.read(cx).projects().is_empty()).then_some(IconName::GitGraph)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("xDI")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(TogglePanelFocus)
    }

    fn activation_priority(&self) -> u32 {
        11
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Deferred: set_active runs inside the workspace's own update.
        if active {
            cx.defer_in(window, |this, _, cx| {
                let store = this.store.read(cx);
                if store.snapshot().is_none() && !store.loading() {
                    this.store.update(cx, |store, cx| store.reload(cx));
                }
            });
        }
    }
}

impl Render for XdiPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let snapshot = store.snapshot().cloned();
        let has_project = store.active().is_some();
        let detected = store.detected();
        let loading = store.loading();
        let background = cx.theme().colors().panel_background;
        let border = cx.theme().colors().border;
        let header = self.render_header(snapshot.as_ref(), cx);
        let mut body = v_flex()
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("XdiPanel")
            .bg(background)
            .child(header)
            .child(
                h_flex().w_full().p_1().child(
                    div()
                        .flex_1()
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(border)
                        .child(self.filter_editor.clone()),
                ),
            );
        match snapshot {
            Some(snapshot) => {
                let rows: Arc<Vec<Row>> = Arc::new(self.visible_rows(&snapshot, cx));
                if rows.is_empty() {
                    body = body.child(
                        v_flex().flex_1().items_center().justify_center().child(
                            Label::new("No matches.")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    );
                } else {
                    let entity = cx.entity();
                    let count = rows.len();
                    let list = gpui::uniform_list("xdi-rows", count, move |range, _window, cx| {
                        entity.update(cx, |this, cx| {
                            range
                                .filter_map(|ix| {
                                    rows.get(ix).map(|row| this.render_row(row, ix, cx))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .flex_1()
                    .track_scroll(&self.scroll);
                    body = body.child(list).child(
                        div().absolute().inset_0().child(div()).custom_scrollbars(
                            ui::Scrollbars::always_visible(ui::ScrollAxes::Vertical)
                                .tracked_scroll_handle(&self.scroll),
                            window,
                            cx,
                        ),
                    );
                }
            }
            None => {
                let message = if !detected {
                    "Looking for an xDI project…"
                } else if !has_project {
                    "No xDI project in this workspace: a folder whose .project names xDI's builder, with mappings or processes."
                } else if loading {
                    "Reading the project…"
                } else {
                    "Open the panel to read the project."
                };
                body = body.child(
                    v_flex()
                        .flex_1()
                        .p_2()
                        .items_center()
                        .justify_center()
                        .child(
                            Label::new(message)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                );
            }
        }
        body.children(self.context_menu.as_ref().map(|(menu, position, _)| {
            deferred(
                anchored()
                    .position(*position)
                    .anchor(gpui::Anchor::TopLeft)
                    .child(menu.clone()),
            )
            .with_priority(3)
        }))
    }
}

#[cfg(test)]
mod render_tests {
    use gpui::{TestAppContext, VisualTestContext, size};
    use project::Project;
    use workspace::MultiWorkspace;

    use super::*;
    use crate::xdi::store::fixture_snapshot;

    /// Draws the panel headlessly in each state it can be in: what this
    /// catches is a state that cannot be drawn at all.
    #[gpui::test]
    async fn the_panel_draws_in_every_state(cx: &mut TestAppContext) {
        let app_state = cx.update(|cx| {
            let app_state = workspace::AppState::test(cx);
            editor::init(cx);
            app_state
        });
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let window =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window
            .read_with(cx, |multi, _| multi.workspace().clone())
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            XdiPanel::new(workspace, window, cx)
        });
        let draw = |label: &str, cx: &mut VisualTestContext| {
            cx.draw(gpui::Point::default(), size(px(320.), px(900.)), |_, _| {
                panel.clone().into_any_element()
            });
            cx.run_until_parked();
            log::info!("drew: {label}");
        };
        draw("no project", cx);

        let dir = tempfile::tempdir().unwrap();
        let (root, snapshot) = fixture_snapshot(dir.path());
        let store = panel.read_with(cx, |panel, _| panel.store.clone());
        store.update(cx, |store, cx| {
            store.set_snapshot(root.clone(), None, cx);
            store.set_loading(true);
        });
        draw("reading", cx);
        store.update(cx, |store, cx| {
            store.set_loading(false);
            store.set_snapshot(root.clone(), Some(snapshot), cx);
        });
        draw("read", cx);

        // Docked, as in the app: a row's click opens the lineage from
        // inside the panel's own update, which must not read the panel
        // back from the workspace (that once panicked on a double lease).
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_panel(panel.clone(), window, cx);
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.show_lineage(
                Focus::Node("job:xdi:Orders/Mappings/LOAD_ORDERS.map".to_owned()),
                window,
                cx,
            );
            panel.show_lineage(
                Focus::Process("process:xdi:Orders/Processes/RUN_ORDERS.proc".to_owned()),
                window,
                cx,
            );
        });
        cx.run_until_parked();
        let tabs = workspace.read_with(cx, |workspace, cx| {
            workspace
                .items_of_type::<super::super::XdiLineageView>(cx)
                .count()
        });
        assert_eq!(tabs, 1, "one lineage tab, recentered");
        panel.update_in(cx, |panel, window, cx| panel.open_inventory(window, cx));
        cx.run_until_parked();
        let inventories = workspace.read_with(cx, |workspace, cx| {
            workspace
                .items_of_type::<super::super::XdiInventoryView>(cx)
                .count()
        });
        assert_eq!(inventories, 1);

        // Every section open, every folder and process too.
        panel.update(cx, |panel, cx| {
            panel.closed.clear();
            for key in [
                "m/Orders",
                "m/Orders/Mappings",
                "p/process:xdi:Orders/Processes/RUN_ORDERS.proc",
                "p/process:xdi:Orders/Processes/EXPORT.proc",
            ] {
                panel.expanded.insert(key.into());
            }
            cx.notify();
        });
        draw("all open", cx);

        // Filtered, then filtered to nothing.
        for text in ["orders", "no such thing"] {
            panel.update_in(cx, |panel, window, cx| {
                panel
                    .filter_editor
                    .update(cx, |editor, cx| editor.set_text(text, window, cx));
            });
            draw("filtered", cx);
        }

        // Read with its metadata folder missing.
        let (_, mut missing) = fixture_snapshot(&dir.path().join("again"));
        for root in &mut missing.project.metadata {
            root.status = el_import::model::RootStatus::Missing;
        }
        store.update(cx, |store, cx| {
            store.set_snapshot(root.clone(), Some(missing), cx)
        });
        panel.update_in(cx, |panel, window, cx| {
            panel
                .filter_editor
                .update(cx, |editor, cx| editor.set_text("", window, cx));
        });
        draw("metadata folder missing", cx);
    }
}
