//! The lineage tab of an xDI project: the graph around a mapping, a table
//! or a process, drawn as the dbt lineage is — tables and mappings as
//! boxes, what reads and loads what as arrows, columns and the path of one
//! of them — with pan, zoom, drags, back and forward, "+" to go further
//! from a node, and a sidebar telling what the node selected is made of.
//! One click selects a node, a double click recenters on it. A mapping's
//! file opens on request only: they run to hundreds of kilobytes of XML;
//! a metadata file never does.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use el_import::lineage::{Neighbourhood, neighbourhood_with, subgraph};
use el_import::model::{ExprKind, IssueKind};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, PathBuilder, Pixels,
    Point, SharedString, Subscription, WeakEntity, Window, canvas, point, px,
};
use serde_json::Value;
use ui::{Tooltip, prelude::*};
use workspace::{
    Workspace,
    item::{Item, TabContentParams},
};

use crate::lineage::{
    GRAPH_COLUMN_ROW_HEIGHT, GRAPH_MAX_COLUMNS, GRAPH_NODE_HEIGHT, GraphLayoutNode,
};
use crate::results_panel::DbtResultsPanel;

use super::Focus;
use super::layout::{XdiLayout, layout};
use super::store::{Snapshot, XdiStore};

const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;
/// Columns are drawn from this zoom up.
const COLUMNS_MIN_ZOOM: f32 = 0.55;
/// So many nodes drawn at most; "+" goes further.
const MAX_NODES: usize = 150;
const MAX_DEPTH: usize = 6;
const SIDEBAR_WIDTH: f32 = 360.;

enum Drag {
    Node(String, Point<Pixels>),
    Canvas(Point<Pixels>),
}

/// What a canvas keeps between frames: pan, zoom, drags and the column
/// traced. Kept apart from the view, for a canvas the dbt and xDI lineage
/// could share.
pub struct CanvasState {
    pub pan: (f32, f32),
    pub zoom: f32,
    /// How far nodes were dragged, by name, in the layout's units.
    pub offsets: HashMap<String, (f32, f32)>,
    drag: Option<Drag>,
    drag_moved: bool,
    /// The column whose path is lit, lower-cased.
    pub selected_column: Option<String>,
    pub show_columns: bool,
    /// Pan to the center on a coming frame.
    recenter: bool,
    recenter_tries: u8,
    /// The canvas's size, as last painted.
    viewport: Rc<Cell<(f32, f32)>>,
}

impl Default for CanvasState {
    fn default() -> Self {
        CanvasState {
            pan: (0., 0.),
            zoom: 1.,
            offsets: HashMap::new(),
            drag: None,
            drag_moved: false,
            selected_column: None,
            show_columns: false,
            recenter: true,
            recenter_tries: 0,
            viewport: Rc::new(Cell::new((0., 0.))),
        }
    }
}

impl CanvasState {
    fn columns_shown(&self) -> bool {
        self.show_columns && self.zoom >= COLUMNS_MIN_ZOOM
    }

    /// Zooms about the viewport's middle.
    fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let (width, height) = self.viewport.get();
        let (cx, cy) = (width / 2., height / 2.);
        let ratio = zoom / self.zoom;
        self.pan = (
            cx - (cx - self.pan.0) * ratio,
            cy - (cy - self.pan.1) * ratio,
        );
        self.zoom = zoom;
    }

    /// Pans so `node` sits in the middle of the viewport; false while the
    /// canvas has not been painted.
    fn center_on(&mut self, node: &GraphLayoutNode) -> bool {
        let (width, height) = self.viewport.get();
        if width <= 1. || height <= 1. {
            return false;
        }
        let (dx, dy) = self.offsets.get(&node.name).copied().unwrap_or((0., 0.));
        let x = (node.x + dx + node.width / 2.) * self.zoom;
        let y = (node.y + dy + node.height / 2.) * self.zoom;
        self.pan = (width / 2. - x, height / 2. - y);
        true
    }
}

/// What a layout was made from.
#[derive(Clone, Debug, PartialEq)]
struct LayoutKey {
    snapshot: usize,
    focus: Focus,
    depth: usize,
    expanded: BTreeSet<String>,
    columns: bool,
}

pub struct XdiLineageView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    store: Entity<XdiStore>,
    focus: Option<Focus>,
    back: Vec<Focus>,
    forward: Vec<Focus>,
    /// How many mappings up and down from the center.
    depth: usize,
    /// Nodes "+" went further from.
    expanded: BTreeSet<String>,
    canvas: CanvasState,
    /// The node the sidebar tells about.
    selected: Option<String>,
    cached: Option<(LayoutKey, Arc<XdiLayout>, Arc<Neighbourhood>)>,
    _observe: Subscription,
}

