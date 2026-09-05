//! The EL panel: the extract-load plugin's own left-dock surface —
//! pipelines explorer, connections at a glance (names and kinds only,
//! never values), New pipeline, Initialize. Fully independent of the dbt
//! panels; works in standalone EL projects with no dbt_project.yml.

use std::path::PathBuf;

use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use ui::{Tooltip, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::ToggleElPanelFocus;

pub struct ElPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    pipelines: Vec<PathBuf>,
    /// (name, kind) — the credential posture: never values.
    connections: Vec<(SharedString, SharedString)>,
    /// connections.yml failed to load (parse error — not absence).
    connections_error: Option<SharedString>,
    /// The active profile (dev/recette/prod…) and all declared ones.
    profile: Option<SharedString>,
    profiles: Vec<SharedString>,
    /// Connections whose table list is unfolded in the explorer.
    expanded: std::collections::HashSet<SharedString>,
    tables: std::collections::HashMap<SharedString, TablesState>,
    _list_tasks: std::collections::HashMap<SharedString, Task<()>>,
    scroll: UniformListScrollHandle,
    _refresh: Task<()>,
}

enum TablesState {
    Loading,
    Loaded(Vec<(String, String)>),
    Failed(SharedString),
}

enum Row {
    Header(SharedString),
    Pipeline(PathBuf),
    Connection(SharedString, SharedString),
    AddConnection,
    Table {
        connection: SharedString,
        schema: String,
        table: String,
    },
    ConnNote(SharedString, Color),
    NewPipeline,
    Initialize,
    Note(SharedString),
}

/// Kinds the explorer can browse — the worker's list/query support.
fn browsable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres")
}

/// The pill that follows the cursor while a table is dragged.
struct DraggedTablePreview(SharedString);

impl gpui::Render for DraggedTablePreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.elevated_surface_background)
            .shadow_md()
            .child(Icon::new(IconName::Table).size(IconSize::XSmall).color(Color::Muted))
            .child(Label::new(self.0.clone()).size(LabelSize::Small))
    }
}

