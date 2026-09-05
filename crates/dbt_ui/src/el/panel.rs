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
    scroll: UniformListScrollHandle,
    _refresh: Task<()>,
}

enum Row {
    Header(SharedString),
    Pipeline(PathBuf),
    Connection(SharedString, SharedString),
    NewPipeline,
    Initialize,
    Note(SharedString),
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
        self.pipelines = el_engine::spec::list_pipelines(&el);
        self.connections = el_engine::spec::load_connections(&el.join("connections.yml"))
            .map(|connections| {
                connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        (name.clone().into(), connection.kind().to_owned().into())
                    })
                    .collect()
            })
            .unwrap_or_default();
        cx.notify();
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
        if !self.connections.is_empty() {
            rows.push(Row::Header("Connections".into()));
            for (name, kind) in &self.connections {
                rows.push(Row::Connection(name.clone(), kind.clone()));
            }
        }
        rows
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
                    .child(
                        IconButton::new("el-panel-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
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
            Row::Connection(name, kind) => base
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
