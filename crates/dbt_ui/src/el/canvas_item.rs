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

use super::layout::{ElLayout, ElNode, ElNodeKind, build_layout};
use super::mapping_editor::{MappingEditorState, SNOWFLAKE_TYPES};
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
    drag_moved: bool,
    project: Entity<Project>,
    mapping: Option<MappingEditorState>,
    type_menu: Option<(Entity<ui::ContextMenu>, Point<Pixels>, gpui::Subscription)>,
    _load: Task<()>,
    _probe: Task<()>,
    _write: Task<()>,
    _preview: Task<()>,
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
            drag_moved: false,
            project,
            mapping: None,
            type_menu: None,
            _load: Task::ready(()),
            _probe: Task::ready(()),
            _write: Task::ready(()),
            _preview: Task::ready(()),
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
                    this.drag_moved = false;
                    cx.notify();
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.drag_moved {
                    return;
                }
                let stream_ix = match this.layout.nodes.get(ix).map(|node| &node.kind) {
                    Some(ElNodeKind::Stream { stream_ix }) => Some(*stream_ix),
                    Some(ElNodeKind::Cast) => Some(
                        this.mapping.as_ref().map(|state| state.stream_ix).unwrap_or(0),
                    ),
                    _ => None,
                };
                if let Some(stream_ix) = stream_ix {
                    this.open_mapping(stream_ix, window, cx);
                }
            }))
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
                if this.drag.is_some() {
                    this.drag_moved = true;
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
        // Edge midpoint hotspots: click a stream wire to edit its mapping.
        let edges = self.layout.edges.clone();
        for edge in &edges {
            let Some(stream_ix) = edge.stream_ix else { continue };
            let (Some(from), Some(to)) = (nodes.get(edge.from), nodes.get(edge.to)) else {
                continue;
            };
            let mid_x = ((from.x + from.width) + to.x) / 2. * self.zoom + self.pan.0 - 11.;
            let mid_y = ((from.y + from.height / 2.) + (to.y + to.height / 2.)) / 2. * self.zoom
                + self.pan.1
                - 11.;
            surface = surface.child(
                div()
                    .id(("el-hotspot", stream_ix))
                    .absolute()
                    .left(px(mid_x))
                    .top(px(mid_y))
                    .size(px(22.))
                    .rounded_full()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().elevated_surface_background)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.border_color(cx.theme().colors().border_focused))
                    .child(
                        Icon::new(IconName::Filter)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_mapping(stream_ix, window, cx);
                    })),
            );
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