impl ElPanel {
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
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            workspace: workspace_handle,
            root: None,
            pipelines: Vec::new(),
            connections: Vec::new(),
            connections_error: None,
            profile: None,
            profiles: Vec::new(),
            expanded: Default::default(),
            tables: Default::default(),
            _list_tasks: Default::default(),
            scroll: UniformListScrollHandle::new(),
            _refresh: Task::ready(()),
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone().or_else(|| {
            self.workspace
                .upgrade()
                .and_then(|workspace| super::discover_el_root(workspace.read(cx), cx))
        });
        self.root = root.clone();
        let Some(root) = root else {
            return;
        };
        let el = super::el_dir(&root);
        // Self-heal the derived JSON schemas the YAML headers point at —
        // hand-made projects never ran Initialize.
        if let Err(error) = super::scaffold::ensure_schemas(&root) {
            log::warn!("el: could not write schemas: {error:#}");
        }
        self.pipelines = el_engine::spec::list_pipelines(&el);
        match el_engine::spec::load_active_connections(&root) {
            Ok((connections, profile)) => {
                self.profile = profile.map(Into::into);
                self.profiles = el_engine::spec::load_connections(&el.join("connections.yml"))
                    .map(|raw| raw.profiles.keys().map(|name| name.clone().into()).collect())
                    .unwrap_or_default();
                self.connections = connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        (name.clone().into(), connection.kind().to_owned().into())
                    })
                    .collect();
                self.connections_error = None;
            }
            Err(el_engine::spec::SpecError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                self.connections = Vec::new();
                self.connections_error = None;
            }
            Err(error) => {
                // A broken file is not an empty file — say so instead of
                // rendering a list that invites a destructive rewrite.
                self.connections = Vec::new();
                self.connections_error =
                    Some(format!("connections.yml could not be read: {error}").into());
            }
        }
        cx.notify();
    }

    /// Switches the checkout's active profile: writes the local selection
    /// file (never the shared YAML), drops caches, and reloads every open
    /// canvas so validation and labels track the new environment.
    fn switch_profile(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let path = el_engine::spec::profile_selection_path(&root);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, name.as_ref()) {
            self.connections_error =
                Some(format!("could not save the profile selection: {error}").into());
            cx.notify();
            return;
        }
        self.tables.clear();
        self.expanded.clear();
        self.refresh(cx);
        self.workspace
            .update(cx, |workspace, cx| {
                let canvases: Vec<_> =
                    workspace.items_of_type::<super::ElPipelineCanvas>(cx).collect();
                for canvas in canvases {
                    canvas.update(cx, |canvas, cx| canvas.reload(cx));
                }
                super::toast(workspace, &format!("Profile: {name}"), cx);
            })
            .ok();
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.pipelines.is_empty() && self.connections.is_empty() {
            rows.push(Row::Note(
                "Extract-load pipelines, defined as YAML in el/.".into(),
            ));
            rows.push(Row::Initialize);
            return rows;
        }
        rows.push(Row::Header("Pipelines".into()));
        for path in &self.pipelines {
            rows.push(Row::Pipeline(path.clone()));
        }
        rows.push(Row::NewPipeline);
        if let Some(error) = &self.connections_error {
            rows.push(Row::Header("Connections".into()));
            rows.push(Row::ConnNote(error.clone(), Color::Error));
            return rows;
        }
        {
            rows.push(Row::Header("Connections".into()));
            for (name, kind) in &self.connections {
                rows.push(Row::Connection(name.clone(), kind.clone()));
                if !self.expanded.contains(name) {
                    continue;
                }
                match self.tables.get(name) {
                    None | Some(TablesState::Loading) => {
                        rows.push(Row::ConnNote("Loading tables…".into(), Color::Muted));
                    }
                    Some(TablesState::Failed(message)) => {
                        rows.push(Row::ConnNote(message.clone(), Color::Error));
                    }
                    Some(TablesState::Loaded(tables)) if tables.is_empty() => {
                        rows.push(Row::ConnNote("No tables.".into(), Color::Muted));
                    }
                    Some(TablesState::Loaded(tables)) => {
                        for (schema, table) in tables {
                            rows.push(Row::Table {
                                connection: name.clone(),
                                schema: schema.clone(),
                                table: table.clone(),
                            });
                        }
                    }
                }
            }
            rows.push(Row::AddConnection);
        }
        rows
    }

    fn toggle_connection(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if self.expanded.contains(&name) {
            self.expanded.remove(&name);
        } else {
            self.expanded.insert(name.clone());
            if !self.tables.contains_key(&name) {
                self.load_tables(name, cx);
            }
        }
        cx.notify();
    }

    fn load_tables(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.tables.insert(name.clone(), TablesState::Loading);
        let connection_name = name.to_string();
        let task = cx.background_spawn(async move {
            let worker = super::find_worker().ok_or_else(|| {
                anyhow::anyhow!(
                    "Connector worker not found — build zdbt-el-worker or set ZDBT_EL_WORKER."
                )
            })?;
            let (connections, _) = el_engine::spec::load_active_connections(&root)?;
            let connection = connections
                .connections
                .get(&connection_name)
                .ok_or_else(|| anyhow::anyhow!("connection is gone from connections.yml"))?;
            let env = el_engine::env::EnvMap::load(&root, None);
            el_engine::explore::list_tables(&worker, &root, connection, &env)
        });
        let key = name.clone();
        self._list_tasks.insert(
            key.clone(),
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    let state = match result {
                        Ok(tables) => TablesState::Loaded(tables),
                        Err(error) => TablesState::Failed(format!("{error:#}").into()),
                    };
                    this.tables.insert(name.clone(), state);
                    cx.notify();
                })
                .ok();
            }),
        );
    }

    /// Called by the connection modal after a successful write: drops
    /// stale table caches (names may have changed) and re-reads the spec.
    pub fn connections_changed(&mut self, cx: &mut Context<Self>) {
        self.tables.clear();
        self.expanded.clear();
        self.refresh(cx);
    }

    fn edit_connection(
        &mut self,
        editing: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else { return };
        let panel = cx.entity().downgrade();
        self.workspace
            .update(cx, |workspace, cx| {
                super::connection_modal::ElConnectionModal::deploy(
                    workspace,
                    panel,
                    root,
                    editing.map(|name| name.to_string()),
                    window,
                    cx,
                );
            })
            .ok();
    }

    fn query_table(
        &mut self,
        connection: SharedString,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| {
                let Some(panel) = workspace.panel::<super::ElRunsPanel>(cx) else {
                    return;
                };
                workspace.focus_panel::<super::ElRunsPanel>(window, cx);
                panel.update(cx, |panel, cx| {
                    panel.show_query_for_table(connection, &schema, &table, window, cx);
                });
            })
            .ok();
    }

    fn open_pipeline(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.workspace
            .update(cx, |workspace, cx| {
                super::ElPipelineCanvas::deploy(workspace, root, path, window, cx);
            })
            .ok();
    }

    fn new_pipeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let dir = super::el_dir(&root).join("pipelines");
        let _ = std::fs::create_dir_all(&dir);
        // First free pipeline_<n> name.
        let mut index = 1;
        let path = loop {
            let candidate = dir.join(format!("pipeline_{index}.yml"));
            if !candidate.exists() {
                break candidate;
            }
            index += 1;
        };
        // Seed source/target from the project's real connections so the
        // starter validates immediately.
        let source = self
            .connections
            .iter()
            .find(|(_, kind)| !matches!(kind.as_ref(), "snowflake"))
            .or(self.connections.first())
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| "files".to_owned());
        let target = self
            .connections
            .iter()
            .find(|(_, kind)| matches!(kind.as_ref(), "duckdb" | "snowflake"))
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| "warehouse".to_owned());
        let starter = format!(
            "# yaml-language-server: $schema=../.zdbt/el-pipeline.schema.json\n\
             version: 1\npipeline: pipeline_{index}\nsource: {source}\n\
             target:\n  connection: {target}\n  schema: LANDING\n  table: '{{stream}}'\nstreams: []\n"
        );
        if std::fs::write(&path, starter).is_ok() {
            self.refresh(cx);
            self.open_pipeline(path, window, cx);
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| {
                super::initialize_workspace(workspace, window, cx);
            })
            .ok();
        self.refresh(cx);
    }
}