impl XdiLineageView {
    /// Opens the project's lineage tab on `focus`, or recenters the open
    /// one.
    pub fn deploy(
        workspace: &mut Workspace,
        store: Entity<XdiStore>,
        focus: Focus,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let existing = workspace
            .items_of_type::<XdiLineageView>(cx)
            .find(|view| view.read(cx).store == store);
        if let Some(existing) = existing {
            existing.update(cx, |view, cx| view.navigate(focus, cx));
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let view = cx.new(|cx| {
            let mut view = Self::new(workspace_handle, store, cx);
            view.navigate(focus, cx);
            view
        });
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    /// Brings the lineage tab forward, opening one if none is.
    pub fn reveal(
        workspace: &mut Workspace,
        store: Entity<XdiStore>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let existing = workspace
            .items_of_type::<XdiLineageView>(cx)
            .find(|view| view.read(cx).store == store);
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let view = cx.new(|cx| Self::new(workspace_handle, store, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        store: Entity<XdiStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            store,
            focus: None,
            back: Vec::new(),
            forward: Vec::new(),
            depth: 2,
            expanded: BTreeSet::new(),
            canvas: CanvasState::default(),
            selected: None,
            cached: None,
            _observe: observe,
        }
    }

    /// Draws the graph around `focus`, remembering where it was.
    pub fn navigate(&mut self, focus: Focus, cx: &mut Context<Self>) {
        if self.focus.as_ref() == Some(&focus) {
            self.canvas.recenter = true;
            cx.notify();
            return;
        }
        if let Some(previous) = self.focus.take() {
            self.back.push(previous);
        }
        self.forward.clear();
        self.show(focus, cx);
    }

    fn show(&mut self, focus: Focus, cx: &mut Context<Self>) {
        self.selected = match &focus {
            Focus::Node(id) => Some(id.clone()),
            Focus::Process(_) => None,
        };
        self.focus = Some(focus);
        self.expanded.clear();
        self.canvas.offsets.clear();
        self.canvas.selected_column = None;
        self.canvas.recenter = true;
        self.canvas.recenter_tries = 0;
        cx.notify();
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(focus) = self.back.pop() {
            if let Some(current) = self.focus.take() {
                self.forward.push(current);
            }
            self.show(focus, cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(focus) = self.forward.pop() {
            if let Some(current) = self.focus.take() {
                self.back.push(current);
            }
            self.show(focus, cx);
        }
    }

    /// The layout drawn, made again only when what it is made from changed.
    fn current_layout(
        &mut self,
        snapshot: &Arc<Snapshot>,
    ) -> Option<(Arc<XdiLayout>, Arc<Neighbourhood>)> {
        let focus = self.focus.clone()?;
        let key = LayoutKey {
            snapshot: Arc::as_ptr(snapshot) as usize,
            focus: focus.clone(),
            depth: self.depth,
            expanded: self.expanded.clone(),
            columns: self.canvas.columns_shown(),
        };
        if let Some((cached, layout, around)) = &self.cached {
            if *cached == key {
                return Some((layout.clone(), around.clone()));
            }
        }
        let around = match &focus {
            Focus::Node(id) => neighbourhood_with(
                &snapshot.index,
                id,
                self.depth,
                self.depth,
                &self.expanded,
                MAX_NODES,
            ),
            Focus::Process(id) => {
                let jobs = snapshot.project.jobs_called_by(id);
                subgraph(&snapshot.index, &jobs, MAX_NODES)
            }
        };
        let around = Arc::new(around);
        let layout = Arc::new(layout(&around, &snapshot.project, key.columns));
        self.cached = Some((key, layout.clone(), around.clone()));
        Some((layout, around))
    }

    fn open_mapping(&mut self, job: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.store.read(cx).snapshot().cloned() else {
            return;
        };
        let Some(job) = snapshot.project.job(job) else {
            return;
        };
        let path = snapshot.project.root.join(&job.path);
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

    fn mouse_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if self.canvas.drag.is_some() && event.pressed_button != Some(MouseButton::Left) {
            self.canvas.drag = None;
            cx.notify();
            return;
        }
        let zoom = self.canvas.zoom;
        match &mut self.canvas.drag {
            None => {}
            Some(Drag::Canvas(last)) => {
                let delta = event.position - *last;
                *last = event.position;
                self.canvas.pan.0 += f32::from(delta.x);
                self.canvas.pan.1 += f32::from(delta.y);
                cx.notify();
            }
            Some(Drag::Node(name, last)) => {
                let delta = event.position - *last;
                *last = event.position;
                if delta.x != px(0.) || delta.y != px(0.) {
                    self.canvas.drag_moved = true;
                }
                // Layout units: a drag follows the pointer at any zoom.
                let offset = self.canvas.offsets.entry(name.clone()).or_insert((0., 0.));
                offset.0 += f32::from(delta.x) / zoom;
                offset.1 += f32::from(delta.y) / zoom;
                cx.notify();
            }
        }
    }

    fn color_for(kind: &str, materialization: &str, cx: &App) -> gpui::Hsla {
        let index = match (kind, materialization) {
            ("job", _) => 0,
            (_, "oracle") => 1,
            (_, "snowflake") => 2,
            (_, "sql server") => 3,
            (_, "postgresql") => 4,
            (_, "mysql") => 5,
            (_, "file") => 6,
            (_, "xml" | "json" | "http") => 7,
            _ => 8,
        };
        cx.theme().accents().color_for_index(index)
    }

    /// A mapping's badges: joins, stages, filters, tokens, a copy.
    fn badges(node: &GraphLayoutNode) -> String {
        let Some(detail) = &node.details else {
            return String::new();
        };
        let count = |key: &str| detail.get(key).and_then(Value::as_u64).unwrap_or(0);
        let mut badges = Vec::new();
        for (key, mark) in [
            ("joins", "⋈"),
            ("stages", "∪"),
            ("filters", "σ"),
            ("tokens", "$"),
        ] {
            let value = count(key);
            if value == 1 {
                badges.push(mark.to_owned());
            } else if value > 1 {
                badges.push(format!("{mark}{value}"));
            }
        }
        if detail.get("copy_like") == Some(&Value::Bool(true)) {
            badges.push("copy".to_owned());
        }
        badges.join(" ")
    }

    fn render_toolbar(&self, title: SharedString, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let open_job = self
            .selected
            .clone()
            .or_else(|| match &self.focus {
                Some(Focus::Node(id)) => Some(id.clone()),
                _ => None,
            })
            .filter(|id| id.starts_with("job:"));
        let process = matches!(self.focus, Some(Focus::Process(_)));
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(colors.border)
            .child(
                IconButton::new("xdi-back", IconName::ArrowLeft)
                    .icon_size(IconSize::Small)
                    .disabled(self.back.is_empty())
                    .tooltip(Tooltip::text("Back"))
                    .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
            )
            .child(
                IconButton::new("xdi-forward", IconName::ArrowRight)
                    .icon_size(IconSize::Small)
                    .disabled(self.forward.is_empty())
                    .tooltip(Tooltip::text("Forward"))
                    .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
            )
            .child(
                div().min_w_0().flex_1().px_1().child(
                    Label::new(title)
                        .size(LabelSize::Small)
                        .weight(gpui::FontWeight::SEMIBOLD)
                        .truncate(),
                ),
            )
            .when(!process, |bar| {
                bar.child(
                    IconButton::new("xdi-less", IconName::Dash)
                        .icon_size(IconSize::Small)
                        .disabled(self.depth <= 1)
                        .tooltip(Tooltip::text("One mapping less each way"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.depth = this.depth.saturating_sub(1).max(1);
                            cx.notify();
                        })),
                )
                .child(
                    Label::new(format!("{} each way", self.depth))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child(
                    IconButton::new("xdi-more", IconName::Plus)
                        .icon_size(IconSize::Small)
                        .disabled(self.depth >= MAX_DEPTH)
                        .tooltip(Tooltip::text("One mapping more each way"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.depth = (this.depth + 1).min(MAX_DEPTH);
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new(
                    "xdi-columns",
                    if self.canvas.show_columns {
                        "Hide columns"
                    } else {
                        "Columns"
                    },
                )
                .label_size(LabelSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.canvas.show_columns = !this.canvas.show_columns;
                    if this.canvas.show_columns && this.canvas.zoom < COLUMNS_MIN_ZOOM {
                        this.canvas.set_zoom(1.);
                    }
                    if !this.canvas.show_columns {
                        this.canvas.selected_column = None;
                    }
                    cx.notify();
                })),
            )
            .child(
                IconButton::new("xdi-zoom-out", IconName::Dash)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Zoom out (⌘ scroll)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.canvas.set_zoom(this.canvas.zoom / 1.2);
                        cx.notify();
                    })),
            )
            .child(
                Label::new(format!("{:.0}%", self.canvas.zoom * 100.))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                IconButton::new("xdi-zoom-in", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Zoom in (⌘ scroll)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.canvas.set_zoom(this.canvas.zoom * 1.2);
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("xdi-center", IconName::Crosshair)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Back to the center"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.canvas.offsets.clear();
                        this.canvas.recenter = true;
                        this.canvas.recenter_tries = 0;
                        cx.notify();
                    })),
            )
            .when_some(open_job, |bar, job| {
                bar.child(
                    Button::new("xdi-open-map", "Open .map")
                        .label_size(LabelSize::Small)
                        .tooltip(Tooltip::text("Open the mapping's file: XML, often large"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_mapping(&job, window, cx)
                        })),
                )
            })
            .into_any_element()
    }

    fn render_canvas(
        &mut self,
        layout: Arc<XdiLayout>,
        around: Arc<Neighbourhood>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let edge_color = colors.text_muted;
        let node_bg = colors.elevated_surface_background;
        let accent = colors.text_accent;
        let text_color = colors.text;
        let muted_color = colors.text_muted;
        let border_color = colors.border;
        let zoom = self.canvas.zoom;
        let pan = self.canvas.pan;
        let name_text_size = px((13. * zoom).clamp(8., 22.));
        let column_text_size = px((12. * zoom).clamp(7., 18.));
        let header_height = GRAPH_NODE_HEIGHT * zoom;
        let row_height = GRAPH_COLUMN_ROW_HEIGHT * zoom;
        let show_columns = self.canvas.columns_shown();

        // Screen positions, drags included: what the edges and the boxes
        // both read, so they never disagree.
        let mut positioned = layout.graph.clone();
        for node in &mut positioned.nodes {
            let (dx, dy) = self
                .canvas
                .offsets
                .get(&node.name)
                .copied()
                .unwrap_or((0., 0.));
            node.x = (node.x + dx) * zoom + pan.0;
            node.y = (node.y + dy) * zoom + pan.1;
            node.width *= zoom;
            node.height *= zoom;
        }
        let positioned = Arc::new(positioned);
        let marks = self
            .canvas
            .selected_column
            .as_ref()
            .map(|column| Arc::new(DbtResultsPanel::column_highlights(&layout.graph, column)));
        let mut column_edge_color = edge_color;
        column_edge_color.a *= if marks.is_some() { 0.2 } else { 0.45 };

        let edge_nodes = positioned.clone();
        let edge_marks = marks.clone();
        let viewport = self.canvas.viewport.clone();
        let edges = canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                viewport.set((f32::from(bounds.size.width), f32::from(bounds.size.height)));
                let curve = |window: &mut Window,
                             start: Point<Pixels>,
                             end: Point<Pixels>,
                             width: f32,
                             color| {
                    let mid_x = (start.x + end.x) / 2.;
                    let mid = point(mid_x, (start.y + end.y) / 2.);
                    let mut builder = PathBuilder::stroke(px(width));
                    builder.move_to(start);
                    builder.curve_to(mid, point(mid_x, start.y));
                    builder.curve_to(end, point(mid_x, end.y));
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                };
                let arrow = |window: &mut Window, end: Point<Pixels>, color| {
                    let mut arrow = PathBuilder::fill();
                    arrow.move_to(end);
                    arrow.line_to(point(end.x - px(7.), end.y - px(4.)));
                    arrow.line_to(point(end.x - px(7.), end.y + px(4.)));
                    if let Ok(path) = arrow.build() {
                        window.paint_path(path, color);
                    }
                };
                for &(from_ix, to_ix) in &edge_nodes.edges {
                    let (Some(from), Some(to)) =
                        (edge_nodes.nodes.get(from_ix), edge_nodes.nodes.get(to_ix))
                    else {
                        continue;
                    };
                    let start = point(
                        bounds.origin.x + px(from.x + from.width),
                        bounds.origin.y + px(from.y + header_height / 2.),
                    );
                    let end = point(
                        bounds.origin.x + px(to.x - 2.),
                        bounds.origin.y + px(to.y + header_height / 2.),
                    );
                    curve(window, start, end, 1.5, edge_color);
                    arrow(window, end, edge_color);
                    if !show_columns {
                        continue;
                    }
                    // Column to column, as the column edges say.
                    let from_rows: HashMap<String, usize> = from
                        .columns
                        .iter()
                        .take(GRAPH_MAX_COLUMNS)
                        .enumerate()
                        .map(|(row, name)| (name.to_lowercase(), row))
                        .collect();
                    let row_y = |node: &GraphLayoutNode, row: usize| {
                        node.y + header_height + 2. * zoom + (row as f32 + 0.5) * row_height
                    };
                    for (to_row, to_column) in to.columns.iter().take(GRAPH_MAX_COLUMNS).enumerate()
                    {
                        let to_lower = to_column.to_lowercase();
                        let Some(leaves) = to.col_lineage.get(&to_lower) else {
                            continue;
                        };
                        for (_, source) in leaves.iter().filter(|(node, _)| *node == from.name) {
                            let Some(&from_row) = from_rows.get(source) else {
                                continue;
                            };
                            let start = point(
                                bounds.origin.x + px(from.x + from.width),
                                bounds.origin.y + px(row_y(from, from_row)),
                            );
                            let end = point(
                                bounds.origin.x + px(to.x),
                                bounds.origin.y + px(row_y(to, to_row)),
                            );
                            let lit = edge_marks.as_ref().is_some_and(|marks| {
                                marks.get(to_ix).is_some_and(|set| set.contains(&to_lower))
                                    && marks.get(from_ix).is_some_and(|set| set.contains(source))
                            });
                            if lit {
                                curve(window, start, end, 2.0, accent);
                                arrow(window, end, accent);
                            } else {
                                curve(window, start, end, 1.0, column_edge_color);
                            }
                        }
                    }
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();

        let mut surface = div()
            .id("xdi-canvas")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(colors.editor_background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    this.canvas.drag = Some(Drag::Canvas(event.position));
                }),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.canvas.drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.canvas.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                if event.modifiers.platform {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 120.,
                        gpui::ScrollDelta::Lines(delta) => delta.y / 8.,
                    };
                    this.canvas.set_zoom(this.canvas.zoom * (1. + delta));
                } else {
                    let (dx, dy) = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => {
                            (f32::from(delta.x), f32::from(delta.y))
                        }
                        gpui::ScrollDelta::Lines(delta) => (delta.x * 20., delta.y * 20.),
                    };
                    this.canvas.pan.0 += dx;
                    this.canvas.pan.1 += dy;
                }
                cx.notify();
            }))
            .child(edges);

        for (ix, node) in positioned.nodes.iter().enumerate() {
            let Some(id) = layout.ids.get(ix).cloned() else {
                continue;
            };
            let color = Self::color_for(&node.kind, &node.materialization, cx);
            let selected = self.selected.as_deref() == Some(id.as_str());
            let shown_columns = if show_columns {
                node.columns.len().min(GRAPH_MAX_COLUMNS)
            } else {
                0
            };
            let more_columns = node.columns.len().saturating_sub(shown_columns);
            let badges = if node.kind == "job" {
                Self::badges(node)
            } else {
                String::new()
            };
            let tooltip_meta = match node.kind.as_str() {
                "job" => node
                    .details
                    .as_ref()
                    .and_then(|detail| detail.get("path")?.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                _ => node
                    .details
                    .as_ref()
                    .and_then(|detail| detail.get("location")?.as_str().map(str::to_owned))
                    .or_else(|| {
                        node.details.as_ref().and_then(|detail| {
                            detail.get("technology")?.as_str().map(str::to_owned)
                        })
                    })
                    .unwrap_or_default(),
            };
            let title = node.name.clone();
            let mut boxed = div()
                .id(("xdi-node", ix))
                .absolute()
                .left(px(node.x))
                .top(px(node.y))
                .w(px(node.width))
                .h(px(node.height))
                .rounded_md()
                .bg(node_bg)
                .map(|this| {
                    if node.is_center || selected {
                        this.border_2()
                    } else {
                        this.border_1()
                    }
                })
                .border_color(if selected { accent } else { color })
                .when(node.is_center, |this| {
                    let mut glow = accent;
                    glow.a = 0.45;
                    this.shadow(vec![gpui::BoxShadow {
                        color: glow,
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(18.),
                        spread_radius: px(4.),
                        inset: false,
                    }])
                })
                .hover(|style| style.border_color(accent))
                .cursor_pointer()
                .tooltip(move |_window, cx| {
                    ui::Tooltip::with_meta(title.clone(), None, tooltip_meta.clone(), cx)
                })
                .child(
                    v_flex()
                        .size_full()
                        .child(
                            div()
                                .w_full()
                                .h(px(header_height - 2.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .px_1()
                                .when(shown_columns > 0, |this| {
                                    this.border_b_1().border_color(border_color)
                                })
                                .child(
                                    div()
                                        .text_size(name_text_size)
                                        .text_color(if node.is_center {
                                            accent
                                        } else {
                                            text_color
                                        })
                                        .overflow_hidden()
                                        .child(SharedString::from(node.name.clone())),
                                )
                                .when(!badges.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_size(name_text_size * 0.85)
                                            .text_color(color)
                                            .child(SharedString::from(badges.clone())),
                                    )
                                }),
                        )
                        .when(shown_columns > 0, |this| {
                            this.children(node.columns.iter().take(shown_columns).enumerate().map(
                                |(row, column)| {
                                    let lower = column.to_lowercase();
                                    let lit = marks.as_ref().is_some_and(|marks| {
                                        marks.get(ix).is_some_and(|set| set.contains(&lower))
                                    });
                                    let expression = node.col_exprs.get(&lower).cloned();
                                    let mut lit_bg = accent;
                                    lit_bg.a = 0.16;
                                    div()
                                        .id(SharedString::from(format!("xdi-col-{ix}-{row}")))
                                        .w_full()
                                        .h(px(row_height))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .overflow_hidden()
                                        .when(lit, |this| this.bg(lit_bg))
                                        .hover(|style| style.bg(lit_bg))
                                        .when_some(expression, |this, expression| {
                                            this.tooltip(ui::Tooltip::text(format!(
                                                "= {expression}"
                                            )))
                                        })
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.canvas.selected_column =
                                                if this.canvas.selected_column.as_deref()
                                                    == Some(lower.as_str())
                                                {
                                                    None
                                                } else {
                                                    Some(lower.clone())
                                                };
                                            cx.stop_propagation();
                                            cx.notify();
                                        }))
                                        .child(
                                            div()
                                                .text_size(column_text_size)
                                                .text_color(if lit { accent } else { muted_color })
                                                .child(SharedString::from(column.clone())),
                                        )
                                },
                            ))
                            .when(more_columns > 0, |this| {
                                this.child(
                                    div().w_full().h(px(row_height)).px_2().child(
                                        div()
                                            .text_size(column_text_size)
                                            .text_color(muted_color)
                                            .child(SharedString::from(format!(
                                                "+{more_columns} more"
                                            ))),
                                    ),
                                )
                            })
                        }),
                );
            boxed = boxed
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener({
                        let name = node.name.clone();
                        move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.canvas.drag_moved = false;
                            this.canvas.drag = Some(Drag::Node(name.clone(), event.position));
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, event: &gpui::ClickEvent, _, cx| {
                        if std::mem::take(&mut this.canvas.drag_moved) {
                            return;
                        }
                        if event.click_count() >= 2 {
                            this.navigate(Focus::Node(id.clone()), cx);
                        } else {
                            this.selected = Some(id.clone());
                            cx.notify();
                        }
                    }
                }));
            surface = surface.child(boxed);

            // "+" where there is more beyond.
            for (up, more) in [
                (true, around.more_up.contains(&id)),
                (false, around.more_down.contains(&id)),
            ] {
                if !more || self.expanded.contains(&id) {
                    continue;
                }
                let anchor_x = if up { node.x } else { node.x + node.width };
                let target = id.clone();
                surface = surface.child(
                    div()
                        .id(SharedString::from(format!(
                            "xdi-expand-{}-{ix}",
                            if up { "up" } else { "down" }
                        )))
                        .absolute()
                        .left(px(anchor_x - 9.))
                        .top(px(node.y + header_height / 2. - 9.))
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(node_bg)
                        .border_1()
                        .border_color(accent)
                        .cursor_pointer()
                        .tooltip(ui::Tooltip::text(format!(
                            "There is more {} {} — show it",
                            if up { "upstream of" } else { "downstream of" },
                            node.name
                        )))
                        .child(Label::new("+").size(LabelSize::Small).color(Color::Accent))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.expanded.insert(target.clone());
                            cx.stop_propagation();
                            cx.notify();
                        })),
                );
            }
        }
        if around.truncated {
            surface = surface.child(
                div().absolute().left_2().bottom_2().child(
                    Label::new(format!(
                        "{MAX_NODES} nodes at most: use + to go further from a node"
                    ))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
                ),
            );
        }
        surface.into_any_element()
    }

    fn render_sidebar(
        &self,
        snapshot: &Snapshot,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let node = snapshot.index.node(id)?;
        let colors = cx.theme().colors();
        let mut lines: Vec<gpui::AnyElement> = Vec::new();
        let heading = |text: &str| -> gpui::AnyElement {
            div()
                .pt_3()
                .pb_0p5()
                .child(
                    Label::new(text.to_owned())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .weight(gpui::FontWeight::SEMIBOLD),
                )
                .into_any_element()
        };
        let line = |text: String, muted: bool| -> gpui::AnyElement {
            div()
                .w_full()
                .child(Label::new(text).size(LabelSize::Small).color(if muted {
                    Color::Muted
                } else {
                    Color::Default
                }))
                .into_any_element()
        };
        lines.push(
            Label::new(node.label.clone())
                .size(LabelSize::Default)
                .weight(gpui::FontWeight::SEMIBOLD)
                .into_any_element(),
        );
        let project = &snapshot.project;
        if let Some(job) = project.job(id) {
            lines.push(line(job.path.clone(), true));
            if job.copy_like {
                lines.push(line("Named like a copy someone kept.".to_owned(), true));
            }
            let relation = |dataset: &Option<String>| -> String {
                dataset
                    .as_ref()
                    .and_then(|dataset| project.datasets.get(dataset))
                    .map(|dataset| {
                        let mut text =
                            format!("{} · {}", dataset.relation(), dataset.technology.label());
                        if !dataset.resolved {
                            text.push_str(" · no metadata");
                        }
                        text
                    })
                    .unwrap_or_else(|| "?".to_owned())
            };
            lines.push(heading("Sources"));
            for source in &job.sources {
                lines.push(line(
                    format!("{} = {}", source.alias, relation(&source.dataset)),
                    false,
                ));
            }
            if !job.stages.is_empty() {
                lines.push(heading("Stages"));
                for stage in &job.stages {
                    let area = stage
                        .staging_area
                        .as_ref()
                        .map(|area| {
                            format!(
                                " in {}{}",
                                area.server.clone().unwrap_or_default(),
                                area.schema
                                    .as_ref()
                                    .map(|schema| format!("/{schema}"))
                                    .unwrap_or_default()
                            )
                        })
                        .unwrap_or_default();
                    lines.push(line(
                        format!("{} = {}{area}", stage.alias, stage.set_expression),
                        false,
                    ));
                }
            }
            if !job.joins.is_empty() {
                lines.push(heading("Joins"));
                for join in &job.joins {
                    lines.push(line(
                        format!(
                            "{}{} {} {} on {}",
                            if join.enabled { "" } else { "(disabled) " },
                            join.left,
                            join.kind.label(),
                            join.right,
                            join.condition.text
                        ),
                        !join.enabled,
                    ));
                }
            }
            if !job.filters.is_empty() {
                lines.push(heading("Filters"));
                for filter in &job.filters {
                    lines.push(line(
                        format!(
                            "{}{}{}",
                            if filter.enabled { "" } else { "(disabled) " },
                            filter
                                .on
                                .as_ref()
                                .map(|on| format!("on {on}: "))
                                .unwrap_or_default(),
                            filter.condition.text
                        ),
                        !filter.enabled,
                    ));
                }
            }
            lines.push(heading("Targets"));
            for target in &job.targets {
                lines.push(line(
                    format!("{} = {}", target.alias, relation(&target.dataset)),
                    false,
                ));
                for template in job
                    .templates
                    .iter()
                    .filter(|template| template.target == target.alias)
                {
                    let parameters: Vec<String> = template
                        .parameters
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect();
                    lines.push(line(
                        format!(
                            "  {}{}",
                            template.template,
                            if parameters.is_empty() {
                                String::new()
                            } else {
                                format!(" ({})", parameters.join(", "))
                            }
                        ),
                        true,
                    ));
                }
                if !target.update_key.is_empty() {
                    lines.push(line(
                        format!("  update key: {}", target.update_key.join(", ")),
                        true,
                    ));
                }
            }
            lines.push(heading("Columns"));
            for column in job.columns.iter().filter(|column| !column.owner_is_stage) {
                let kind = if column.expression.approximate {
                    "guess"
                } else {
                    match column.expression.kind {
                        ExprKind::Copy => "copy",
                        ExprKind::Transform => "transform",
                        ExprKind::Constant => "constant",
                    }
                };
                lines.push(line(
                    format!(
                        "{}{}.{}{} = {}  · {kind}",
                        if column.enabled { "" } else { "(disabled) " },
                        column.owner,
                        column.column,
                        column
                            .set
                            .as_ref()
                            .filter(|_| job
                                .target(&column.owner)
                                .is_some_and(|target| target.sets.len() > 1))
                            .map(|set| format!(" [{set}]"))
                            .unwrap_or_default(),
                        column.expression.text
                    ),
                    !column.enabled,
                ));
            }
            if !job.tokens.is_empty() {
                lines.push(heading("Tokens"));
                for token in &job.tokens {
                    lines.push(line(token.kind.written(&token.text), false));
                }
            }
            lines.push(heading("Run by"));
            let callers = project.callers_of(&job.id);
            if callers.is_empty() {
                lines.push(line(
                    "No process of this project: a delivery or a scheduler may run it.".to_owned(),
                    true,
                ));
            }
            for caller in callers {
                lines.push(line(caller.path.clone(), false));
            }
            let issues: Vec<&el_import::model::Issue> = project
                .issues
                .iter()
                .filter(|issue| issue.path.as_deref() == Some(job.path.as_str()))
                .collect();
            if !issues.is_empty() {
                lines.push(heading("Problems"));
                for issue in issues {
                    lines.push(line(
                        format!(
                            "{}{}",
                            if issue.kind == IssueKind::Unparsed {
                                ""
                            } else {
                                "! "
                            },
                            issue.message
                        ),
                        false,
                    ));
                }
            }
        } else {
            let detail = &node.detail;
            let text = |key: &str| detail.get(key).and_then(Value::as_str).map(str::to_owned);
            lines.push(line(
                [text("technology"), node.connection.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                true,
            ));
            if let Some(location) = text("location") {
                lines.push(line(location, true));
            }
            if detail.get("resolved") == Some(&Value::Bool(false)) {
                lines.push(line(
                    "No metadata found: only its name is known.".to_owned(),
                    true,
                ));
            }
            if let Some(file) = text("metadata_file") {
                lines.push(line(format!("Described in {file}"), true));
            }
            if let Some(columns) = detail.get("columns").and_then(Value::as_array) {
                if !columns.is_empty() {
                    lines.push(heading("Columns"));
                }
                let key: Vec<&str> = detail
                    .get("primary_key")
                    .and_then(Value::as_array)
                    .map(|key| key.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                for column in columns {
                    let name = column
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mut text = name.to_owned();
                    if let Some(data_type) = column.get("type").and_then(Value::as_str) {
                        text.push_str(&format!(" {data_type}"));
                        if let Some(size) = column.get("size").and_then(Value::as_u64) {
                            text.push_str(&format!("({size})"));
                        }
                    }
                    if column.get("nullable") == Some(&Value::Bool(false)) {
                        text.push_str(" not null");
                    }
                    if key.contains(&name) {
                        text.push_str(" · key");
                    }
                    lines.push(line(text, false));
                }
            }
            for (title, nodes) in [
                ("Written by", snapshot.index.upstream(id)),
                ("Read by", snapshot.index.downstream(id)),
            ] {
                lines.push(heading(title));
                if nodes.is_empty() {
                    lines.push(line("No mapping of this project.".to_owned(), true));
                }
                for node in nodes {
                    lines.push(line(node.label.clone(), false));
                }
            }
        }
        Some(
            v_flex()
                .id("xdi-sidebar")
                .w(px(SIDEBAR_WIDTH))
                .flex_shrink_0()
                .h_full()
                .overflow_y_scroll()
                .p_3()
                .gap_0p5()
                .border_l_1()
                .border_color(colors.border)
                .bg(colors.panel_background)
                .child(
                    h_flex().w_full().justify_end().child(
                        IconButton::new("xdi-sidebar-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = None;
                                cx.notify();
                            })),
                    ),
                )
                .children(lines)
                .into_any_element(),
        )
    }

    fn title(&self, snapshot: Option<&Arc<Snapshot>>) -> String {
        match (&self.focus, snapshot) {
            (Some(Focus::Node(id)), Some(snapshot)) => snapshot
                .index
                .node(id)
                .map(|node| node.label.clone())
                .unwrap_or_else(|| id.clone()),
            (Some(Focus::Process(id)), Some(snapshot)) => snapshot
                .project
                .process(id)
                .map(|process| format!("Process {}", process.name))
                .unwrap_or_else(|| id.clone()),
            _ => "Lineage".to_owned(),
        }
    }

    fn message(text: &str) -> gpui::AnyElement {
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                Label::new(text.to_owned())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .into_any_element()
    }
}

