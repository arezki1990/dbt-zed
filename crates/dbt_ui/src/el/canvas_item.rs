//! The pipeline canvas: a center-pane Item rendering one `el/pipelines/*.yml`
//! as source → Cast & Map → Snowflake graph. U1 is read-only: pan, zoom,
//! session-local node drags, live refresh when the file changes on disk —
//! the mapping editor and YAML writes arrive next.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, PathBuilder, Pixels,
    Point, SharedString, Task, WeakEntity, Window, canvas, point, px,
};
use project::Project;
use ui::{Tooltip, prelude::*};
use workspace::{
    Workspace,
    item::{Item, TabContentParams},
};

use super::layout::{ElEdge, ElLayout, ElNode, ElNodeKind, build_layout};
use el_engine::spec::{Connections, Pipeline, SpecIssue};

const MIN_ZOOM: f32 = 0.4;
const MAX_ZOOM: f32 = 2.0;

enum CanvasDrag {
    Node(usize, Point<Pixels>),
    Canvas(Point<Pixels>),
}

struct LoadedSpec {
    pipeline: Arc<Pipeline>,
    issues: Vec<SpecIssue>,
    missing_env: Vec<String>,
}

pub struct ElPipelineCanvas {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    project_root: PathBuf,
    spec_path: PathBuf,
    loaded: Option<LoadedSpec>,
    parse_error: Option<SharedString>,
    layout: ElLayout,
    spec_mtime: Option<SystemTime>,
    pan: (f32, f32),
    zoom: f32,
    drag: Option<CanvasDrag>,
    _load: Task<()>,
    _subscription: gpui::Subscription,
}