impl EventEmitter<PanelEvent> for ElPanel {}

impl Focusable for ElPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ElPanel {
    fn persistent_name() -> &'static str {
        "EL Panel"
    }

    fn panel_key() -> &'static str {
        "ElPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(280.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ArrowRightLeft)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("EL Pipelines")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElPanelFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |this, _, cx| this.refresh(cx));
        }
    }
}

impl Render for ElPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let rows = std::sync::Arc::new(self.rows());
        let count = rows.len();
        let entity = cx.entity();

        let list = gpui::uniform_list("el-panel-rows", count, {
            move |range, _window, cx| {
                entity.update(cx, |this, cx| {
                    range
                        .filter_map(|ix| rows.get(ix).map(|row| this.render_row(row, ix, cx)))
                        .collect::<Vec<_>>()
                })
            }
        })
        .flex_1()
        .track_scroll(&self.scroll);

        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElPanel")
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new("EL").size(LabelSize::Small))
                    .child(div().flex_1())
                    .children(self.profile.clone().map(|profile| {
                        Label::new(profile)
                            .size(LabelSize::XSmall)
                            .color(Color::Accent)
                    }))
                    .child(
                        IconButton::new("el-panel-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Explicit refresh also invalidates cached
                                // table listings.
                                this.tables.clear();
                                for name in this.expanded.clone() {
                                    this.load_tables(name, cx);
                                }
                                this.refresh(cx);
                            })),
                    ),
            )
            .children((!self.profiles.is_empty()).then(|| {
                // The environment switcher: same pipelines, different
                // connections. One dropdown, the active profile as its
                // face — the panel's single accent.
                let profiles = self.profiles.clone();
                let active = self.profile.clone();
                let panel = cx.entity().downgrade();
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("profile")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        ui::PopoverMenu::new("el-profile-select")
                            .trigger(
                                Button::new(
                                    "el-profile-trigger",
                                    active.clone().unwrap_or_else(|| "choose…".into()),
                                )
                                .label_size(LabelSize::XSmall)
                                .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                                .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                            )
                            .menu(move |window, cx| {
                                let panel = panel.clone();
                                let profiles = profiles.clone();
                                let active = active.clone();
                                Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                                    for name in profiles {
                                        let panel = panel.clone();
                                        let selected = active.as_ref() == Some(&name);
                                        let label = name.clone();
                                        menu = menu.toggleable_entry(
                                            label,
                                            selected,
                                            ui::IconPosition::Start,
                                            None,
                                            move |_, cx| {
                                                panel
                                                    .update(cx, |this, cx| {
                                                        this.switch_profile(name.clone(), cx)
                                                    })
                                                    .ok();
                                            },
                                        );
                                    }
                                    menu
                                }))
                            }),
                    )
            }))
            .child(list)
    }
}

