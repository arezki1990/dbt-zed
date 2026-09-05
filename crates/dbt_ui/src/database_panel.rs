//! Left-dock Database explorer: the offline Database → Schema → Relation →
//! Column tree from [`crate::database`], with previews routed through the
//! existing results panel. No warehouse query is ever issued from here.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use editor::Editor;
use gpui::{
    App, AsyncWindowContext, ClipboardItem, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, MouseButton, Pixels, Point, Subscription, Task, UniformListScrollHandle,
    WeakEntity, Window, anchored, deferred, px,
};
use settings::Settings as _;
use ui::{ContextMenu, WithScrollbar, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::{
    ToggleDatabaseFocus,
    database::{ColumnState, DbCatalog, RelationKind, build_catalog},
    dbt_settings::DbtSettings,
    results_panel::{DbtResultsPanel, ShowTarget},
};

pub struct DbtDatabasePanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    catalog: Option<Arc<DbCatalog>>,
    pipelines: Vec<PathBuf>,
    load_error: Option<SharedString>,
    loading: bool,
    /// (manifest mtime, catalog mtime) at the last successful load, for the
    /// cheap staleness check on re-activation.
    loaded_mtimes: (Option<SystemTime>, Option<SystemTime>),
    /// Expanded node keys: "DB", "DB\u{1}SCHEMA", "DB\u{1}SCHEMA\u{1}REL".
    expanded: HashSet<SharedString>,
    filter_editor: Entity<Editor>,
    scroll: UniformListScrollHandle,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _load: Task<()>,
}

/// Everything one visible row needs to paint and act on, precomputed so the
/// uniform_list closure never touches the catalog.
struct RowEntry {
    depth: usize,
    /// Set on expandable rows; the chevron and (for db/schema) row click
    /// toggle it.
    key: Option<SharedString>,
    expanded: bool,
    label: SharedString,
    /// Muted right-hand detail: counts, types, materialization.
    detail: Option<SharedString>,
    action: RowAction,
}

enum RowAction {
    Toggle,
    Relation(RelationAction),
    Column { name: SharedString },
    Pipeline(PathBuf),
    InitializeEl,
    Note,
}

#[derive(Clone)]
struct RelationAction {
    name: SharedString,
    fqn: SharedString,
    /// Defining file relative to the project root.
    file_path: Option<PathBuf>,
    /// Set for dbt models: preview through ShowTarget::Model so the Compiled
    /// and Lineage tabs populate too.
    model: Option<(SharedString, PathBuf)>,
}

fn node_key(parts: &[&str]) -> SharedString {
    parts.join("\u{1}").into()
}