impl ElPipelineCanvas {
    /// Opens (or refocuses) the canvas for `spec_path` in the active pane.
    pub fn deploy(
        workspace: &mut Workspace,
        project_root: PathBuf,
        spec_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let existing = workspace
            .items_of_type::<ElPipelineCanvas>(cx)
            .find(|item| item.read(cx).spec_path == spec_path);
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let project = workspace.project().clone();
        let canvas = cx.new(|cx| {
            Self::new(workspace_handle, project, project_root, spec_path, cx)
        });
        workspace.add_item_to_active_pane(Box::new(canvas), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        project_root: PathBuf,
        spec_path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        // Any worktree change is a cheap mtime check against one file, so
        // no debounce or path filtering is needed.
        let subscription = cx.subscribe(
            &project,
            |this: &mut Self, _, event: &project::Event, cx| {
                if matches!(event, project::Event::WorktreeUpdatedEntries(..)) {
                    this.reload_if_changed(cx);
                }
            },
        );
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            project_root,
            spec_path,
            loaded: None,
            parse_error: None,
            layout: ElLayout::default(),
            spec_mtime: None,
            pan: (0., 0.),
            zoom: 1.,
            drag: None,
            _load: Task::ready(()),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        let mtime = std::fs::metadata(&self.spec_path)
            .and_then(|meta| meta.modified())
            .ok();
        if mtime != self.spec_mtime {
            self.reload(cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.spec_mtime = std::fs::metadata(&self.spec_path)
            .and_then(|meta| meta.modified())
            .ok();
        let spec_path = self.spec_path.clone();
        let project_root = self.project_root.clone();
        let task = cx.background_spawn(async move { load_spec(&project_root, &spec_path) });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(loaded) => {
                        this.layout = build_layout(
                            &loaded.pipeline,
                            loaded_connections(&this.project_root).as_ref(),
                        );
                        this.loaded = Some(loaded);
                        this.parse_error = None;
                    }
                    Err(error) => {
                        this.parse_error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn set_zoom(&mut self, new_zoom: f32, cx: &mut Context<Self>) {
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - self.zoom).abs() > f32::EPSILON {
            // Anchor on the layout center so the graph doesn't slide away.
            let (cx_pt, cy_pt) = (self.layout.width / 2., self.layout.height / 2.);
            self.pan.0 += cx_pt * (self.zoom - new_zoom);
            self.pan.1 += cy_pt * (self.zoom - new_zoom);
            self.zoom = new_zoom;
            cx.notify();
        }
    }

    fn open_yaml(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.spec_path.clone();
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

    fn pipeline_name(&self) -> SharedString {
        self.loaded
            .as_ref()
            .map(|loaded| loaded.pipeline.pipeline.clone().into())
            .unwrap_or_else(|| {
                self.spec_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("pipeline")
                    .to_owned()
                    .into()
            })
    }

    fn render_node(&self, ix: usize, node: &ElNode, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let zoom = self.zoom;
        let accent = match node.kind {
            ElNodeKind::Stream { .. } => cx.theme().status().info,
            ElNodeKind::Cast => cx.theme().status().warning,
            ElNodeKind::Target => cx.theme().status().success,
        };
        let icon = match node.kind {
            ElNodeKind::Stream { .. } => IconName::FileCode,
            ElNodeKind::Cast => IconName::ArrowRightLeft,
            ElNodeKind::Target => IconName::DatabaseZap,
        };
        let x = node.x * zoom + self.pan.0;
        let y = node.y * zoom + self.pan.1;

        v_flex()
            .id(ix)
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(node.width * zoom))
            .h(px(node.height * zoom))
            .px(px(10. * zoom))
            .py(px(6. * zoom))
            .justify_center()
            .gap(px(2. * zoom))
            .rounded_md()
            .border_1()
            .border_color(accent.opacity(0.7))
            .bg(colors.elevated_surface_background)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.drag = Some(CanvasDrag::Node(ix, event.position));
                    cx.notify();
                }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                    .child(
                        div().min_w_0().flex_1().child(
                            Label::new(node.label.clone())
                                .size(LabelSize::Small)
                                .truncate(),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(10. * zoom))
                    .text_color(colors.text_muted)
                    .truncate()
                    .child(node.sublabel.clone()),
            )
            .into_any_element()
    }

    fn render_edges(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let nodes = self.layout.nodes.clone();
        let edges = self.layout.edges.clone();
        let zoom = self.zoom;
        let pan = self.pan;
        let stroke = cx.theme().colors().text_muted.opacity(0.55);

        canvas(
            |_, _, _| {},
            move |_bounds, _, window, _| {
                let place = |node: &ElNode| -> (Point<Pixels>, Point<Pixels>) {
                    let left = point(
                        px(node.x * zoom + pan.0),
                        px((node.y + node.height / 2.) * zoom + pan.1),
                    );
                    let right = point(
                        px((node.x + node.width) * zoom + pan.0),
                        px((node.y + node.height / 2.) * zoom + pan.1),
                    );
                    (left, right)
                };
                for edge in &edges {
                    let (Some(from), Some(to)) = (nodes.get(edge.from), nodes.get(edge.to))
                    else {
                        continue;
                    };
                    let (_, start) = place(from);
                    let (end, _) = place(to);
                    let mid_x = (start.x + end.x) / 2.;
                    let mut path = PathBuilder::stroke(px(1.5 * zoom));
                    path.move_to(start);
                    path.curve_to(point(mid_x, start.y), point(mid_x, start.y));
                    path.curve_to(end, point(mid_x, end.y));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, stroke);
                    }
                    let _ = &edge.stream_ix;
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .into_any_element()
    }

    fn render_canvas(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let mut surface = div()
            .id("el-canvas")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(colors.editor_background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.drag = Some(CanvasDrag::Canvas(event.position));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if !event.dragging() {
                    this.drag = None;
                    return;
                }
                match &mut this.drag {
                    Some(CanvasDrag::Canvas(last)) => {
                        this.pan.0 += f32::from(event.position.x - last.x);
                        this.pan.1 += f32::from(event.position.y - last.y);
                        *last = event.position;
                        cx.notify();
                    }
                    Some(CanvasDrag::Node(ix, last)) => {
                        let ix = *ix;
                        let (dx, dy) = (
                            f32::from(event.position.x - last.x) / this.zoom,
                            f32::from(event.position.y - last.y) / this.zoom,
                        );
                        if let Some(node) = this.layout.nodes.get_mut(ix) {
                            node.x += dx;
                            node.y += dy;
                        }
                        if let Some(CanvasDrag::Node(_, last)) = &mut this.drag {
                            *last = event.position;
                        }
                        cx.notify();
                    }
                    None => {}
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                if event.modifiers.platform {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 120.,
                        gpui::ScrollDelta::Lines(delta) => delta.y / 8.,
                    };
                    this.set_zoom(this.zoom * (1. + delta), cx);
                } else {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                        gpui::ScrollDelta::Lines(delta) => (delta.x * 20., delta.y * 20.),
                    };
                    this.pan.0 += delta.0;
                    this.pan.1 += delta.1;
                    cx.notify();
                }
            }));

        surface = surface.child(self.render_edges(cx));
        let nodes = self.layout.nodes.clone();
        for (ix, node) in nodes.iter().enumerate() {
            surface = surface.child(self.render_node(ix, node, cx));
        }
        surface.into_any_element()
    }
}

fn load_spec(project_root: &std::path::Path, spec_path: &std::path::Path) -> Result<LoadedSpec> {
    let pipeline = el_engine::spec::load_pipeline(spec_path)?;
    let connections = loaded_connections(project_root);
    let issues = connections
        .as_ref()
        .map(|connections| el_engine::spec::validate(&pipeline, connections))
        .unwrap_or_default();
    // Missing env references, checked against real env + project dotenv —
    // names only, values never surface.
    let env = el_engine::env::EnvMap::load(project_root, None);
    let mut missing_env = Vec::new();
    if let Some(connections) = &connections {
        for name in [&pipeline.source, &pipeline.target.connection] {
            if let Some(connection) = connections.connections.get(name) {
                for var in connection.env_refs() {
                    if !env.contains(&var) && !missing_env.contains(&var) {
                        missing_env.push(var);
                    }
                }
            }
        }
    }
    Ok(LoadedSpec {
        pipeline: Arc::new(pipeline),
        issues: issues
            .into_iter()
            .filter(|issue| !issue.message.contains("references ${"))
            .collect(),
        missing_env,
    })
}

fn loaded_connections(project_root: &std::path::Path) -> Option<Connections> {
    el_engine::spec::load_connections(&super::el_dir(project_root).join("connections.yml")).ok()
}

impl EventEmitter<()> for ElPipelineCanvas {}

impl Focusable for ElPipelineCanvas {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ElPipelineCanvas {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("Pipeline: {}", self.pipeline_name()).into()
    }