impl ElPanel {
    fn render_row(&self, row: &Row, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let base = h_flex()
            .id(ix)
            .h(px(24.))
            .w_full()
            .px_2()
            .gap_1()
            .items_center()
            .hover(|style| style.bg(cx.theme().colors().element_hover));
        match row {
            Row::Header(title) => base
                .child(
                    Label::new(title.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Row::Note(text) => base
                .child(Label::new(text.clone()).size(LabelSize::XSmall).color(Color::Muted))
                .into_any_element(),
            Row::Pipeline(path) => {
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("pipeline")
                    .to_owned();
                let path = path.clone();
                base.cursor_pointer()
                    .child(
                        Icon::new(IconName::ArrowRightLeft)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name).size(LabelSize::Small).truncate())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_pipeline(path.clone(), window, cx);
                    }))
                    .into_any_element()
            }
            Row::Connection(name, kind) => {
                let can_browse = browsable(kind);
                let expanded = self.expanded.contains(name);
                let chevron = if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                };
                let toggle_name = name.clone();
                base.when(can_browse, |row| {
                    row.cursor_pointer()
                        .child(Icon::new(chevron).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_connection(toggle_name.clone(), cx);
                        }))
                })
                .child(
                    Icon::new(IconName::DatabaseZap)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                .child(div().flex_1())
                .child(
                    Label::new(kind.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child({
                    let name = name.clone();
                    IconButton::new(("el-conn-edit", ix), IconName::Pencil)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Edit connection"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_connection(Some(name.clone()), window, cx);
                        }))
                })
                .into_any_element()
            }
            Row::AddConnection => base
                .cursor_pointer()
                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                .child(
                    Label::new("Add connection")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.edit_connection(None, window, cx)
                }))
                .into_any_element(),
            Row::Table {
                connection,
                schema,
                table,
            } => {
                let connection = connection.clone();
                let schema = schema.clone();
                let table = table.clone();
                let label = format!("{schema}.{table}");
                let dragged = super::DraggedTable {
                    connection: connection.clone(),
                    schema: schema.clone(),
                    table: table.clone(),
                };
                let drag_label: SharedString = label.clone().into();
                base.cursor_pointer()
                    .pl_6()
                    .child(
                        Icon::new(IconName::Table)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(label).size(LabelSize::Small).truncate())
                    // Drag onto a pipeline canvas to add it as a stream.
                    .on_drag(dragged, move |_, _, _, cx| {
                        let drag_label = drag_label.clone();
                        cx.new(|_| DraggedTablePreview(drag_label))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.query_table(
                            connection.clone(),
                            schema.clone(),
                            table.clone(),
                            window,
                            cx,
                        );
                    }))
                    .into_any_element()
            }
            Row::ConnNote(text, color) => base
                .pl_6()
                .child(Label::new(text.clone()).size(LabelSize::XSmall).color(*color))
                .into_any_element(),
            Row::NewPipeline => base
                .cursor_pointer()
                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                .child(
                    Label::new("New pipeline")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_pipeline(window, cx)))
                .into_any_element(),
            Row::Initialize => base
                .cursor_pointer()
                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                .child(
                    Label::new("Initialize EL workspace…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .on_click(cx.listener(|this, _, window, cx| this.initialize(window, cx)))
                .into_any_element(),
        }
    }
}