impl EventEmitter<()> for XdiLineageView {}

impl Focusable for XdiLineageView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for XdiLineageView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        let name = self
            .store
            .read(cx)
            .active()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("Lineage: {name}").into()
    }

    fn tab_content(
        &self,
        params: TabContentParams,
        _window: &Window,
        cx: &App,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_1()
            .child(Icon::new(IconName::GitGraph).color(Color::Muted))
            .child(Label::new(self.tab_content_text(0, cx)).color(params.text_color()))
            .into_any_element()
    }
}

impl Render for XdiLineageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let snapshot = store.snapshot().cloned();
        let loading = store.loading();
        let title: SharedString = self.title(snapshot.as_ref()).into();
        let mut root = v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("XdiLineageView")
            .bg(cx.theme().colors().editor_background)
            .child(self.render_toolbar(title, cx));
        let Some(snapshot) = snapshot else {
            return root.child(Self::message(if loading {
                "Reading the project…"
            } else {
                "No xDI project read yet."
            }));
        };
        if self.focus.is_none() {
            return root.child(Self::message(
                "Choose a mapping, a table or a process in the xDI panel.",
            ));
        }
        let Some((layout, around)) = self.current_layout(&snapshot) else {
            return root.child(Self::message("Nothing to draw."));
        };
        if layout.graph.nodes.is_empty() {
            return root.child(Self::message(
                "Not in the project as last read: renamed, removed, or not read yet.",
            ));
        }
        if self.canvas.recenter {
            let center = layout
                .graph
                .nodes
                .iter()
                .find(|node| node.is_center)
                .or_else(|| layout.graph.nodes.first())
                .cloned();
            let centered = center.is_some_and(|node| self.canvas.center_on(&node));
            if centered {
                self.canvas.recenter = false;
                self.canvas.recenter_tries = 0;
            } else {
                // Not painted yet: try again next frame, a few times only.
                self.canvas.recenter_tries += 1;
                if self.canvas.recenter_tries <= 10 {
                    let entity = cx.entity_id();
                    window.on_next_frame(move |_, cx| cx.notify(entity));
                } else {
                    self.canvas.recenter = false;
                }
            }
        }
        let canvas = self.render_canvas(layout, around, cx);
        let sidebar = self
            .selected
            .clone()
            .and_then(|id| self.render_sidebar(&snapshot, &id, cx));
        root = root.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(canvas)
                .children(sidebar),
        );
        root
    }
}