impl ElPipelineCanvas {
    fn toast(&self, message: String, cx: &mut Context<Self>) {
        struct ElCanvasToast;
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(
                    workspace::Toast::new(
                        workspace::notifications::NotificationId::unique::<ElCanvasToast>(),
                        message,
                    ),
                    cx,
                );
            })
            .ok();
    }

    fn open_mapping(&mut self, stream_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else { return };
        if stream_ix >= loaded.pipeline.streams.len() {
            return;
        }
        let pipeline = loaded.pipeline.clone();
        self.mapping = Some(MappingEditorState::open(&pipeline, stream_ix, window, cx));
        self.kick_probe(pipeline, stream_ix, window, cx);
        cx.notify();
    }

    fn close_mapping(&mut self, cx: &mut Context<Self>) {
        self.mapping = None;
        self.type_menu = None;
        cx.notify();
    }

    /// Background schema probe: fills inferred dtypes and unspecced
    /// columns via the same preview path a run uses.
    fn kick_probe(
        &mut self,
        pipeline: Arc<Pipeline>,
        stream_ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_root = self.project_root.clone();
        let stream_name = pipeline.streams[stream_ix].name.clone();
        let excluded: Vec<String> = pipeline.streams[stream_ix]
            .select
            .as_ref()
            .map(|select| select.exclude.clone())
            .unwrap_or_default();
        let worker = super::find_worker();
        let task = cx.background_spawn(async move {
            el_engine::preview_stream(
                &project_root,
                &pipeline,
                &stream_name,
                30,
                worker.as_deref(),
                &el_engine::CancelFlag::default(),
            )
        });
        self._probe = cx.spawn_in(_window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(state) = &mut this.mapping else { return };
                if state.stream_ix != stream_ix {
                    return;
                }
                match result {
                    Ok(preview) => state.absorb_probe(&preview.columns, &excluded, window, cx),
                    Err(error) => {
                        state.probing = false;
                        state.probe_error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn apply_mapping(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.mapping else { return };
        let Some(loaded) = &self.loaded else { return };
        let mut pipeline = (*loaded.pipeline).clone();
        match state.apply(&mut pipeline, cx) {
            Ok(_) => {}
            Err(error) => {
                self.toast(format!("Apply failed: {error:#}"), cx);
                return;
            }
        }
        if let Some(state) = &mut self.mapping {
            state.dirty = false;
        }
        let workspace = self.workspace.clone();
        let project = self.project.clone();
        let spec_path = self.spec_path.clone();
        self._write = cx.spawn_in(_window, async move |this, cx| {
            let result =
                super::spec_io::write_spec(workspace, project, spec_path, pipeline, cx).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.reload(cx),
                    Err(error) => this.toast(format!("Write failed: {error:#}"), cx),
                }
            })
            .ok();
        });
    }

    /// Runs the bounded preview and shows rows (or failed casts) in the
    /// results panel's grid.
    fn preview_to_grid(&mut self, failures_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.mapping else { return };
        let Some(loaded) = &self.loaded else { return };
        let pipeline = loaded.pipeline.clone();
        let stream_ix = state.stream_ix;
        let stream_name = pipeline.streams[stream_ix].name.clone();
        let project_root = self.project_root.clone();
        let worker = super::find_worker();
        let title: SharedString = if failures_only {
            format!("{stream_name} · failed casts").into()
        } else {
            format!("{stream_name} · preview").into()
        };
        let task = cx.background_spawn(async move {
            el_engine::preview_stream(
                &project_root,
                &pipeline,
                &stream_name,
                200,
                worker.as_deref(),
                &el_engine::CancelFlag::default(),
            )
        });
        let workspace = self.workspace.clone();
        self._preview = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            match result {
                Ok(preview) => {
                    let (columns, rows): (Vec<SharedString>, Vec<Vec<SharedString>>) =
                        if failures_only {
                            (
                                vec!["column".into(), "failed".into(), "sample values".into()],
                                preview
                                    .failures
                                    .iter()
                                    .map(|failure| {
                                        vec![
                                            failure.column.clone().into(),
                                            failure.count.to_string().into(),
                                            failure.samples.join(" · ").into(),
                                        ]
                                    })
                                    .collect(),
                            )
                        } else {
                            (
                                preview
                                    .columns
                                    .iter()
                                    .map(|column| {
                                        format!("{} ({})", column.name, column.target_type).into()
                                    })
                                    .collect(),
                                preview
                                    .rows
                                    .iter()
                                    .map(|row| {
                                        row.iter().map(|cell| cell.clone().into()).collect()
                                    })
                                    .collect(),
                            )
                        };
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            let Some(panel) =
                                workspace.panel::<crate::results_panel::DbtResultsPanel>(cx)
                            else {
                                return;
                            };
                            workspace
                                .focus_panel::<crate::results_panel::DbtResultsPanel>(window, cx);
                            panel.update(cx, |panel, cx| {
                                panel.show_table(title, columns, rows, window, cx)
                            });
                        })
                        .ok();
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.toast(format!("Preview failed: {error:#}"), cx)
                    })
                    .ok();
                }
            }
        });
    }

    fn deploy_type_menu(
        &mut self,
        draft_ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity().downgrade();
        let menu = ui::ContextMenu::build(window, cx, |mut menu, _, _| {
            menu = menu.entry("inherit source type", None, {
                let entity = entity.clone();
                move |_, cx| {
                    entity
                        .update(cx, |this, cx| this.set_draft_cast(draft_ix, None, cx))
                        .ok();
                }
            });
            for spelling in SNOWFLAKE_TYPES {
                let entity = entity.clone();
                menu = menu.entry(*spelling, None, move |_, cx| {
                    entity
                        .update(cx, |this, cx| {
                            this.set_draft_cast(draft_ix, Some((*spelling).into()), cx)
                        })
                        .ok();
                });
            }
            menu
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe(&menu, |this, _, _: &gpui::DismissEvent, cx| {
            this.type_menu.take();
            cx.notify();
        });
        self.type_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn set_draft_cast(&mut self, draft_ix: usize, cast: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.mapping {
            if let Some(draft) = state.drafts.get_mut(draft_ix) {
                draft.cast = cast;
                state.dirty = true;
            }
        }
        cx.notify();
    }

    fn render_mapping_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let Some(state) = &self.mapping else {
            return div().into_any_element();
        };
        let stream_count = self
            .loaded
            .as_ref()
            .map(|loaded| loaded.pipeline.streams.len())
            .unwrap_or(0);
        let stream_ix = state.stream_ix;

        let mut rows = v_flex().id("el-map-rows").flex_1().min_h_0().overflow_y_scroll().gap_0p5().p_1();
        for (draft_ix, draft) in state.drafts.iter().enumerate() {
            let include = draft.include;
            let strict = draft.strict;
            let row = h_flex()
                .w_full()
                .gap_1()
                .items_center()
                .px_1()
                .py_0p5()
                .rounded_sm()
                .when(!include, |row| row.opacity(0.5))
                .child(
                    IconButton::new(
                        ("el-inc", draft_ix),
                        if include {
                            IconName::Check
                        } else {
                            IconName::Circle
                        },
                    )
                    .icon_size(IconSize::XSmall)
                    .tooltip(ui::Tooltip::text(if include {
                        "Included — click to exclude"
                    } else {
                        "Excluded — click to include"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(state) = &mut this.mapping {
                            if let Some(draft) = state.drafts.get_mut(draft_ix) {
                                draft.include = !draft.include;
                                state.dirty = true;
                            }
                        }
                        cx.notify();
                    })),
                )
                .child(
                    v_flex()
                        .w(px(120.))
                        .flex_shrink_0()
                        .child(Label::new(draft.name.clone()).size(LabelSize::Small).truncate())
                        .child(
                            Label::new(draft.inferred.clone().unwrap_or_else(|| "…".into()))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
                .child(
                    Icon::new(IconName::ArrowRight)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                )
                .child(div().w(px(100.)).flex_shrink_0().child(draft.rename.clone()))
                .child(
                    Button::new(
                        ("el-type", draft_ix),
                        draft.cast.clone().unwrap_or_else(|| "inherit".into()),
                    )
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        let position = match event {
                            gpui::ClickEvent::Mouse(event) => event.up.position,
                            _ => Point::default(),
                        };
                        this.deploy_type_menu(draft_ix, position, window, cx);
                    })),
                )
                .child(
                    IconButton::new(("el-strict", draft_ix), IconName::Warning)
                        .icon_size(IconSize::XSmall)
                        .toggle_state(strict)
                        .tooltip(ui::Tooltip::text(if strict {
                            "Strict: failures stop the stream"
                        } else {
                            "Lax: failures become NULL and are counted"
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(state) = &mut this.mapping {
                                if let Some(draft) = state.drafts.get_mut(draft_ix) {
                                    draft.strict = !draft.strict;
                                    state.dirty = true;
                                }
                            }
                            cx.notify();
                        })),
                );
            rows = rows.child(row);
        }

        v_flex()
            .w(px(420.))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        IconButton::new("el-map-prev", IconName::ChevronLeft)
                            .icon_size(IconSize::Small)
                            .disabled(stream_ix == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let ix = this.mapping.as_ref().map(|s| s.stream_ix).unwrap_or(0);
                                if ix > 0 {
                                    this.open_mapping(ix - 1, window, cx);
                                }
                            })),
                    )
                    .child(
                        Label::new(state.stream_name.clone())
                            .size(LabelSize::Small),
                    )
                    .child(
                        IconButton::new("el-map-next", IconName::ChevronRight)
                            .icon_size(IconSize::Small)
                            .disabled(stream_ix + 1 >= stream_count)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let ix = this.mapping.as_ref().map(|s| s.stream_ix).unwrap_or(0);
                                this.open_mapping(ix + 1, window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-map-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.close_mapping(cx))),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(
                        Label::new("target table")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1().child(state.target_table.clone())),
            )
            .children(state.probe_error.clone().map(|error| {
                div().px_2().py_1().child(
                    Label::new(error).size(LabelSize::XSmall).color(Color::Warning),
                )
            }))
            .child(rows)
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        Button::new("el-map-preview", "Preview")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.preview_to_grid(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("el-map-failed", "Failed casts")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.preview_to_grid(true, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("el-map-apply", "Apply")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_mapping(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
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
            let canvas = self.render_canvas(cx);
            let content = if self.mapping.is_some() {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(canvas))
                    .child(self.render_mapping_sidebar(cx))
                    .into_any_element()
            } else {
                canvas
            };
            body.child(content)
        }
        .on_action(cx.listener(|this, _: &crate::CloseMappingEditor, _, cx| {
            this.close_mapping(cx);
        }))
        .children(self.type_menu.as_ref().map(|(menu, position, _)| {
            gpui::deferred(
                gpui::anchored()
                    .position(*position)
                    .anchor(gpui::Anchor::TopLeft)
                    .child(menu.clone()),
            )
            .with_priority(3)
        }))
    }
}
