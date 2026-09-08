//! The EL panel: the extract-load plugin's own left-dock surface —
//! pipelines explorer, connections at a glance (names and kinds only,
//! never values), New pipeline, Initialize. Fully independent of the dbt
//! panels; works in standalone EL projects with no dbt_project.yml.

use std::path::PathBuf;

use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use ui::{Tooltip, WithScrollbar as _, prelude::*};
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
    /// Declared remotes: (name, host shown muted — never the token).
    remotes: Vec<(SharedString, SharedString)>,
    /// Section keys the user folded ("pipelines", "connections", "remotes").
    collapsed: std::collections::HashSet<&'static str>,
    /// Connections whose table list is unfolded in the explorer.
    expanded: std::collections::HashSet<SharedString>,
    tables: std::collections::HashMap<SharedString, TablesState>,
    /// Bumped on profile/connection changes: in-flight table listings
    /// from the previous environment are dropped, not displayed.
    tables_epoch: u64,
    _list_tasks: std::collections::HashMap<SharedString, Task<()>>,
    scroll: UniformListScrollHandle,
    scroll_lower: UniformListScrollHandle,
    scroll_remotes: UniformListScrollHandle,
    /// Heights of the Pipelines and Connections sections (Remotes takes
    /// the rest); the two splitters drag them.
    split: f32,
    split_connections: f32,
    /// (which splitter, pointer y at drag start, split at drag start).
    split_drag: Option<(usize, f32, f32)>,
    _refresh: Task<()>,
}

enum TablesState {
    Loading,
    Loaded(Vec<(String, String)>),
    Failed(SharedString),
}

enum Row {
    Header(SharedString),
    /// Section headers: collapse chevron, label, and the "+" action.
    PipelinesHeader,
    ConnectionsHeader,
    RemotesHeader,
    Remote(SharedString, SharedString),
    Pipeline(PathBuf),
    Connection(SharedString, SharedString),
    Table {
        connection: SharedString,
        schema: String,
        table: String,
    },
    ConnNote(SharedString, Color),
    Initialize,
    Note(SharedString),
}