    fn tab_content(&self, params: TabContentParams, _window: &Window, _cx: &App) -> gpui::AnyElement {
        h_flex()
            .gap_1()
            .child(Icon::new(IconName::ArrowRightLeft).color(Color::Muted))
            .child(Label::new(self.tab_content_text(0, _cx)).color(params.text_color()))
            .into_any_element()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(self.spec_path.display().to_string().into())
    }
}

impl Render for ElPipelineCanvas {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();

        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                Label::new(self.pipeline_name())
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("el-zoom-out", IconName::Dash)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Zoom out"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom / 1.2, cx))),
            )
            .child(
                Label::new(format!("{:.0}%", self.zoom * 100.))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                IconButton::new("el-zoom-in", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Zoom in"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom * 1.2, cx))),
            )
            .child(
                IconButton::new("el-open-yaml", IconName::FileCode)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Open YAML"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_yaml(window, cx))),
            )
            .child(
                IconButton::new("el-refresh", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Reload from file"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );

        let mut body = v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElPipelineCanvas")
            .bg(colors.editor_background)
            .child(toolbar);

        if let Some(loaded) = &self.loaded {
            if !loaded.missing_env.is_empty() {
                body = body.child(
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().status().warning_background)
                        .child(
                            Label::new(format!(
                                "Missing environment variables: {}",
                                loaded.missing_env.join(", ")
                            ))
                            .size(LabelSize::XSmall)
                            .color(Color::Warning),
                        ),
                );
            }
            for issue in &loaded.issues {
                let prefix = issue
                    .stream
                    .as_ref()
                    .map(|stream| format!("{stream}: "))
                    .unwrap_or_default();
                body = body.child(
                    div().w_full().px_2().py_0p5().child(
                        Label::new(format!("⚠ {prefix}{}", issue.message))
                            .size(LabelSize::XSmall)
                            .color(Color::Warning),
                    ),
                );
            }
        }

        if let Some(error) = &self.parse_error {
            body.child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Label::new(error.clone())
                            .size(LabelSize::Small)
                            .color(Color::Error),
                    )
                    .child(
                        Button::new("el-open-broken-yaml", "Open YAML")
                            .on_click(cx.listener(|this, _, window, cx| this.open_yaml(window, cx))),
                    ),
            )
        } else if self.loaded.is_none() {
            body.child(
                v_flex().flex_1().items_center().justify_center().child(
                    Label::new("Loading pipeline…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
        } else {
            body.child(self.render_canvas(cx))
        }
    }
}