/// The dbt project root visible in this workspace: the configured
/// `dbt.project_dir` when valid, else a worktree root holding
/// dbt_project.yml, else one directory below it.
pub(crate) fn discover_workspace_root(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    let settings = DbtSettings::get_global(cx);
    for worktree in workspace.project().read(cx).worktrees(cx) {
        let root = worktree.read(cx).abs_path().to_path_buf();
        if let Some(dir) = &settings.project_dir {
            let candidate = if std::path::Path::new(dir).is_absolute() {
                PathBuf::from(dir)
            } else {
                root.join(dir)
            };
            if candidate.join("dbt_project.yml").is_file() {
                return Some(candidate);
            }
        }
        if root.join("dbt_project.yml").is_file() {
            return Some(root);
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten().take(64) {
                let path = entry.path();
                if path.join("dbt_project.yml").is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn artifact_mtimes(root: &std::path::Path) -> (Option<SystemTime>, Option<SystemTime>) {
    let mtime = |name: &str| {
        std::fs::metadata(root.join("target").join(name))
            .and_then(|meta| meta.modified())
            .ok()
    };
    (mtime("manifest.json"), mtime("catalog.json"))
}

impl DbtDatabasePanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            Self::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        cx.new(|cx| {
            let filter_editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Filter relations and columns…", window, cx);
                editor
            });
            cx.subscribe(
                &filter_editor,
                |_, _, event: &editor::EditorEvent, cx| {
                    if matches!(event, editor::EditorEvent::BufferEdited) {
                        cx.notify();
                    }
                },
            )
            .detach();

            Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                root: None,
                catalog: None,
                pipelines: Vec::new(),
                load_error: None,
                loading: false,
                loaded_mtimes: (None, None),
                expanded: HashSet::default(),
                filter_editor,
                scroll: UniformListScrollHandle::new(),
                context_menu: None,
                _load: Task::ready(()),
            }
        })
    }

    /// Loads (or reloads) the catalog when the artifacts changed since the
    /// last parse. Cheap when nothing changed: two mtime stats.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let root = match self.root.clone() {
            Some(root) => Some(root),
            None => {
                let discovered = self
                    .workspace
                    .upgrade()
                    .and_then(|workspace| discover_workspace_root(workspace.read(cx), cx));
                self.root = discovered.clone();
                discovered
            }
        };
        let Some(root) = root else {
            return;
        };
        // The pipelines listing is one read_dir — refresh it every pass so
        // the section tracks el/ even when the dbt artifacts are unchanged.
        self.pipelines = el_engine::spec::list_pipelines(&crate::el::el_dir(&root));
        let mtimes = artifact_mtimes(&root);
        if self.catalog.is_some() && mtimes == self.loaded_mtimes {
            return;
        }
        if self.loading {
            return;
        }
        self.loading = true;
        let task = cx.background_spawn(async move { build_catalog(&root) });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.loaded_mtimes = mtimes;
                match result {
                    Ok(catalog) => {
                        this.load_error = None;
                        // First load: open every database so the tree isn't a
                        // wall of closed chevrons.
                        if this.catalog.is_none() {
                            for db in &catalog.databases {
                                this.expanded.insert(node_key(&[db.name.as_ref()]));
                            }
                        }
                        this.catalog = Some(Arc::new(catalog));
                    }
                    Err(error) => this.load_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn toggle(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    /// The flat visible-row list for the current expansion and filter state.
    fn visible_rows(&self, cx: &App) -> Vec<RowEntry> {
        let query = self.filter_editor.read(cx).text(cx).trim().to_lowercase();
        let filtering = !query.is_empty();
        let mut rows = Vec::new();

        // EL pipelines section — present whether or not dbt artifacts exist.
        if !filtering {
            if self.pipelines.is_empty() {
                rows.push(RowEntry {
                    depth: 0,
                    key: None,
                    expanded: false,
                    label: "Initialize EL workspace…".into(),
                    detail: None,
                    action: RowAction::InitializeEl,
                });
            } else {
                for path in &self.pipelines {
                    let name = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("pipeline");
                    rows.push(RowEntry {
                        depth: 0,
                        key: None,
                        expanded: false,
                        label: format!("Pipeline: {name}").into(),
                        detail: Some("EL".into()),
                        action: RowAction::Pipeline(path.clone()),
                    });
                }
            }
        }

        let Some(catalog) = &self.catalog else {
            return rows;
        };

        if !catalog.catalog_present {
            rows.push(RowEntry {
                depth: 0,
                key: None,
                expanded: false,
                label: "No target/catalog.json — run `dbt compile --write-catalog` for column types."
                    .into(),
                detail: None,
                action: RowAction::Note,
            });
        }

        for db in &catalog.databases {
            let db_key = node_key(&[db.name.as_ref()]);
            // Under a filter the tree shows matching subtrees, fully expanded.
            let mut db_rows = Vec::new();
            for schema in &db.schemas {
                let schema_key = node_key(&[db.name.as_ref(), schema.name.as_ref()]);
                let mut schema_rows = Vec::new();
                for relation in &schema.relations {
                    let rel_key =
                        node_key(&[db.name.as_ref(), schema.name.as_ref(), relation.name.as_ref()]);
                    let name_match =
                        filtering && relation.name.to_lowercase().contains(&query);
                    let matching_columns: Vec<&crate::database::DbColumn> = match &relation.columns
                    {
                        ColumnState::Known(cols) if filtering && !name_match => cols
                            .iter()
                            .filter(|col| col.name.to_lowercase().contains(&query))
                            .collect(),
                        _ => Vec::new(),
                    };
                    if filtering && !name_match && matching_columns.is_empty() {
                        continue;
                    }

                    let expanded = if filtering {
                        !matching_columns.is_empty()
                    } else {
                        self.expanded.contains(&rel_key)
                    };
                    let detail = {
                        let mut parts = vec![relation.kind.label().to_owned()];
                        if let Some(rows) = relation.row_count {
                            parts.push(format!("{rows} rows"));
                        }
                        Some(SharedString::from(parts.join(" · ")))
                    };
                    schema_rows.push(RowEntry {
                        depth: 2,
                        key: Some(rel_key.clone()),
                        expanded,
                        label: relation.name.clone(),
                        detail,
                        action: RowAction::Relation(RelationAction {
                            name: relation.name.clone(),
                            fqn: relation.fqn.clone(),
                            file_path: relation.file_path.clone(),
                            model: match (&relation.kind, &relation.file_path) {
                                (RelationKind::Model { .. }, Some(path)) => {
                                    Some((relation.name.clone(), path.clone()))
                                }
                                _ => None,
                            },
                        }),
                    });
                    if expanded {
                        match &relation.columns {
                            ColumnState::Known(cols) => {
                                let iter: Vec<&crate::database::DbColumn> =
                                    if filtering && !matching_columns.is_empty() {
                                        matching_columns
                                    } else {
                                        cols.iter().collect()
                                    };
                                for col in iter {
                                    let mut detail = col.data_type.as_deref().map(str::to_lowercase);
                                    if let (Some(text), Some(_)) = (&mut detail, &col.description) {
                                        text.push_str(" · doc");
                                    }
                                    schema_rows.push(RowEntry {
                                        depth: 3,
                                        key: None,
                                        expanded: false,
                                        label: col.name.clone(),
                                        detail: detail.map(Into::into),
                                        action: RowAction::Column {
                                            name: col.name.clone(),
                                        },
                                    });
                                }
                            }
                            ColumnState::Unknown => schema_rows.push(RowEntry {
                                depth: 3,
                                key: None,
                                expanded: false,
                                label: "columns unknown — run `dbt compile --write-catalog`"
                                    .into(),
                                detail: None,
                                action: RowAction::Note,
                            }),
                        }
                    }
                }
                if schema_rows.is_empty() && filtering {
                    continue;
                }
                let schema_expanded = filtering || self.expanded.contains(&schema_key);
                db_rows.push(RowEntry {
                    depth: 1,
                    key: Some(schema_key),
                    expanded: schema_expanded,
                    label: schema.name.clone(),
                    detail: Some(format!("{}", schema.relations.len()).into()),
                    action: RowAction::Toggle,
                });
                if schema_expanded {
                    db_rows.append(&mut schema_rows);
                }
            }
            if db_rows.is_empty() && filtering {
                continue;
            }
            let db_expanded = filtering || self.expanded.contains(&db_key);
            rows.push(RowEntry {
                depth: 0,
                key: Some(db_key),
                expanded: db_expanded,
                label: db.name.clone(),
                detail: Some(format!("{}", db.schemas.len()).into()),
                action: RowAction::Toggle,
            });
            if db_expanded {
                rows.append(&mut db_rows);
            }
        }
        rows
    }

    /// Runs `select *` (or the model itself) through the shared results
    /// panel, so previews get the full grid: sorting, search, CSV export.
    fn preview_relation(
        &mut self,
        relation: &RelationAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let target = match &relation.model {
            Some((name, rel_path)) => ShowTarget::Model {
                name: name.to_lowercase().into(),
                rel_path: rel_path.clone(),
            },
            None => ShowTarget::Inline {
                sql: format!("select * from {}", relation.fqn),
                label: relation.name.clone(),
            },
        };
        self.workspace
            .update(cx, |workspace, cx| {
                let Some(panel) = workspace.panel::<DbtResultsPanel>(cx) else {
                    return;
                };
                workspace.focus_panel::<DbtResultsPanel>(window, cx);
                panel.update(cx, |panel, cx| panel.run_show(target, root, window, cx));
            })
            .ok();
    }

    fn open_relation_file(&mut self, rel_path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let path = root.join(rel_path);
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

    fn deploy_relation_menu(
        &mut self,
        relation: RelationAction,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            menu.context(self.focus_handle.clone())
                .entry("Preview data", None, {
                    let panel = panel.clone();
                    let relation = relation.clone();
                    move |window, cx| {
                        panel
                            .update(cx, |this, cx| {
                                this.preview_relation(&relation, window, cx);
                            })
                            .ok();
                    }
                })
                .when_some(relation.file_path.clone(), |menu, path| {
                    let panel = panel.clone();
                    menu.entry("Open model file", None, move |window, cx| {
                        panel
                            .update(cx, |this, cx| {
                                this.open_relation_file(&path, window, cx);
                            })
                            .ok();
                    })
                })
                .entry("Copy fully-qualified name", None, {
                    let fqn = relation.fqn.clone();
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(fqn.to_string()));
                    }
                })
                .entry("Copy name", None, {
                    let name = relation.name.clone();
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(name.to_string()));
                    }
                })
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn render_row(&self, row: &RowEntry, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let muted = cx.theme().colors().text_muted;
        let mut item = h_flex()
            .id(ix)
            .h(px(24.))
            .w_full()
            .flex_shrink_0()
            .pl(px(6. + row.depth as f32 * 14.))
            .pr_2()
            .gap_1()
            .items_center()
            .hover(|style| style.bg(cx.theme().colors().element_hover));

        if let Some(key) = &row.key {
            item = item.child(
                IconButton::new(
                    SharedString::from(format!("chev-{key}")),
                    if row.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                )
                .icon_size(IconSize::Small)
                .on_click(cx.listener({
                    let key = key.clone();
                    move |this, _, _, cx| this.toggle(key.clone(), cx)
                })),
            );
        } else {
            item = item.child(div().w(px(18.)).flex_shrink_0());
        }

        let (icon, label_color) = match &row.action {
            RowAction::Toggle if row.depth == 0 => (Some(IconName::DatabaseZap), Color::Default),
            RowAction::Toggle => (Some(IconName::Folder), Color::Default),
            RowAction::Relation(_) => (Some(IconName::Table), Color::Default),
            RowAction::Column { .. } => (None, Color::Muted),
            RowAction::Pipeline(_) => (Some(IconName::ArrowRightLeft), Color::Default),
            RowAction::InitializeEl => (Some(IconName::Plus), Color::Muted),
            RowAction::Note => (Some(IconName::Info), Color::Muted),
        };
        if let Some(icon) = icon {
            item = item.child(
                Icon::new(icon)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            );
        }
        item = item.child(
            div().min_w_0().flex_1().child(
                Label::new(row.label.clone())
                    .size(LabelSize::Small)
                    .color(label_color)
                    .truncate(),
            ),
        );
        if let Some(detail) = &row.detail {
            item = item.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(muted)
                    .child(detail.clone()),
            );
        }

        match &row.action {
            RowAction::Toggle => {
                if let Some(key) = row.key.clone() {
                    item = item.cursor_pointer().on_click(cx.listener(
                        move |this, _, _, cx| this.toggle(key.clone(), cx),
                    ));
                }
            }
            RowAction::Relation(relation) => {
                let click = relation.clone();
                let menu = relation.clone();
                item = item
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.preview_relation(&click, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.deploy_relation_menu(menu.clone(), event.position, window, cx);
                        }),
                    );
            }
            RowAction::Column { name } => {
                let name = name.clone();
                item = item.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let name = name.clone();
                            let menu = ContextMenu::build(window, cx, |menu, _, _| {
                                menu.context(this.focus_handle.clone()).entry(
                                    "Copy column name",
                                    None,
                                    move |_, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            name.to_string(),
                                        ));
                                    },
                                )
                            });
                            window.focus(&menu.focus_handle(cx), cx);
                            let subscription =
                                cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
                                    this.context_menu.take();
                                    cx.notify();
                                });
                            this.context_menu = Some((menu, event.position, subscription));
                            cx.notify();
                        }),
                    );
            }
            RowAction::Pipeline(path) => {
                let path = path.clone();
                item = item.cursor_pointer().on_click(cx.listener(
                    move |this, _, window, cx| {
                        let (Some(root), path) = (this.root.clone(), path.clone()) else {
                            return;
                        };
                        this.workspace
                            .update(cx, |workspace, cx| {
                                crate::el::ElPipelineCanvas::deploy(
                                    workspace, root, path, window, cx,
                                );
                            })
                            .ok();
                    },
                ));
            }
            RowAction::InitializeEl => {
                item = item.cursor_pointer().on_click(cx.listener(
                    |this, _, window, cx| {
                        this.workspace
                            .update(cx, |workspace, cx| {
                                crate::el::initialize_workspace(workspace, window, cx);
                            })
                            .ok();
                        this.loaded_mtimes = (None, None);
                        this.catalog = None;
                        this.ensure_loaded(cx);
                    },
                ));
            }
            RowAction::Note => {}
        }

        item.into_any_element()
    }
}