/// Kinds the explorer can browse — the worker's list/query support.
fn browsable(kind: &str) -> bool {
    matches!(kind, "duckdb" | "postgres" | "oracle")
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
            remotes: Vec::new(),
            collapsed: Default::default(),
            expanded: Default::default(),
            tables: Default::default(),
            tables_epoch: 0,
            _list_tasks: Default::default(),
            scroll: UniformListScrollHandle::new(),
            scroll_lower: UniformListScrollHandle::new(),
            scroll_remotes: UniformListScrollHandle::new(),
            split: 200.,
            split_connections: 220.,
            split_drag: None,
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
        self.remotes = el_engine::spec::load_remotes(&el.join("remotes.yml"))
            .map(|remotes| {
                remotes
                    .remotes
                    .iter()
                    .map(|(name, remote)| {
                        // Host only: scheme stripped, path/query dropped.
                        let host = remote
                            .url
                            .split_once("://")
                            .map(|(_, rest)| rest)
                            .unwrap_or(&remote.url)
                            .split(['/', '?'])
                            .next()
                            .unwrap_or("")
                            .to_owned();
                        (name.clone().into(), host.into())
                    })
                    .collect()
            })
            .unwrap_or_default();
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
        self.tables_epoch += 1;
        self.refresh(cx);
        // The selection file is one voice among three — say what actually
        // happened instead of assuming the write took effect.
        let effective = self.profile.clone();
        let message = if effective.as_ref() == Some(&name) {
            format!("Profile: {name}")
        } else if std::env::var("ZDBT_EL_PROFILE").is_ok() {
            format!(
                "Saved, but ZDBT_EL_PROFILE overrides the selection — this window                  still runs as {}.",
                effective.as_deref().unwrap_or("the base connections")
            )
        } else {
            format!(
                "Saved, but the active profile is {} — check connections.yml.",
                effective.as_deref().unwrap_or("none")
            )
        };
        let console = self.console(cx);
        self.workspace
            .update(cx, |workspace, cx| {
                let canvases: Vec<_> =
                    workspace.items_of_type::<super::ElPipelineCanvas>(cx).collect();
                for canvas in canvases {
                    canvas.update(cx, |canvas, cx| canvas.reload(cx));
                }
                if effective.as_ref() == Some(&name) {
                    super::toast(workspace, &message, cx);
                } else {
                    super::toast_error(workspace, &message, None, cx);
                }
            })
            .ok();
        // The EL console's Query state is from the old environment — reset
        // it, outside the workspace lease (it reads the workspace).
        if let Some(console) = console {
            console.update(cx, |panel, cx| panel.profile_changed(cx));
        }
    }

    /// True when the project has nothing EL yet — one list with the
    /// invitation to initialize.
    fn is_empty_project(&self) -> bool {
        self.pipelines.is_empty() && self.connections.is_empty() && self.remotes.is_empty()
    }

    /// The upper list: pipelines.
    fn upper_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.is_empty_project() {
            rows.push(Row::Note(
                "Extract-load pipelines, defined as YAML in el/.".into(),
            ));
            rows.push(Row::Initialize);
            return rows;
        }
        rows.push(Row::PipelinesHeader);
        if !self.collapsed.contains("pipelines") {
            for path in &self.pipelines {
                rows.push(Row::Pipeline(path.clone()));
            }
        }
        rows
    }

    /// The middle list: connections with their explorer trees.
    fn connection_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(error) = &self.connections_error {
            rows.push(Row::ConnectionsHeader);
            if !self.collapsed.contains("connections") {
                rows.push(Row::ConnNote(error.clone(), Color::Error));
            }
            return rows;
        }
        if !self.collapsed.contains("connections") {
            rows.push(Row::ConnectionsHeader);
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
        } else {
            rows.push(Row::ConnectionsHeader);
        }
        rows
    }

    /// The bottom list: declared servers.
    fn remote_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        rows.push(Row::RemotesHeader);
        if self.collapsed.contains("remotes") {
            return rows;
        }
        if self.remotes.is_empty() {
            rows.push(Row::ConnNote(
                "No servers yet — press + to add one.".into(),
                Color::Muted,
            ));
        }
        for (name, host) in &self.remotes {
            rows.push(Row::Remote(name.clone(), host.clone()));
        }
        rows
    }

    fn toggle_section(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key);
        }
        cx.notify();
    }

    /// Opens the console's Remote tab on the named server.
    fn show_remote(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // Two separate leases: the console may read the workspace while it
        // refreshes, so it must not be updated inside workspace.update.
        let Some(console) = self.console(cx) else { return };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.focus_panel::<super::ElRunsPanel>(window, cx);
            })
            .ok();
        console.update(cx, |panel, cx| panel.show_remote(name, cx));
    }

    /// The EL console panel, read without holding a workspace lease.
    fn console(&self, cx: &App) -> Option<Entity<super::ElRunsPanel>> {
        self.workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).panel::<super::ElRunsPanel>(cx))
    }

    pub fn remotes_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    fn edit_remote(
        &mut self,
        editing: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else { return };
        let panel = cx.entity().downgrade();
        self.workspace
            .update(cx, |workspace, cx| {
                super::remote_modal::ElRemoteModal::deploy(
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

    /// Kept for the YAML route: open (or scaffold) remotes.yml.
    #[allow(dead_code)]
    fn add_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let path = super::el_dir(&root).join("remotes.yml");
        if !path.exists() {
            let starter = "# el/remotes.yml — el serve daemons the IDE can drive from the \
Remote tab.\n# Tokens are ${VAR} references, never literals. Non-loopback URLs must be \
https.\nversion: 1\nremotes:\n  # local_daemon:\n  #   url: http://127.0.0.1:7431\n  #   \
token: \"${ZDBT_EL_TOKEN}\"\n";
            if let Err(error) = std::fs::write(&path, starter) {
                self.workspace
                    .update(cx, |workspace, cx| {
                        super::toast_error(
                            workspace,
                            &format!("could not create remotes.yml: {error}"),
                            None,
                            cx,
                        )
                    })
                    .ok();
                return;
            }
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                    .detach();
            })
            .ok();
    }

    fn toggle_connection(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if self.expanded.contains(&name) {
            self.expanded.remove(&name);
        } else {
            self.expanded.insert(name.clone());
            if !self.tables.contains_key(&name) {
                self.load_tables(name.clone(), cx);
            }
        }
        // Touching a connection here is how the Query tab picks its target.
        if let Some(console) = self.console(cx) {
            console.update(cx, |panel, cx| panel.select_connection(name.clone(), cx));
        }
        cx.notify();
    }

    fn load_tables(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.tables.insert(name.clone(), TablesState::Loading);
        let epoch = self.tables_epoch;
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
                    if this.tables_epoch != epoch {
                        return; // a different environment answered
                    }
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
        self.tables_epoch += 1;
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
        let Some(console) = self.console(cx) else { return };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.focus_panel::<super::ElRunsPanel>(window, cx);
            })
            .ok();
        console.update(cx, |panel, cx| {
            panel.show_query_for_table(connection, &schema, &table, window, cx);
        });
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
            .find(|(_, kind)| matches!(kind.as_ref(), "duckdb" | "snowflake" | "oracle"))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let make_list = |id: &'static str,
                         rows: Vec<Row>,
                         scroll: &UniformListScrollHandle,
                         cx: &mut Context<Self>| {
            let rows = std::sync::Arc::new(rows);
            let count = rows.len();
            let entity = cx.entity();
            gpui::uniform_list(id, count, {
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
            .track_scroll(scroll)
        };
        let empty = self.is_empty_project();
        let upper = make_list("el-panel-upper", self.upper_rows(), &self.scroll, cx);
        let middle = make_list("el-panel-connections", self.connection_rows(), &self.scroll_lower, cx);
        let bottom = make_list("el-panel-remotes", self.remote_rows(), &self.scroll_remotes, cx);
        let dragging = self.split_drag.is_some();
        const HEADER_ONLY: f32 = 26.;
        // A 1px line with a 5px grab area — drag to trade space.
        let splitter = |which: usize, dragging: bool, cx: &mut Context<Self>| {
            div()
                .id(("el-panel-split", which))
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
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let start = if which == 0 { this.split } else { this.split_connections };
                        this.split_drag = Some((which, f32::from(event.position.y), start));
                        cx.notify();
                    }),
                )
        };
        let pipelines_h = if self.collapsed.contains("pipelines") { HEADER_ONLY } else { self.split };
        let connections_h = if self.collapsed.contains("connections") {
            HEADER_ONLY
        } else {
            self.split_connections
        };
        let list: gpui::AnyElement = if empty {
            upper.flex_1().into_any_element()
        } else {
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .h(px(pipelines_h))
                        .flex_shrink_0()
                        .child(upper.size_full())
                        .vertical_scrollbar_for(&self.scroll, window, cx),
                )
                .child(splitter(0, dragging, cx))
                .child(
                    div()
                        .h(px(connections_h))
                        .flex_shrink_0()
                        .child(middle.size_full())
                        .vertical_scrollbar_for(&self.scroll_lower, window, cx),
                )
                .child(splitter(1, dragging, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(bottom.size_full())
                        .vertical_scrollbar_for(&self.scroll_remotes, window, cx),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElPanel")
            .bg(colors.panel_background)
            .when(dragging, |flex| {
                flex.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((which, start_y, start_split)) = this.split_drag {
                        let next =
                            (start_split + f32::from(event.position.y) - start_y).clamp(60., 900.);
                        if which == 0 {
                            this.split = next;
                        } else {
                            this.split_connections = next;
                        }
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.split_drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.split_drag = None;
                        cx.notify();
                    }),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new("EL").size(LabelSize::Default))
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
    /// A section header: chevron + label toggle the fold; callers append
    /// the section's "+" action.
    fn render_section_header(
        &self,
        base: gpui::Stateful<gpui::Div>,
        key: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let folded = self.collapsed.contains(key);
        let colors = cx.theme().colors();
        base.cursor_pointer()
            .bg(colors.element_background)
            .border_b_1()
            .border_color(colors.border)
            .child(
                Icon::new(if folded {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(IconSize::XSmall)
                .color(Color::Muted),
            )
            .child(Label::new(label).size(LabelSize::Default).color(Color::Default))
            .child(div().flex_1())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(key, cx)))
    }

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
            Row::PipelinesHeader => self
                .render_section_header(base, "pipelines", "Pipelines", cx)
                .child(
                    IconButton::new("el-new-pipeline", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("New pipeline"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.new_pipeline(window, cx)
                        })),
                )
                .into_any_element(),
            Row::ConnectionsHeader => self
                .render_section_header(base, "connections", "Connections", cx)
                .child(
                    IconButton::new("el-add-connection", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Add connection"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_connection(None, window, cx)
                        })),
                )
                .into_any_element(),
            Row::RemotesHeader => self
                .render_section_header(base, "remotes", "Remotes", cx)
                .child(
                    IconButton::new("el-add-remote", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Add server"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_remote(None, window, cx)
                        })),
                )
                .into_any_element(),
            Row::Remote(name, host) => {
                let open_name = name.clone();
                base.cursor_pointer()
                    .child(
                        Icon::new(IconName::Server)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                    .child(div().flex_1())
                    .child(
                        Label::new(host.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .child({
                        let name = name.clone();
                        IconButton::new(("el-remote-edit", ix), IconName::Pencil)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Edit server"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.edit_remote(Some(name.clone()), window, cx);
                            }))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_remote(open_name.clone(), window, cx);
                    }))
                    .into_any_element()
            }
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