#[cfg(test)]
mod render_tests {
    use gpui::{TestAppContext, VisualTestContext, size};
    use project::Project;
    use workspace::MultiWorkspace;

    use super::*;
    use crate::xdi::store::fixture_snapshot;

    const FACT: &str = "table:snowflake://xy12345.eu-west-1/sales_db/analytics/fact_orders";

    /// Draws the lineage tab headlessly in each state it can be in: what
    /// this catches is a state that cannot be drawn at all.
    #[gpui::test]
    async fn the_lineage_tab_draws_in_every_state(cx: &mut TestAppContext) {
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
        let store = cx.new(|cx| XdiStore::new(project.clone(), cx));
        let view = workspace.update_in(cx, |_, _, cx| {
            let workspace = cx.entity().downgrade();
            cx.new(|cx| XdiLineageView::new(workspace, store.clone(), cx))
        });
        let draw = |label: &str, cx: &mut VisualTestContext| {
            cx.draw(gpui::Point::default(), size(px(1400.), px(800.)), |_, _| {
                view.clone().into_any_element()
            });
            cx.run_until_parked();
            log::info!("drew: {label}");
        };
        draw("nothing read", cx);

        let dir = tempfile::tempdir().unwrap();
        let (root, snapshot) = fixture_snapshot(dir.path());
        store.update(cx, |store, cx| {
            store.set_snapshot(root.clone(), Some(snapshot), cx)
        });
        draw("read, nothing chosen", cx);

        view.update(cx, |view, cx| {
            view.navigate(Focus::Node(FACT.to_owned()), cx)
        });
        draw("around a table", cx);
        draw("around a table, centered", cx);
        view.update(cx, |view, cx| {
            view.canvas.show_columns = true;
            view.canvas.selected_column = Some("amount".to_owned());
            cx.notify();
        });
        draw("columns, one traced", cx);
        view.update(cx, |view, cx| {
            view.canvas.set_zoom(0.3);
            cx.notify();
        });
        draw("zoomed out: columns hidden", cx);

        // A mapping, selected with its sidebar, then deeper and expanded.
        view.update(cx, |view, cx| {
            view.canvas.set_zoom(1.);
            view.navigate(
                Focus::Node("job:xdi:Orders/Mappings/LOAD_ORDERS.map".to_owned()),
                cx,
            );
            view.depth = 6;
            view.expanded.insert(FACT.to_owned());
            cx.notify();
        });
        draw("around a mapping, deeper", cx);
        view.update(cx, |view, cx| {
            view.selected = Some(FACT.to_owned());
            cx.notify();
        });
        draw("a table's sidebar", cx);

        // A process, back, forward, and something gone.
        view.update(cx, |view, cx| {
            view.navigate(
                Focus::Process("process:xdi:Orders/Processes/RUN_ORDERS.proc".to_owned()),
                cx,
            )
        });
        draw("a process", cx);
        view.update(cx, |view, cx| view.go_back(cx));
        draw("back", cx);
        view.update(cx, |view, cx| view.go_forward(cx));
        draw("forward", cx);
        view.update(cx, |view, cx| {
            view.navigate(Focus::Node("job:xdi:gone.map".to_owned()), cx)
        });
        draw("gone", cx);
        assert_eq!(view.read_with(cx, |view, _| view.back.len()), 3);
        assert_eq!(
            view.read_with(cx, |view, cx| view.tab_content_text(0, cx)),
            "Lineage: Sales"
        );
    }
}