impl EventEmitter<PanelEvent> for DbtDatabasePanel {}

impl Focusable for DbtDatabasePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for DbtDatabasePanel {
    fn persistent_name() -> &'static str {
        "dbt Database Explorer"
    }

    fn panel_key() -> &'static str {
        "DbtDatabasePanel"
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

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::DatabaseZap)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("dbt Database Explorer")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleDatabaseFocus)
    }

    fn activation_priority(&self) -> u32 {
        4
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Lazy: a closed panel never reads the artifacts. Deferred because
        // set_active fires inside the Workspace's own update, and
        // ensure_loaded reads the Workspace to discover the project root —
        // reading it synchronously here double-leases and panics (the same
        // trap the results panel hit in http_client()).
        if active {
            cx.defer_in(window, |this, _, cx| this.ensure_loaded(cx));
        }
    }
}

impl Render for DbtDatabasePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Arc<Vec<RowEntry>> = Arc::new(self.visible_rows(cx));
        let row_elements: Vec<gpui::AnyElement> = Vec::new();
        drop(row_elements);
        let generated = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.catalog_generated_at.clone());

        let entity = cx.entity();
        let count = rows.len();
        let list = gpui::uniform_list("dbt-database-rows", count, {
            move |range, _window, cx| {
                entity.update(cx, |this, cx| {
                    range
                        .filter_map(|ix| {
                            rows.get(ix).map(|row| this.render_row(row, ix, cx))
                        })
                        .collect::<Vec<_>>()
                })
            }
        })
        .flex_1()
        .track_scroll(&self.scroll);

        let mut body = v_flex()
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("DbtDatabasePanel")
            .bg(cx.theme().colors().panel_background)
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        div()
                            .flex_1()
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(self.filter_editor.clone()),
                    )
                    .child(
                        IconButton::new("dbt-db-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text("Reload from dbt artifacts"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.loaded_mtimes = (None, None);
                                this.catalog = None;
                                this.ensure_loaded(cx);
                            })),
                    ),
            );

        if let Some(generated) = generated {
            body = body.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(10.))
                    .text_color(cx.theme().colors().text_muted)
                    .child(SharedString::from(format!(
                        "dbt-managed objects · catalog {generated}"
                    ))),
            );
        }

        if let Some(error) = &self.load_error {
            body = body.child(
                div()
                    .p_2()
                    .child(Label::new(error.clone()).size(LabelSize::Small).color(Color::Warning)),
            );
        } else if self.catalog.is_none() {
            let message: SharedString = if self.loading {
                "Loading dbt artifacts…".into()
            } else if self.root.is_none() {
                "No dbt project found in this workspace.".into()
            } else {
                "Open the panel to load target/manifest.json.".into()
            };
            body = body.child(
                v_flex().flex_1().items_center().justify_center().child(
                    Label::new(message).size(LabelSize::Small).color(Color::Muted),
                ),
            );
        } else {
            body = body.child(list).child(
                div().absolute().inset_0().child(div()).custom_scrollbars(
                    ui::Scrollbars::always_visible(ui::ScrollAxes::Vertical)
                        .tracked_scroll_handle(&self.scroll),
                    window,
                    cx,
                ),
            );
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
