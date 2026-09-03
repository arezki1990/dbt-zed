use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use editor::{Editor, ToOffset as _};
use gpui::{
    App, AsyncWindowContext, ClipboardItem, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, MouseButton, PathBuilder, Pixels, Point, ScrollHandle, Subscription,
    Task, WeakEntity, Window, anchored, canvas, deferred, point, px,
};
use language::LanguageRegistry;
use settings::Settings as _;
use ui::{
    ColumnWidthConfig, ContextMenu, ResizableColumnsState, Table, TableInteractionState,
    TableResizeBehavior, prelude::*,
};
use util::command::new_command;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::{
    ShowModelData, ToggleFocus,
    dbt_settings::DbtSettings,
    lineage::{
        GRAPH_COL_GAP, GRAPH_COLUMN_ROW_HEIGHT, GRAPH_MAX_COLUMNS, GRAPH_NODE_HEIGHT,
        GRAPH_PADDING, GRAPH_ROW_GAP, LayoutGraph, LineageStore, LineageTree, LineageTreeNode,
    },
};

/// Upstream-facing source names for `column` on `node`: the stored references
/// of its full expression, closed transitively through the node's own
/// CTE-level entries — so `MAX(absence_date)` where `absence_date` was itself
/// renamed from `date_absence` in an earlier CTE resolves to `date_absence`.
fn column_sources(node: &crate::lineage::GraphLayoutNode, column: &str) -> Vec<String> {
    let mut sources = vec![column.to_owned()];
    let mut queue = vec![column.to_owned()];
    let mut steps = 0;
    while let Some(current) = queue.pop() {
        steps += 1;
        if steps > 64 || sources.len() > 32 {
            break;
        }
        let refs = node.col_refs.get(&current).cloned().or_else(|| {
            node.col_exprs
                .get(&current)
                .map(|expr| crate::lineage::expr_column_refs(expr))
        });
        for referenced in refs.unwrap_or_default() {
            if !sources.contains(&referenced) {
                sources.push(referenced.clone());
                queue.push(referenced);
            }
        }
    }
    sources
}

/// Source columns on `from` feeding `column` of `to`. Exact AST lineage wins
/// when present (including "no link from this parent" — the precision case);
/// otherwise the name/reference heuristic closure decides.
fn sources_toward(
    from: &crate::lineage::GraphLayoutNode,
    to: &crate::lineage::GraphLayoutNode,
    column: &str,
) -> Vec<String> {
    if let Some(leaves) = to.col_lineage.get(column) {
        return leaves
            .iter()
            .filter(|(node, _)| *node == from.name)
            .map(|(_, source)| source.clone())
            .collect();
    }
    column_sources(to, column)
}

/// One-glance badge string for a node's SQL operations, e.g. "⋈2 Σ σ ƒ".
fn ops_badges(ops: &crate::lineage::NodeOps) -> String {
    let mut badges = Vec::new();
    if !ops.joins.is_empty() {
        badges.push(format!("⋈{}", ops.joins.len()));
    }
    if ops.group_by || !ops.aggregations.is_empty() {
        badges.push("Σ".to_owned());
    }
    if !ops.filters.is_empty() {
        badges.push("σ".to_owned());
    }
    if ops.windows {
        badges.push("ƒ".to_owned());
    }
    if ops.distinct {
        badges.push("D".to_owned());
    }
    if ops.unions > 0 {
        badges.push("∪".to_owned());
    }
    badges.join(" ")
}

/// Multi-line tooltip describing a node's operations for debugging.
fn ops_tooltip(ops: &crate::lineage::NodeOps) -> String {
    let mut lines = Vec::new();
    if !ops.joins.is_empty() {
        lines.push(format!("Joins: {}", ops.joins.join(", ")));
    }
    if !ops.aggregations.is_empty() {
        let group = if ops.group_by { " over group by" } else { "" };
        lines.push(format!("Aggregations: {}{group}", ops.aggregations.join(", ")));
    } else if ops.group_by {
        lines.push("Aggregation: group by".to_owned());
    }
    if !ops.filters.is_empty() {
        lines.push(format!("Filters: {}", ops.filters.join(", ")));
    }
    if ops.windows {
        lines.push("Window functions".to_owned());
    }
    if ops.distinct {
        lines.push("Select distinct".to_owned());
    }
    if ops.unions > 0 {
        lines.push(format!("Union of {} branches", ops.unions + 1));
    }
    lines.join("\n")
}

/// Below this zoom, column rows (and their edges) collapse away — node text
/// scales with zoom, so columns stay legible well under 100%.
const COLUMNS_MIN_ZOOM: f32 = 0.55;

pub struct DbtResultsPanel {
    focus_handle: FocusHandle,
    table_interaction: Entity<TableInteractionState>,
    languages: Arc<LanguageRegistry>,
    workspace: WeakEntity<Workspace>,
    lineage_store: Arc<LineageStore>,
    state: ResultsState,
    view: ResultsView,
    compiled_editor: Option<Entity<Editor>>,
    lineage_layout: Option<Arc<LayoutGraph>>,
    lineage_tree: Option<Arc<LineageTree>>,
    expanded: HashSet<SharedString>,
    show_upstream: bool,
    show_downstream: bool,
    show_columns: bool,
    show_tree: bool,
    /// Details card sidebar for the centered model.
    show_details: bool,
    collapsed_up: HashSet<String>,
    collapsed_down: HashSet<String>,
    selected_column: Option<String>,
    pan: (f32, f32),
    zoom: f32,
    node_offsets: HashMap<String, (f32, f32)>,
    graph_drag: Option<GraphDrag>,
    drag_moved: bool,
    canvas_scroll: ScrollHandle,
    /// The model the current lineage graph is centered on.
    lineage_model: Option<String>,
    search_editor: Entity<Editor>,
    /// Active sort: (column index, ascending).
    sort: Option<(usize, bool)>,
    column_widths: Option<Entity<ResizableColumnsState>>,
    hidden_columns: HashSet<usize>,
    show_column_picker: bool,
    last_root: Option<PathBuf>,
    /// Project roots already auto-parsed this session.
    parsed_roots: HashSet<PathBuf>,
    /// Recenter the viewport on the browsed model at the next paint-valid
    /// render (set when a lineage refresh lands while the canvas isn't
    /// painted yet, e.g. another results tab is active).
    pending_center: bool,
    /// Consecutive render frames spent waiting for canvas bounds.
    center_retry_frames: u8,
    /// Nodes expanded past the lineage depth budget via their + handle.
    depth_expansions: HashSet<String>,
    /// When set, the lineage view shows a DAG focused on this column only.
    column_focus: Option<String>,
    /// Pan/zoom to restore when leaving column focus.
    focus_return: Option<((f32, f32), f32)>,
    /// Open right-click menu on a column row.
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _run: Task<()>,
    _lineage_refresh: Task<()>,
}

enum GraphDrag {
    Node(String, Point<Pixels>),
    Canvas(Point<Pixels>),
}

#[derive(Copy, Clone, PartialEq)]
enum ResultsView {
    Table,
    Compiled,
    Lineage,
}

/// What `dbt show` should execute: a whole model, or an ad-hoc SQL chunk
/// (editor selection) compiled with `--inline`.
pub enum ShowTarget {
    Model {
        name: SharedString,
        rel_path: PathBuf,
    },
    Inline(String),
}

impl ShowTarget {
    fn label(&self) -> SharedString {
        match self {
            ShowTarget::Model { name, .. } => name.clone(),
            ShowTarget::Inline(_) => "selection".into(),
        }
    }
}

enum ResultsState {
    Empty,
    Running {
        model: SharedString,
    },
    Failed {
        model: SharedString,
        message: SharedString,
    },
    Loaded {
        model: SharedString,
        columns: Arc<Vec<SharedString>>,
        rows: Arc<Vec<Vec<SharedString>>>,
        compiled: Option<SharedString>,
    },
}

/// Finds the dbt project root for a model file: the configured `project_dir`
/// setting when valid, otherwise the nearest ancestor of the file (within the
/// worktree) containing dbt_project.yml — so nested layouts like
/// `<repo>/employees/dbt_project.yml` load automatically.
fn discover_project_root(
    file_abs: &std::path::Path,
    worktree_root: &std::path::Path,
    configured: Option<&str>,
) -> Option<PathBuf> {
    if let Some(configured) = configured {
        let candidate = if std::path::Path::new(configured).is_absolute() {
            PathBuf::from(configured)
        } else {
            worktree_root.join(configured)
        };
        if candidate.join("dbt_project.yml").is_file() {
            return Some(candidate);
        }
    }
    let mut dir = file_abs.parent()?;
    loop {
        if dir.join("dbt_project.yml").is_file() {
            return Some(dir.to_path_buf());
        }
        if dir == worktree_root || !dir.starts_with(worktree_root) {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// Resolves the dbt model open in the active editor, for lineage-follows-file
/// browsing.
fn active_model_file(workspace: &Workspace, cx: &App) -> Option<(String, PathBuf)> {
    let editor = workspace.active_item(cx)?.act_as::<Editor>(cx)?;
    let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
    let buffer = buffer.read(cx);
    let file = buffer.file()?;
    let abs_path = file.as_local()?.abs_path(cx);
    // Any .sql file inside a dbt project counts as a model, regardless of
    // which language claimed the buffer (the sql extension may win the
    // suffix in shared configs).
    if abs_path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
        return None;
    }
    let model = abs_path.file_stem()?.to_str()?.to_owned();
    let worktree_root = workspace
        .project()
        .read(cx)
        .worktree_for_id(file.worktree_id(cx), cx)?
        .read(cx)
        .abs_path()
        .to_path_buf();
    let settings = DbtSettings::get_global(cx);
    let root = discover_project_root(
        &abs_path,
        &worktree_root,
        settings.project_dir.as_deref(),
    )?;
    Some((model, root))
}

/// Resolves the model (or selection) under the active editor and runs
/// `dbt show` for it, revealing the results panel.
pub fn show_model_data(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(panel) = workspace.panel::<DbtResultsPanel>(cx) else {
        return;
    };

    let resolved = workspace
        .active_item(cx)
        .and_then(|item| item.act_as::<Editor>(cx))
        .and_then(|editor| {
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let file = buffer.read(cx).file()?;
            let abs_path = file.as_local()?.abs_path(cx);
            let model = abs_path.file_stem()?.to_str()?.to_owned();
            let worktree = workspace
                .project()
                .read(cx)
                .worktree_for_id(file.worktree_id(cx), cx)?;
            let worktree_root = worktree.read(cx).abs_path().to_path_buf();
            let settings = DbtSettings::get_global(cx);
            let root = discover_project_root(
                &abs_path,
                &worktree_root,
                settings.project_dir.as_deref(),
            )?;
            let rel_path = abs_path.strip_prefix(&root).ok()?.to_path_buf();

            // A non-empty selection runs as an ad-hoc query, SQL-IDE style;
            // otherwise the whole model runs.
            let snapshot = editor.read(cx).buffer().read(cx).snapshot(cx);
            let selection = editor.read(cx).selections.newest_anchor().clone();
            let range = selection.start.to_offset(&snapshot)..selection.end.to_offset(&snapshot);
            let target = if range.is_empty() {
                ShowTarget::Model {
                    name: model.into(),
                    rel_path,
                }
            } else {
                ShowTarget::Inline(snapshot.text_for_range(range).collect::<String>())
            };
            Some((target, root))
        });

    workspace.focus_panel::<DbtResultsPanel>(window, cx);
    panel.update(cx, |panel, cx| match resolved {
        Some((target, root)) => panel.run_show(target, root, window, cx),
        None => {
            panel.state = ResultsState::Failed {
                model: "?".into(),
                message: "Open a dbt model file first, then run dbt: show model data.".into(),
            };
            cx.notify();
        }
    });
}

impl DbtResultsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            Self::new(workspace, window, cx)
        })
    }

    pub fn new(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let languages = workspace.project().read(cx).languages().clone();
        let workspace_handle = cx.entity().downgrade();
        let workspace_entity = cx.entity().clone();
        cx.new(|cx| {
            let search_editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Search results…", window, cx);
                editor
            });
            cx.subscribe(
                &search_editor,
                |_, _, event: &editor::EditorEvent, cx| {
                    if matches!(event, editor::EditorEvent::BufferEdited) {
                        cx.notify();
                    }
                },
            )
            .detach();

            // Browsing model files drives the lineage: whenever the active
            // editor changes to a dbt model, recenter the graph on it.
            cx.subscribe(
                &workspace_entity,
                |this: &mut Self, workspace, event: &workspace::Event, cx| {
                    if let workspace::Event::ActiveItemChanged = event {
                        if let Some((model, root)) = active_model_file(workspace.read(cx), cx)
                        {
                            this.refresh_lineage(model, root, cx);
                        }
                    }
                },
            )
            .detach();
            Self {
            focus_handle: cx.focus_handle(),
            table_interaction: cx.new(|cx| {
                TableInteractionState::new(cx).with_custom_scrollbar(
                    ui::Scrollbars::for_settings::<editor::EditorSettingsScrollbarProxy>(),
                )
            }),
            languages,
            workspace: workspace_handle,
            lineage_store: Arc::new(LineageStore::default()),
            state: ResultsState::Empty,
            view: ResultsView::Table,
            compiled_editor: None,
            lineage_layout: None,
            lineage_tree: None,
            expanded: Default::default(),
            show_upstream: true,
            show_downstream: true,
            show_columns: false,
            show_tree: true,
            show_details: false,
            collapsed_up: Default::default(),
            collapsed_down: Default::default(),
            selected_column: None,
            pan: (0., 0.),
            zoom: 1.0,
            node_offsets: Default::default(),
            graph_drag: None,
            drag_moved: false,
                canvas_scroll: ScrollHandle::new(),
                lineage_model: None,
                search_editor,
                sort: None,
                column_widths: None,
                hidden_columns: Default::default(),
                show_column_picker: false,
                last_root: None,
                parsed_roots: Default::default(),
                pending_center: false,
                center_retry_frames: 0,
                depth_expansions: Default::default(),
                column_focus: None,
                focus_return: None,
                context_menu: None,
                _run: Task::ready(()),
                _lineage_refresh: Task::ready(()),
            }
        })
    }

    /// Row indices of the loaded results after applying search and sort.
    fn display_indices(
        &self,
        rows: &[Vec<SharedString>],
        cx: &App,
    ) -> Vec<usize> {
        let query = self.search_editor.read(cx).text(cx).trim().to_lowercase();
        let mut indices: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                query.is_empty()
                    || row
                        .iter()
                        .any(|cell| cell.to_lowercase().contains(query.as_str()))
            })
            .map(|(ix, _)| ix)
            .collect();
        if let Some((column, ascending)) = self.sort {
            let compare = |a: &str, b: &str| match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                _ => a.cmp(b),
            };
            indices.sort_by(|&a, &b| {
                let a = rows[a].get(column).map(|c| c.as_ref()).unwrap_or("");
                let b = rows[b].get(column).map(|c| c.as_ref()).unwrap_or("");
                let ordering = compare(a, b);
                if ascending { ordering } else { ordering.reverse() }
            });
        }
        indices
    }

    fn export_csv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ResultsState::Loaded {
            model,
            columns,
            rows,
            ..
        } = &self.state
        else {
            return;
        };
        let Some(root) = self.last_root.clone() else {
            return;
        };
        let quote = |cell: &str| -> String {
            if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
                format!("\"{}\"", cell.replace('"', "\"\""))
            } else {
                cell.to_owned()
            }
        };
        let mut out = String::new();
        out.push_str(
            &columns
                .iter()
                .map(|column| quote(column))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
        for ix in self.display_indices(rows, cx) {
            out.push_str(
                &rows[ix]
                    .iter()
                    .map(|cell| quote(cell))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
        let path = root.join("target").join(format!("{model}_results.csv"));
        match std::fs::write(&path, out) {
            Ok(()) => {
                self.workspace
                    .update(cx, |workspace, cx| {
                        workspace
                            .open_abs_path(
                                path,
                                workspace::OpenOptions::default(),
                                window,
                                cx,
                            )
                            .detach();
                    })
                    .ok();
            }
            Err(error) => log::error!("dbt: csv export failed: {error}"),
        }
    }

    /// Recomputes the lineage graph centered on `model` without running any
    /// query — this is what makes file browsing drive the graph.
    fn refresh_lineage(&mut self, model: String, root: PathBuf, cx: &mut Context<Self>) {
        if self.lineage_model.as_deref() == Some(model.as_str()) {
            // Same model still active: only pull it back when it has actually
            // left the viewport — never fight manual panning.
            if !self.center_in_view() {
                self.pending_center = true;
                cx.notify();
            }
            return;
        }

        // First detection of a project this session: refresh its manifest so
        // the lineage graph reflects the current model files.
        let settings = DbtSettings::get_global(cx).clone();
        let graph_depth = settings.lineage_depth as i32;
        let tree_depth = settings.lineage_tree_depth as usize;
        let max_nodes = settings.lineage_max_nodes as usize;
        if settings.parse_on_load && !self.parsed_roots.contains(&root) {
            self.parsed_roots.insert(root.clone());
            let command_root = root.clone();
            let parse_root = root.clone();
            let parse_model = model.clone();
            let catalog_settings = settings.clone();
            let http = self.http_client(cx);
            let parse = cx.background_spawn(async move {
                let binary = crate::dbt_install::ensure_binary(&settings, http)
                    .await
                    .map_err(std::io::Error::other)?;
                let mut command = new_command(&binary);
                command.arg("parse");
                apply_common_args(&mut command, &settings, &command_root);
                command.current_dir(&command_root).output().await
            });
            cx.spawn(async move |this, cx| {
                match parse.await {
                    Ok(output) if output.status.success() => {
                        log::info!("dbt: auto-parse succeeded for {parse_root:?}");
                        this.update(cx, |this, cx| {
                            this.lineage_model = None;
                            this.refresh_lineage(parse_model.clone(), parse_root.clone(), cx);
                        })
                        .ok();
                    }
                    Ok(output) => {
                        log::warn!(
                            "dbt: auto-parse failed for {parse_root:?}:\n{}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                        return;
                    }
                    Err(error) => {
                        log::warn!("dbt: auto-parse spawn failed: {error:#}");
                        return;
                    }
                }

                // Second stage: refresh the catalog so sources (which only get
                // columns from catalog.json) participate in column lineage.
                let catalog_root = parse_root.clone();
                let catalog = cx.background_executor().spawn(async move {
                    let binary = crate::dbt_install::ensure_binary(&catalog_settings, None)
                        .await
                        .map_err(std::io::Error::other)?;
                    let mut command = new_command(&binary);
                    // Fusion writes the catalog from compile; Core uses docs.
                    if catalog_settings.distribution == "core" {
                        command.args(["docs", "generate"]);
                    } else {
                        command.args(["compile", "--write-catalog"]);
                    }
                    apply_common_args(&mut command, &catalog_settings, &catalog_root);
                    command.current_dir(&catalog_root).output().await
                });
                match catalog.await {
                    Ok(output) if output.status.success() => {
                        log::info!("dbt: catalog refreshed for {parse_root:?}");
                        this.update(cx, |this, cx| {
                            this.lineage_model = None;
                            this.refresh_lineage(parse_model, parse_root, cx);
                        })
                        .ok();
                    }
                    Ok(output) => log::warn!(
                        "dbt: catalog refresh failed for {parse_root:?}:\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    ),
                    Err(error) => log::warn!("dbt: catalog spawn failed: {error:#}"),
                }
            })
            .detach();
        }

        self.lineage_model = Some(model.clone());
        let store = self.lineage_store.clone();
        let expansions = self.depth_expansions.clone();
        let task = cx.background_spawn(async move {
            let tree = store.lineage_tree(&root, &model, tree_depth, max_nodes).ok();
            let layout = store
                .lineage_layout(&root, &model, graph_depth, max_nodes, &expansions)
                .ok();
            (tree, layout)
        });
        self._lineage_refresh = cx.spawn(async move |this, cx| {
            let (tree, layout) = task.await;
            this.update(cx, |this, cx| {
                if tree.is_none() && layout.is_none() {
                    return;
                }
                this.lineage_tree = tree.map(Arc::new);
                log::debug!(
                    "dbt lineage: layout arrived for {:?} ({} nodes)",
                    this.lineage_model,
                    this.lineage_layout.as_ref().map_or(0, |l| l.nodes.len()),
                );
                this.lineage_layout = layout.map(Arc::new);
                this.expanded.clear();
                this.collapsed_up.clear();
                this.collapsed_down.clear();
                this.selected_column = None;
                this.pan = (0., 0.);
                this.node_offsets.clear();
                this.graph_drag = None;
                this.view = ResultsView::Lineage;
                // Centering last: it owns the final pan — nothing below may
                // reset it (that exact stomp hid the first-open centering).
                this.pending_center = !this.center_on_model();
                cx.notify();
            })
            .ok();
        });
    }

    /// The app's HTTP client, used to download the managed dbt Fusion
    /// distribution when nothing is on PATH. Read from the global Client —
    /// never from the Workspace entity, which is mid-update when panel
    /// actions run (reading it there double-leases and panics).
    fn http_client(&self, cx: &Context<Self>) -> Option<Arc<dyn http_client::HttpClient>> {
        let client: Arc<dyn http_client::HttpClient> =
            client::Client::global(cx).http_client();
        Some(client)
    }

    fn run_show(
        &mut self,
        target: ShowTarget,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = target.label();
        self.state = ResultsState::Running {
            model: model.clone(),
        };
        cx.notify();

        self.last_root = Some(root.clone());
        let settings = DbtSettings::get_global(cx).clone();
        let lineage_store = self.lineage_store.clone();
        let http = self.http_client(cx);
        let depth_expansions = self.depth_expansions.clone();
        let command = cx.background_spawn(async move {
            let limit = settings.show_limit.to_string();
            let binary = crate::dbt_install::ensure_binary(&settings, http).await?;
            let mut command = new_command(&binary);
            command.arg("show");
            match &target {
                ShowTarget::Model { name, .. } => {
                    command.args(["--select", name.as_ref()]);
                }
                ShowTarget::Inline(sql) => {
                    command.args(["--inline", sql]);
                }
            }
            command.args(["--limit", &limit, "--output", "json"]);
            apply_common_args(&mut command, &settings, &root);
            let output = command
                .current_dir(&root)
                .output()
                .await
                .with_context(|| format!("spawning `{binary} show`"))?;
            let (columns, rows) =
                parse_show_output(&output.stdout, &output.stderr, output.status.success())?;
            let compiled = fetch_compiled_sql(&binary, &settings, &target, &root).await;
            let (lineage_tree, lineage_layout) = match &target {
                ShowTarget::Model { name, .. } => (
                    lineage_store
                        .lineage_tree(
                            &root,
                            name.as_ref(),
                            settings.lineage_tree_depth as usize,
                            settings.lineage_max_nodes as usize,
                        )
                        .ok(),
                    lineage_store
                        .lineage_layout(
                            &root,
                            name.as_ref(),
                            settings.lineage_depth as i32,
                            settings.lineage_max_nodes as usize,
                            &depth_expansions,
                        )
                        .ok(),
                ),
                ShowTarget::Inline(_) => (None, None),
            };
            anyhow::Ok((columns, rows, compiled, lineage_tree, lineage_layout))
        });

        let languages = self.languages.clone();
        self._run = cx.spawn_in(window, async move |this, cx| {
            let result = command.await;
            let result = result.map(|(columns, rows, compiled, lineage_tree, lineage_layout)| {
                (
                    columns,
                    rows,
                    compiled,
                    lineage_tree.map(Arc::new),
                    lineage_layout.map(Arc::new),
                )
            });
            let sql_language = languages.language_for_name("SQL (dbt)").await.ok();
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((columns, rows, compiled, lineage_tree, lineage_layout)) => {
                        this.lineage_layout = lineage_layout;
                        this.pending_center = !this.center_on_model();
                        this.lineage_tree = lineage_tree;
                        this.expanded.clear();
                        this.collapsed_up.clear();
                        this.collapsed_down.clear();
                        this.selected_column = None;
                        this.pan = (0., 0.);
                        this.node_offsets.clear();
                        this.graph_drag = None;
                        this.compiled_editor = compiled.as_ref().map(|sql| {
                            let buffer = cx.new(|cx| {
                                let mut buffer = language::Buffer::local(sql.clone(), cx);
                                buffer.set_language(sql_language.clone(), cx);
                                buffer
                            });
                            cx.new(|cx| {
                                let mut editor = Editor::for_buffer(buffer, None, window, cx);
                                editor.set_read_only(true);
                                editor
                            })
                        });
                        this.lineage_model = Some(model.to_string());
                        this.view = ResultsView::Table;
                        this.sort = None;
                        this.hidden_columns.clear();
                        // Content-aware initial widths; drag handles resize
                        // like a spreadsheet from there. Column 0 is the
                        // pinned row-number gutter (pinning also activates
                        // the table's scroll-aware resize layout).
                        let mut widths: Vec<Pixels> = vec![px(56.)];
                        widths.extend(columns.iter().enumerate().map(
                            |(column_ix, column)| {
                                let mut chars = column.len();
                                for row in rows.iter().take(30) {
                                    if let Some(cell) = row.get(column_ix) {
                                        chars = chars.max(cell.len());
                                    }
                                }
                                px((chars as f32 * 8.2 + 24.).clamp(90., 340.))
                            },
                        ));
                        this.column_widths = (!columns.is_empty()).then(|| {
                            let state = cx.new(|_| {
                                // MinSize is in rems: 3.75rem ≈ 60px columns,
                                // 2rem ≈ 32px row-number gutter.
                                let mut behaviors =
                                    vec![TableResizeBehavior::MinSize(3.75); columns.len() + 1];
                                behaviors[0] = TableResizeBehavior::MinSize(2.);
                                ResizableColumnsState::new(
                                    columns.len() + 1,
                                    widths,
                                    behaviors,
                                )
                            });
                            // The table updates this entity on every drag
                            // frame; observing it makes the resize track the
                            // pointer smoothly instead of jumping on release.
                            cx.observe(&state, |_, _, cx| cx.notify()).detach();
                            state
                        });
                        this.search_editor.update(cx, |editor, cx| {
                            editor.set_text("", window, cx);
                        });
                        this.state = ResultsState::Loaded {
                            model,
                            columns: Arc::new(columns),
                            rows: Arc::new(rows),
                            compiled: compiled.map(SharedString::from),
                        };
                    }
                    Err(error) => {
                        this.compiled_editor = None;
                        this.state = ResultsState::Failed {
                            model,
                            message: format!("{error:#}").into(),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = match &self.state {
            ResultsState::Empty => "dbt results".into(),
            ResultsState::Running { model } => format!("dbt show {model} — running…").into(),
            ResultsState::Failed { model, .. } => format!("dbt show {model} — failed").into(),
            ResultsState::Loaded { model, rows, .. } => {
                format!("dbt show {model} — {} rows", rows.len()).into()
            }
        };
        h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                IconButton::new("dbt-show-refresh", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .tooltip(ui::Tooltip::for_action_title(
                        "Show data for the active model",
                        &ShowModelData,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ShowModelData), cx);
                    }),
            )
            .child(Label::new(title).size(LabelSize::Small))
            .when(
                self.view == ResultsView::Table
                    && matches!(self.state, ResultsState::Loaded { .. }),
                |this| {
                    this.child(
                        div()
                            .w(px(200.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(self.search_editor.clone()),
                    )
                    .child(
                        Button::new("dbt-export-csv", "Export CSV").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.export_csv(window, cx);
                            },
                        )),
                    )
                    .child(
                        Button::new("dbt-column-picker-toggle", "Columns")
                            .toggle_state(self.show_column_picker)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_column_picker = !this.show_column_picker;
                                cx.notify();
                            })),
                    )
                },
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("dbt-view-results", "Results")
                            .toggle_state(self.view == ResultsView::Table)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = ResultsView::Table;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("dbt-view-compiled", "Compiled")
                            .toggle_state(self.view == ResultsView::Compiled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = ResultsView::Compiled;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("dbt-view-lineage", "Lineage")
                            .toggle_state(self.view == ResultsView::Lineage)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = ResultsView::Lineage;
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new("dbt-open-settings", IconName::Settings)
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text("Open dbt settings"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    Box::new(zed_actions::OpenSettingsPage {
                                        page: "dbt".into(),
                                        target: None,
                                    }),
                                    cx,
                                );
                            }),
                    ),
            )
    }

    fn graph_mouse_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        // Self-heal: if the button was released and something swallowed the
        // mouse-up, end the drag rather than sticking to the pointer.
        if self.graph_drag.is_some() && event.pressed_button != Some(MouseButton::Left) {
            self.graph_drag = None;
            cx.notify();
            return;
        }
        // (dragged node, delta) — None node = pan.
        let (node, delta) = match &mut self.graph_drag {
            None => return,
            Some(GraphDrag::Node(name, last)) => {
                let delta = event.position - *last;
                *last = event.position;
                (Some(name.clone()), delta)
            }
            Some(GraphDrag::Canvas(last)) => {
                let delta = event.position - *last;
                *last = event.position;
                (None, delta)
            }
        };
        match node {
            Some(name) => {
                if delta.x != px(0.) || delta.y != px(0.) {
                    self.drag_moved = true;
                }
                let offset = self.node_offsets.entry(name).or_insert((0., 0.));
                offset.0 += f32::from(delta.x);
                offset.1 += f32::from(delta.y);
            }
            None => {
                self.pan.0 += f32::from(delta.x);
                self.pan.1 += f32::from(delta.y);
            }
        }
        cx.notify();
    }

    fn graph_end_drag(&mut self, cx: &mut Context<Self>) {
        self.graph_drag = None;
        cx.notify();
    }

    /// The interactive lineage graph in a pannable viewport; shared between
    /// the Results view (side pane) and the Lineage view.
    /// When a column is selected, an overlay tracing the transformation each
    /// model on the path applies to it — source to target, in level order.
    /// Per-node set of column names on the selected column's lineage path,
    /// propagated transitively through renames in both directions: a target
    /// column marks the upstream columns its expression references, and a
    /// downstream column referencing a marked one gets marked too.
    pub(crate) fn column_highlights(
        layout: &LayoutGraph,
        selected: &str,
    ) -> Vec<std::collections::HashSet<String>> {
        let mut marked: Vec<std::collections::HashSet<String>> =
            vec![Default::default(); layout.nodes.len()];
        for (ix, node) in layout.nodes.iter().enumerate() {
            if node
                .columns
                .iter()
                .any(|column| column.to_lowercase() == selected)
            {
                marked[ix].insert(selected.to_owned());
            }
        }

        let mut passes = 0;
        loop {
            let mut changed = false;
            passes += 1;
            for &(from_ix, to_ix) in &layout.edges {
                let from = &layout.nodes[from_ix];
                let to = &layout.nodes[to_ix];
                // Downstream: target columns whose sources are marked upstream.
                let mut add_to = Vec::new();
                for column in to.columns.iter() {
                    let column = column.to_lowercase();
                    if marked[to_ix].contains(&column) {
                        continue;
                    }
                    if sources_toward(from, to, &column)
                        .iter()
                        .any(|source| marked[from_ix].contains(source))
                    {
                        add_to.push(column);
                    }
                }
                // Upstream: sources referenced by marked target columns.
                let mut add_from = Vec::new();
                for column in marked[to_ix].iter() {
                    for source in sources_toward(from, to, column) {
                        if !marked[from_ix].contains(&source)
                            && from
                                .columns
                                .iter()
                                .any(|from_column| from_column.to_lowercase() == source)
                        {
                            add_from.push(source);
                        }
                    }
                }
                for column in add_to {
                    changed |= marked[to_ix].insert(column);
                }
                for source in add_from {
                    changed |= marked[from_ix].insert(source);
                }
            }
            if !changed || passes > 16 {
                break;
            }
        }
        marked
    }

    fn enter_column_focus(&mut self, column: String, cx: &mut Context<Self>) {
        if self.column_focus.is_none() {
            self.focus_return = Some((self.pan, self.zoom));
        }
        self.selected_column = Some(column.clone());
        self.column_focus = Some(column);
        self.pan = (0., 0.);
        self.zoom = 1.0;
        cx.notify();
    }

    fn exit_column_focus(&mut self, cx: &mut Context<Self>) {
        self.column_focus = None;
        if let Some((pan, zoom)) = self.focus_return.take() {
            self.pan = pan;
            self.zoom = zoom;
        }
        cx.notify();
    }

    /// Right-click menu on a column row: focus its lineage, copy things.
    fn deploy_column_menu(
        &mut self,
        column: String,
        expr: Option<String>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        let focus_column = column.clone();
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            menu.context(self.focus_handle.clone())
                .entry("Focus column lineage", None, {
                    let panel = panel.clone();
                    move |_, cx| {
                        panel
                            .update(cx, |this, cx| {
                                this.enter_column_focus(focus_column.to_lowercase(), cx);
                            })
                            .ok();
                    }
                })
                .entry("Copy column name", None, {
                    let column = column.clone();
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(column.clone()));
                    }
                })
                .when_some(expr, |menu, expr| {
                    menu.entry("Copy expression", None, move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(expr.clone()));
                    })
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

    /// Whether column rows render, given the zoom — always in focus mode.
    fn columns_visible_at(&self, zoom: f32) -> bool {
        self.column_focus.is_some() || (self.show_columns && zoom >= COLUMNS_MIN_ZOOM)
    }

    /// A compact DAG of only the nodes on `column`'s lineage path, each node
    /// showing just its marked column(s) — the per-column focus view.
    fn build_column_focus_layout(
        &self,
        full: &LayoutGraph,
        column: &str,
    ) -> Option<Arc<LayoutGraph>> {
        use crate::lineage::{
            GRAPH_COL_GAP, GRAPH_COLUMN_ROW_HEIGHT, GRAPH_NODE_HEIGHT, GRAPH_PADDING,
            GRAPH_ROW_GAP,
        };
        let marks = Self::column_highlights(full, column);
        let mut kept: Vec<(usize, crate::lineage::GraphLayoutNode)> = Vec::new();
        for (ix, node) in full.nodes.iter().enumerate() {
            if marks[ix].is_empty() {
                continue;
            }
            let mut node = node.clone();
            let mut columns: Vec<String> = node
                .columns
                .iter()
                .filter(|name| marks[ix].contains(&name.to_lowercase()))
                .cloned()
                .collect();
            columns.sort();
            node.columns = columns;
            kept.push((ix, node));
        }
        if kept.is_empty() {
            return None;
        }

        // Restack by level: x from cumulative level widths, y stacked.
        let mut index_map: HashMap<usize, usize> = HashMap::default();
        let mut levels: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
        for (kept_ix, (full_ix, node)) in kept.iter().enumerate() {
            index_map.insert(*full_ix, kept_ix);
            levels.entry(node.level).or_default().push(kept_ix);
        }
        let row_pitch = |node: &crate::lineage::GraphLayoutNode| {
            GRAPH_NODE_HEIGHT
                + 4.
                + node.columns.len() as f32 * GRAPH_COLUMN_ROW_HEIGHT
                + GRAPH_ROW_GAP
        };
        let tallest = levels
            .values()
            .map(|ixs| {
                ixs.iter()
                    .map(|&kept_ix| row_pitch(&kept[kept_ix].1))
                    .sum::<f32>()
            })
            .fold(0.0_f32, f32::max);
        let mut x = GRAPH_PADDING;
        let mut max_height = 0.0_f32;
        for ixs in levels.values() {
            let column_width = ixs
                .iter()
                .map(|&kept_ix| {
                    let node = &kept[kept_ix].1;
                    let longest = node
                        .columns
                        .iter()
                        .map(String::len)
                        .max()
                        .unwrap_or(0)
                        .max(node.name.len());
                    26. + 8. * longest as f32
                })
                .fold(120.0_f32, f32::max);
            let level_height: f32 = ixs
                .iter()
                .map(|&kept_ix| row_pitch(&kept[kept_ix].1))
                .sum();
            let mut y = GRAPH_PADDING + (tallest - level_height) / 2.;
            for &kept_ix in ixs {
                let node = &mut kept[kept_ix].1;
                node.x = x;
                node.y = y;
                node.width = column_width;
                node.height =
                    GRAPH_NODE_HEIGHT + 4. + node.columns.len() as f32 * GRAPH_COLUMN_ROW_HEIGHT;
                y += row_pitch(node);
                max_height = max_height.max(y);
            }
            x += column_width + GRAPH_COL_GAP;
        }
        let edges = full
            .edges
            .iter()
            .filter_map(|(from, to)| Some((*index_map.get(from)?, *index_map.get(to)?)))
            .collect();
        Some(Arc::new(LayoutGraph {
            nodes: kept.into_iter().map(|(_, node)| node).collect(),
            edges,
            width: x - GRAPH_COL_GAP + GRAPH_PADDING,
            height: max_height + GRAPH_PADDING,
        }))
    }

    /// Whether the centered model is currently visible in the viewport.
    fn center_in_view(&self) -> bool {
        let Some(layout) = self.lineage_layout.as_ref() else {
            return true;
        };
        let Some(node) = layout.nodes.iter().find(|node| node.is_center) else {
            return true;
        };
        let viewport = self.canvas_scroll.bounds().size;
        let (view_w, view_h) = (f32::from(viewport.width), f32::from(viewport.height));
        if view_w <= 1. || view_h <= 1. {
            return true;
        }
        let offset = self
            .node_offsets
            .get(&node.name)
            .copied()
            .unwrap_or((0., 0.));
        let x = (node.x + node.width / 2.) * self.zoom + offset.0 + self.pan.0;
        let y = (node.y + node.height / 2.) * self.zoom + offset.1 + self.pan.1;
        x >= 0. && x <= view_w && y >= 0. && y <= view_h
    }

    /// Pans the canvas so the centered (browsed) model sits in the middle of
    /// the viewport — browsing a file always brings its node into view.
    fn center_on_model(&mut self) -> bool {
        let Some(layout) = self.lineage_layout.as_ref() else {
            log::info!("dbt lineage: center skipped — no layout");
            return true;
        };
        let Some(node) = layout.nodes.iter().find(|node| node.is_center) else {
            log::info!(
                "dbt lineage: center skipped — no is_center among {} nodes (model {:?})",
                layout.nodes.len(),
                self.lineage_model,
            );
            return true;
        };
        let viewport = self.canvas_scroll.bounds().size;
        let (view_w, view_h) = (f32::from(viewport.width), f32::from(viewport.height));
        if view_w <= 1. || view_h <= 1. {
            // The canvas hasn't painted yet — retry on a later render.
            return false;
        }
        let offset = self
            .node_offsets
            .get(&node.name)
            .copied()
            .unwrap_or((0., 0.));
        let center_x = (node.x + node.width / 2.) * self.zoom + offset.0;
        let center_y = (node.y + node.height / 2.) * self.zoom + offset.1;
        self.pan = (view_w / 2. - center_x, view_h / 2. - center_y);
        self.canvas_scroll.set_offset(point(px(0.), px(0.)));
        log::debug!(
            "dbt lineage: centered {} at ({center_x:.0},{center_y:.0}) in {view_w:.0}x{view_h:.0} -> pan ({:.0},{:.0})",
            node.name,
            self.pan.0,
            self.pan.1,
        );
        true
    }

    /// Changes zoom anchored on the graph's gravity center (the browsed
    /// node, or the graph middle) so the graph stays in place instead of
    /// sliding toward the canvas origin and off screen.
    fn set_zoom(&mut self, new_zoom: f32, cx: &mut Context<Self>) {
        let new_zoom = new_zoom.clamp(0.4, 2.0);
        if (new_zoom - self.zoom).abs() < f32::EPSILON {
            return;
        }
        if let Some(layout) = self.lineage_layout.as_ref() {
            let anchor = layout
                .nodes
                .iter()
                .find(|node| node.is_center)
                .map(|node| (node.x + node.width / 2., node.y + node.height / 2.))
                .unwrap_or((layout.width / 2., layout.height / 2.));
            self.pan.0 += anchor.0 * (self.zoom - new_zoom);
            self.pan.1 += anchor.1 * (self.zoom - new_zoom);
        }
        self.zoom = new_zoom;
        cx.notify();
    }

    fn render_column_trace(&self, cx: &Context<Self>) -> Option<gpui::Div> {
        let column = self.selected_column.clone()?;
        let layout = self.lineage_layout.as_ref()?;
        // Rename-aware: follow the same propagation as the canvas highlight,
        // so renamed columns appear in the trace under their local names.
        let marks = Self::column_highlights(layout, &column);
        let mut steps: Vec<(i32, String, String, bool)> = layout
            .nodes
            .iter()
            .enumerate()
            .filter(|(ix, _)| !marks[*ix].is_empty())
            .map(|(ix, node)| {
                let local = marks[ix]
                    .iter()
                    .min()
                    .cloned()
                    .unwrap_or_else(|| column.clone());
                let expr = node.col_exprs.get(&local).cloned();
                let is_transform = expr.as_ref().is_some_and(|expr| {
                    let lower = expr.to_lowercase();
                    lower != local && !lower.ends_with(&format!(".{local}"))
                });
                let label = match expr {
                    Some(expr) if is_transform => format!("{local} = {expr}"),
                    Some(_) => format!("{local} · passthrough"),
                    None if node.kind == "source" => format!("{local} · source"),
                    None => local.clone(),
                };
                (node.level, node.name.clone(), label, is_transform)
            })
            .collect();
        if steps.is_empty() {
            return None;
        }
        steps.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let colors = cx.theme().colors();
        let accent = cx.theme().players().local().cursor;
        let total = steps.len();
        Some(
            div()
                .absolute()
                .left_2()
                .bottom_2()
                .max_w(px(560.))
                .max_h(px(220.))
                .overflow_hidden()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .bg(colors.elevated_surface_background)
                .p_2()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            Label::new(format!("Trace · {}", column.to_uppercase()))
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        )
                        .children(steps.into_iter().take(12).map(
                            |(_, name, label, is_transform)| {
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.text)
                                            .child(SharedString::from(name)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(if is_transform {
                                                accent
                                            } else {
                                                colors.text_muted
                                            })
                                            .child(SharedString::from(label)),
                                    )
                            },
                        ))
                        .when(total > 12, |this| {
                            this.child(
                                Label::new(format!("+{} more", total - 12))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                        }),
                ),
        )
    }

    fn render_graph_viewport(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let full_layout = self.lineage_layout.clone()?;
        let mut degrees: HashMap<String, (bool, bool)> = HashMap::new();
        for &(from, to) in &full_layout.edges {
            if let Some(node) = full_layout.nodes.get(from) {
                degrees.entry(node.name.clone()).or_default().1 = true;
            }
            if let Some(node) = full_layout.nodes.get(to) {
                degrees.entry(node.name.clone()).or_default().0 = true;
            }
        }
        let degrees = Arc::new(degrees);
        let layout = match self
            .column_focus
            .as_ref()
            .and_then(|column| self.build_column_focus_layout(&full_layout, column))
        {
            Some(focus_layout) => focus_layout,
            None => self.filtered_layout(&full_layout),
        };
        Some(
            div()
                .id(SharedString::from(id.to_owned()))
                .size_full()
                .overflow_scroll()
                .track_scroll(&self.canvas_scroll)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                        this.graph_drag = Some(GraphDrag::Canvas(event.position));
                    }),
                )
                // cmd + scroll wheel zooms, anchored on the gravity center.
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    if !event.modifiers.platform {
                        return;
                    }
                    let delta_y = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 120.,
                        gpui::ScrollDelta::Lines(delta) => delta.y / 8.,
                    };
                    if delta_y != 0. {
                        this.set_zoom(this.zoom * (1. + delta_y), cx);
                        cx.stop_propagation();
                    }
                }))
                .on_mouse_move(cx.listener(|this, event, _, cx| {
                    this.graph_mouse_move(event, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.graph_end_drag(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.graph_end_drag(cx)),
                )
                .child(self.render_lineage_canvas(layout, degrees, cx))
                .into_any_element(),
        )
    }

    fn materialization_color(materialization: &str, cx: &App) -> gpui::Hsla {
        let index = match materialization {
            "table" => 0,
            "view" => 1,
            "incremental" => 2,
            "ephemeral" => 3,
            "seed" => 4,
            "snapshot" => 5,
            "source" => 6,
            _ => 7,
        };
        cx.theme().accents().color_for_index(index)
    }

    /// Applies the global stream toggles and the per-node collapse state:
    /// a node is visible when reachable from the model without crossing a
    /// collapsed handle. Remaining columns are re-anchored to the left edge.
    fn filtered_layout(&self, layout: &LayoutGraph) -> Arc<LayoutGraph> {
        let node_count = layout.nodes.len();
        let Some(center_ix) = layout.nodes.iter().position(|node| node.is_center) else {
            return Arc::new(layout.clone());
        };
        let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for &(from, to) in &layout.edges {
            outgoing[from].push(to);
            incoming[to].push(from);
        }

        let mut visible = vec![false; node_count];
        visible[center_ix] = true;
        for upstream in [true, false] {
            if upstream && !self.show_upstream || !upstream && !self.show_downstream {
                continue;
            }
            let collapsed = if upstream {
                &self.collapsed_up
            } else {
                &self.collapsed_down
            };
            let mut stack = vec![center_ix];
            while let Some(ix) = stack.pop() {
                if collapsed.contains(layout.nodes[ix].name.as_str()) {
                    continue;
                }
                let links = if upstream { &incoming[ix] } else { &outgoing[ix] };
                for &linked in links {
                    if !visible[linked] {
                        visible[linked] = true;
                        stack.push(linked);
                    }
                }
            }
        }

        log::debug!(
            "dbt lineage: visibility {}/{} nodes (show_up={} show_down={} collapsed_up={:?} collapsed_down={:?})",
            visible.iter().filter(|visible| **visible).count(),
            node_count,
            self.show_upstream,
            self.show_downstream,
            self.collapsed_up,
            self.collapsed_down,
        );
        // Column selection promotes marked-but-hidden columns into the visible
        // window, so a traced path never disappears into "+N more".
        let column_marks = self
            .selected_column
            .as_ref()
            .map(|selected| Self::column_highlights(layout, selected));
        let mut index_map = vec![None; node_count];
        let mut nodes = Vec::new();
        for (ix, node) in layout.nodes.iter().enumerate() {
            if visible[ix] {
                index_map[ix] = Some(nodes.len());
                let mut node = node.clone();
                if let Some(marks) = column_marks.as_ref() {
                    let set = &marks[ix];
                    let hidden_marked = node
                        .columns
                        .iter()
                        .skip(GRAPH_MAX_COLUMNS)
                        .any(|column| set.contains(&column.to_lowercase()));
                    if hidden_marked {
                        let (mut promoted, rest): (Vec<String>, Vec<String>) = node
                            .columns
                            .drain(..)
                            .partition(|column| set.contains(&column.to_lowercase()));
                        promoted.extend(rest);
                        node.columns = promoted;
                    }
                }
                // Semantic zoom: columns collapse away when zoomed out.
                let columns_visible = self.columns_visible_at(self.zoom);
                if columns_visible && !node.columns.is_empty() {
                    let shown = node.columns.len().min(GRAPH_MAX_COLUMNS);
                    let more = usize::from(node.columns.len() > GRAPH_MAX_COLUMNS);
                    node.height =
                        GRAPH_NODE_HEIGHT + 4. + (shown + more) as f32 * GRAPH_COLUMN_ROW_HEIGHT;
                    // Widen the box to fit its longest visible column name.
                    let longest = node
                        .columns
                        .iter()
                        .take(GRAPH_MAX_COLUMNS)
                        .map(|column| column.len())
                        .max()
                        .unwrap_or(0);
                    node.width = node.width.max((28. + 7.2 * longest as f32).min(340.));
                } else {
                    node.height = GRAPH_NODE_HEIGHT;
                }
                nodes.push(node);
            }
        }

        // Re-stack each level vertically (heights vary when columns show).
        let mut groups: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
        for (ix, node) in nodes.iter().enumerate() {
            groups.entry(node.level).or_default().push(ix);
        }
        let mut content_height = 0.0_f32;
        for group in groups.values() {
            let group_height: f32 = group
                .iter()
                .map(|&ix| nodes[ix].height + GRAPH_ROW_GAP)
                .sum::<f32>()
                - GRAPH_ROW_GAP;
            content_height = content_height.max(group_height);
        }
        for group in groups.values() {
            let group_height: f32 = group
                .iter()
                .map(|&ix| nodes[ix].height + GRAPH_ROW_GAP)
                .sum::<f32>()
                - GRAPH_ROW_GAP;
            let mut y = GRAPH_PADDING + (content_height - group_height) / 2.;
            for &ix in group {
                nodes[ix].y = y;
                y += nodes[ix].height + GRAPH_ROW_GAP;
            }
        }

        // Re-space levels horizontally too — widths vary when columns show,
        // and hidden levels compact away.
        let mut x = GRAPH_PADDING;
        for group in groups.values() {
            let level_width = group
                .iter()
                .map(|&ix| nodes[ix].width)
                .fold(80.0_f32, f32::max);
            for &ix in group {
                nodes[ix].x = x;
            }
            x += level_width + GRAPH_COL_GAP;
        }
        let width = x - GRAPH_COL_GAP + GRAPH_PADDING;

        let edges = layout
            .edges
            .iter()
            .filter_map(|&(from, to)| Some((index_map[from]?, index_map[to]?)))
            .collect();
        Arc::new(LayoutGraph {
            nodes,
            edges,
            width,
            height: content_height + 2. * GRAPH_PADDING,
        })
    }

    fn render_lineage_canvas(
        &self,
        layout: Arc<LayoutGraph>,
        degrees: Arc<HashMap<String, (bool, bool)>>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = cx.theme().colors();
        let edge_color = theme.text_muted;
        let node_bg = theme.elevated_surface_background;
        let center_border = theme.text_accent;
        let text_color = theme.text;
        let muted_color = theme.text_muted;
        let name_text_size = px((13. * self.zoom).clamp(8., 22.));
        let column_text_size = px((11. * self.zoom).clamp(7., 18.));

        // Single source of truth: drag offsets are baked into node positions
        // once, and both the edge canvas and the node elements read from the
        // same positioned set — they can never disagree.
        let zoom = self.zoom;
        let header_height = GRAPH_NODE_HEIGHT * zoom;
        let column_row_height = GRAPH_COLUMN_ROW_HEIGHT * zoom;
        let mut positioned = (*layout).clone();
        for node in &mut positioned.nodes {
            // Zoom scales geometry; pan and drag offsets are screen-space.
            node.x = node.x * zoom + self.pan.0;
            node.y = node.y * zoom + self.pan.1;
            node.width *= zoom;
            node.height *= zoom;
            if let Some(&(dx, dy)) = self.node_offsets.get(&node.name) {
                node.x += dx;
                node.y += dy;
            }
        }
        let positioned = Arc::new(positioned);

        let edge_nodes = positioned.clone();
        let show_columns = self.columns_visible_at(self.zoom);
        let selected_column = self.selected_column.clone();
        let column_marks = selected_column
            .as_ref()
            .map(|selected| Arc::new(Self::column_highlights(&layout, selected)));
        let edge_column_marks = column_marks.clone();
        let accent = center_border;
        let mut column_edge_color = edge_color;
        column_edge_color.a *= if selected_column.is_some() { 0.2 } else { 0.45 };

        let edges = canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let draw_curve =
                    |window: &mut Window, start: Point<Pixels>, end: Point<Pixels>, width: f32, color| {
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
                let draw_arrow = |window: &mut Window, end: Point<Pixels>, color| {
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
                    // Edges start flush at the source border (emerging from
                    // behind its handle when present); the arrow end is inset
                    // only where the target actually shows a collapse handle.
                    let end_inset = if to.level <= 0 { 12. } else { 2. };
                    let start = point(
                        bounds.origin.x + px(from.x + from.width),
                        bounds.origin.y + px(from.y + header_height / 2.),
                    );
                    let end = point(
                        bounds.origin.x + px(to.x - end_inset),
                        bounds.origin.y + px(to.y + header_height / 2.),
                    );
                    draw_curve(window, start, end, 1.5, edge_color);
                    draw_arrow(window, end, edge_color);

                    // Column-level lineage: same-named columns on connected
                    // nodes get a thin edge from row to row.
                    if show_columns && !from.columns.is_empty() && !to.columns.is_empty() {
                        let from_rows: HashMap<String, usize> = from
                            .columns
                            .iter()
                            .take(GRAPH_MAX_COLUMNS)
                            .enumerate()
                            .map(|(row, name)| (name.to_lowercase(), row))
                            .collect();
                        for (to_row, to_column) in
                            to.columns.iter().take(GRAPH_MAX_COLUMNS).enumerate()
                        {
                            let to_lower = to_column.to_lowercase();
                            // Exact AST lineage first, heuristic fallback.
                            let sources = sources_toward(from, to, &to_lower);
                            for source in &sources {
                                let Some(&from_row) = from_rows.get(source) else {
                                    continue;
                                };
                                let row_y = |node: &_, row: usize| {
                                    let node: &crate::lineage::GraphLayoutNode = node;
                                    node.y
                                        + header_height
                                        + 2. * zoom
                                        + (row as f32 + 0.5) * column_row_height
                                };
                                let start = point(
                                    bounds.origin.x + px(from.x + from.width),
                                    bounds.origin.y + px(row_y(from, from_row)),
                                );
                                let end = point(
                                    bounds.origin.x + px(to.x),
                                    bounds.origin.y + px(row_y(to, to_row)),
                                );
                                // The selected column's transformation path
                                // lights up in accent with direction arrows —
                                // selecting either endpoint works.
                                let is_selected =
                                    edge_column_marks.as_ref().is_some_and(|marks| {
                                        marks[to_ix].contains(to_lower.as_str())
                                            && marks[from_ix].contains(source.as_str())
                                    });
                                if is_selected {
                                    draw_curve(window, start, end, 2.0, accent);
                                    draw_arrow(window, end, accent);
                                } else {
                                    draw_curve(window, start, end, 1.0, column_edge_color);
                                }
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

        let workspace = self.workspace.clone();
        div()
            .id("dbt-lineage-content")
            .relative()
            .w(px(layout.width * zoom))
            .h(px(layout.height * zoom))
            .child(edges)
            .children(positioned.nodes.clone().into_iter().enumerate().map(|(ix, node)| {
                let path = node.path.clone();
                let workspace = workspace.clone();
                let materialization_color =
                    Self::materialization_color(&node.materialization, cx);
                let shown_columns = if self.columns_visible_at(zoom) {
                    node.columns.len().min(GRAPH_MAX_COLUMNS)
                } else {
                    0
                };
                let more_columns = node.columns.len().saturating_sub(shown_columns);

                div()
                    .id(SharedString::from(format!("dbt-graph-node-{ix}")))
                    .absolute()
                    .left(px(node.x))
                    .top(px(node.y))
                    .w(px(node.width))
                    .h(px(node.height))
                    .rounded_md()
                    .bg(node_bg)
                    .map(|this| {
                        if node.is_center {
                            this.border_2()
                        } else {
                            this.border_1()
                        }
                    })
                    .border_color(materialization_color)
                    // The browsed model glows so it's findable at a glance.
                    .when(node.is_center, |this| {
                        let mut glow = center_border;
                        glow.a = 0.45;
                        this.shadow(vec![gpui::BoxShadow {
                            color: glow,
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(18.),
                            spread_radius: px(4.),
                            inset: false,
                        }])
                    })
                    .hover(|style| style.border_color(center_border))
                    .cursor_pointer()
                    // Transformation summary for debugging: what this model does.
                    .when_some(node.ops.clone(), |this, ops| {
                        this.tooltip(ui::Tooltip::text(ops_tooltip(&ops)))
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
                                    // Split the table name from the column list.
                                    .when(shown_columns > 0, |this| {
                                        this.border_b_1()
                                            .border_color(cx.theme().colors().border)
                                    })
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(name_text_size)
                                            .text_color(if node.is_center {
                                                center_border
                                            } else {
                                                text_color
                                            })
                                            .child(SharedString::from(node.name.clone())),
                                    )
                                    .when_some(
                                        node.ops.as_ref().map(|ops| ops_badges(ops)).filter(|badges| !badges.is_empty()),
                                        |this, badges| {
                                            this.child(
                                                div()
                                                    .text_size(name_text_size * 0.8)
                                                    .text_color(materialization_color)
                                                    .child(SharedString::from(badges)),
                                            )
                                        },
                                    ),
                            )
                            .when(shown_columns > 0, |this| {
                                this.children(
                                    node.columns.iter().take(shown_columns).enumerate().map(
                                        |(row, column)| {
                                            let column_lower = column.to_lowercase();
                                            let column_expr =
                                                node.col_exprs.get(&column_lower).cloned();
                                            let menu_column = column.clone();
                                            let menu_expr = column_expr.clone();
                                            let is_selected =
                                                column_marks.as_ref().is_some_and(|marks| {
                                                    marks.get(ix).is_some_and(|set| {
                                                        set.contains(&column_lower)
                                                    })
                                                });
                                            let mut selected_bg = center_border;
                                            selected_bg.a = 0.16;
                                            div()
                                                .id(SharedString::from(format!(
                                                    "dbt-col-{ix}-{row}"
                                                )))
                                                .w_full()
                                                .h(px(column_row_height))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .when(is_selected, |this| this.bg(selected_bg))
                                                .when_some(column_expr, |this, expr| {
                                                    this.tooltip(ui::Tooltip::text(format!(
                                                        "= {expr}"
                                                    )))
                                                })
                                                .hover(|style| style.bg(selected_bg))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    |_, _, cx| cx.stop_propagation(),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Right,
                                                    cx.listener(
                                                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                                                            this.deploy_column_menu(
                                                                menu_column.clone(),
                                                                menu_expr.clone(),
                                                                event.position,
                                                                window,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        },
                                                    ),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if this.column_focus.is_some() {
                                                        // Inside focus mode a click re-focuses.
                                                        this.enter_column_focus(
                                                            column_lower.clone(),
                                                            cx,
                                                        );
                                                    } else if this.selected_column.as_deref()
                                                        == Some(column_lower.as_str())
                                                    {
                                                        this.selected_column = None;
                                                    } else {
                                                        this.selected_column =
                                                            Some(column_lower.clone());
                                                    }
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div()
                                                        .text_size(column_text_size)
                                                        .text_color(if is_selected {
                                                            center_border
                                                        } else {
                                                            muted_color
                                                        })
                                                        .child(SharedString::from(
                                                            column.clone(),
                                                        )),
                                                )
                                        },
                                    ),
                                )
                                .when(more_columns > 0, |this| {
                                    this.child(
                                        div().w_full().h(px(column_row_height)).px_2().child(
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
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let name = node.name.clone();
                            move |this, event: &gpui::MouseDownEvent, _, cx| {
                                this.drag_moved = false;
                                this.graph_drag =
                                    Some(GraphDrag::Node(name.clone(), event.position));
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if std::mem::take(&mut this.drag_moved) {
                            return;
                        }
                        let Some(path) = path.clone() else {
                            return;
                        };
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace
                                    .open_abs_path(
                                        path,
                                        workspace::OpenOptions {
                                            // Keep keyboard focus in the graph
                                            // while browsing node to node.
                                            focus: Some(false),
                                            ..Default::default()
                                        },
                                        window,
                                        cx,
                                    )
                                    .detach();
                            })
                            .ok();
                    }))
            }))
            // Floating collapse/expand handles: siblings of the nodes, so they
            // never interfere with node dragging or get clipped by node bounds.
            .children(positioned.nodes.iter().enumerate().flat_map(|(ix, node)| {
                let (has_upstream, has_downstream) =
                    degrees.get(&node.name).copied().unwrap_or((false, false));
                let materialization_color =
                    Self::materialization_color(&node.materialization, cx);
                let mut handles = Vec::new();
                let mut push_handle = |suffix: &'static str,
                                       anchor_x: f32,
                                       collapsed: bool,
                                       upstream: bool| {
                    let name = node.name.clone();
                    handles.push(
                        div()
                            .id(SharedString::from(format!("dbt-handle-{suffix}-{ix}")))
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
                            .border_color(materialization_color)
                            .shadow_sm()
                            .cursor_pointer()
                            .hover(|style| style.border_color(center_border))
                            .child(
                                Label::new(if collapsed { "+" } else { "−" })
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            // Handle presses must not start a canvas pan.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let set = if upstream {
                                    &mut this.collapsed_up
                                } else {
                                    &mut this.collapsed_down
                                };
                                if !set.remove(&name) {
                                    set.insert(name.clone());
                                }
                                log::info!(
                                    "dbt lineage: toggled {} collapse for {name}; up={:?} down={:?}",
                                    if upstream { "upstream" } else { "downstream" },
                                    this.collapsed_up,
                                    this.collapsed_down,
                                );
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                };
                // Only the handle pointing away from the model is meaningful:
                // collapsing toward the center can never hide anything.
                if has_upstream && node.level <= 0 {
                    push_handle("up", node.x, self.collapsed_up.contains(&node.name), true);
                }
                if has_downstream && node.level >= 0 {
                    push_handle(
                        "down",
                        node.x + node.width,
                        self.collapsed_down.contains(&node.name),
                        false,
                    );
                }
                // Depth-boundary nodes with unloaded neighbors get a +
                // handle that grows the graph from them.
                let mut push_expand = |suffix: &'static str, anchor_x: f32| {
                    let name = node.name.clone();
                    handles.push(
                        div()
                            .id(SharedString::from(format!("dbt-expand-{suffix}-{ix}")))
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
                            .border_color(center_border)
                            .shadow_sm()
                            .cursor_pointer()
                            .hover(|style| style.border_color(center_border))
                            .child(
                                Label::new("+")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.depth_expansions.insert(name.clone());
                                // Force a lineage recompute for the same model.
                                if let Some(model) = this.lineage_model.take() {
                                    if let Some(root) = this.last_root.clone() {
                                        this.refresh_lineage(model, root, cx);
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                };
                if node.truncated_up && !has_upstream && node.level <= 0 {
                    push_expand("up", node.x);
                }
                if node.truncated_down && !has_downstream && node.level >= 0 {
                    push_expand("down", node.x + node.width);
                }
                handles
            }))
            .into_any_element()
    }

    /// Details card for the centered model: relation, docs, stats, columns.
    fn render_details_sidebar(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let layout = self.lineage_layout.as_ref()?;
        let node = layout.nodes.iter().find(|node| node.is_center)?.clone();
        let details = node.details.clone().unwrap_or(serde_json::Value::Null);
        let get_str = |key: &str| -> Option<String> {
            details
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let tags: Vec<String> = details
            .get("tags")
            .and_then(|tags| tags.as_array())
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| tag.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let column_type = |name: &str| -> Option<String> {
            details
                .get("column_types")?
                .get(name.to_lowercase())
                .and_then(|kind| kind.as_str())
                .map(|kind| kind.to_lowercase())
        };
        let column_doc = |name: &str| -> Option<String> {
            details
                .get("column_descriptions")?
                .get(name.to_lowercase())
                .and_then(|doc| doc.as_str())
                .map(str::to_owned)
        };
        let row_count = details.get("row_count").and_then(|value| value.as_u64());
        let bytes = details.get("bytes").and_then(|value| value.as_u64());
        let materialization_color = Self::materialization_color(&node.materialization, cx);
        let muted = cx.theme().colors().text_muted;

        let mut body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(Label::new(node.name.clone()).size(LabelSize::Small))
                    .child(
                        div()
                            .px_1p5()
                            .rounded_md()
                            .border_1()
                            .border_color(materialization_color)
                            .child(
                                Label::new(node.materialization.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    ),
            );
        if let Some(relation) = get_str("relation") {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(SharedString::from(relation)),
            );
        }
        if let Some(description) = get_str("description") {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().colors().text)
                    .child(SharedString::from(description)),
            );
        }
        if row_count.is_some() || bytes.is_some() {
            let mut stats = Vec::new();
            if let Some(rows) = row_count {
                stats.push(format!("{rows} rows"));
            }
            if let Some(bytes) = bytes {
                stats.push(format!("{:.1} MB", bytes as f64 / 1_048_576.));
            }
            body = body.child(
                Label::new(stats.join(" · "))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );
        }
        if !tags.is_empty() {
            body = body.child(h_flex().gap_1().flex_wrap().children(tags.into_iter().map(
                |tag| {
                    div()
                        .px_1p5()
                        .rounded_md()
                        .bg(cx.theme().colors().element_background)
                        .child(Label::new(tag).size(LabelSize::XSmall).color(Color::Muted))
                },
            )));
        }
        if let Some(ops) = node.ops.as_ref() {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(SharedString::from(ops_tooltip(ops))),
            );
        }
        body = body.child(
            Label::new(format!("Columns ({})", node.columns.len()))
                .size(LabelSize::XSmall)
                .color(Color::Accent),
        );
        for (ix, column) in node.columns.iter().enumerate() {
            let doc = column_doc(column);
            let mut row = h_flex()
                .id(SharedString::from(format!("dbt-detail-col-{ix}")))
                .w_full()
                .gap_2()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(cx.theme().colors().text)
                        .child(SharedString::from(column.clone())),
                );
            if let Some(kind) = column_type(column) {
                row = row.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(muted)
                        .child(SharedString::from(kind)),
                );
            }
            if let Some(doc) = doc {
                row = row.tooltip(ui::Tooltip::text(doc));
            }
            body = body.child(row);
        }

        Some(
            div()
                .id("dbt-lineage-details")
                .w(px(300.))
                .h_full()
                .flex_none()
                .p_2()
                .border_l_1()
                .border_color(cx.theme().colors().border)
                .overflow_y_scroll()
                .child(body)
                .into_any_element(),
        )
    }

    fn render_tree_rows(
        &self,
        id_prefix: &'static str,
        nodes: &[LineageTreeNode],
        depth: usize,
        rows: &mut Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for tree_node in nodes {
            let key: SharedString =
                format!("{id_prefix}:{depth}:{}", tree_node.node.name).into();
            let is_expanded = self.expanded.contains(&key);
            let expandable = !tree_node.children.is_empty();

            let workspace = self.workspace.clone();
            let path = tree_node.node.path.clone();
            let row = h_flex()
                .pl(px(depth as f32 * 16.))
                .gap_1()
                .when(expandable, |this| {
                    this.child(
                        IconButton::new(
                            key.clone(),
                            if is_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                        )
                        .icon_size(IconSize::Small)
                        .on_click(cx.listener({
                            let key = key.clone();
                            move |this, _, _, cx| {
                                if !this.expanded.remove(&key) {
                                    this.expanded.insert(key.clone());
                                }
                                cx.notify();
                            }
                        })),
                    )
                })
                .when(!expandable, |this| this.child(div().w(px(22.))))
                .child(
                    Button::new(
                        SharedString::from(format!("open:{key}")),
                        format!(
                            "{} ({}){}",
                            tree_node.node.name,
                            tree_node.node.kind,
                            if tree_node.truncated { " …" } else { "" }
                        ),
                    )
                    .on_click(move |_, window, cx| {
                        let Some(path) = path.clone() else {
                            return;
                        };
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace
                                    .open_abs_path(
                                        path,
                                        workspace::OpenOptions {
                                            // Keep keyboard focus in the graph
                                            // while browsing node to node.
                                            focus: Some(false),
                                            ..Default::default()
                                        },
                                        window,
                                        cx,
                                    )
                                    .detach();
                            })
                            .ok();
                    }),
                );
            rows.push(row.into_any_element());

            if is_expanded {
                self.render_tree_rows(id_prefix, &tree_node.children, depth + 1, rows, cx);
            }
        }
    }

    fn render_tree_section(
        &self,
        id_prefix: &'static str,
        title: &'static str,
        nodes: &[LineageTreeNode],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let section_key: SharedString = format!("{id_prefix}:section").into();
        let collapsed = self.expanded.contains(&section_key);
        let mut rows = Vec::new();
        if !collapsed {
            self.render_tree_rows(id_prefix, nodes, 0, &mut rows, cx);
        }
        v_flex()
            .flex_1()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        IconButton::new(
                            section_key.clone(),
                            if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                        )
                        .icon_size(IconSize::Small)
                        .on_click(cx.listener({
                            let key = section_key.clone();
                            move |this, _, _, cx| {
                                if !this.expanded.remove(&key) {
                                    this.expanded.insert(key.clone());
                                }
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        Label::new(format!("{title} ({})", nodes.len()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .when(!collapsed && nodes.is_empty(), |this| {
                this.child(Label::new("—").color(Color::Muted).size(LabelSize::Small))
            })
            .children(rows)
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.view == ResultsView::Lineage {
            let lineage_tree = self.lineage_tree.clone();
            return match lineage_tree {
                Some(tree) => h_flex()
                    .size_full()
                    .items_start()
                    .when_some(self.lineage_layout.clone(), |this, full_layout| {
                        let layout = self.filtered_layout(&full_layout);
                        let mut legend: Vec<SharedString> = Vec::new();
                        for node in &layout.nodes {
                            let materialization: SharedString =
                                node.materialization.clone().into();
                            if !legend.contains(&materialization) {
                                legend.push(materialization);
                            }
                        }
                        this.child(
                            v_flex()
                                .flex_1()
                                .h_full()
                                // Allow shrinking below intrinsic content width
                                // so the tree sidebar stays on screen and the
                                // legend wraps instead of clipping.
                                .min_w(px(0.))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_1()
                                        .gap_2()
                                        .flex_wrap()
                                        .border_b_1()
                                        .border_color(cx.theme().colors().border)
                                        .child(
                                            Button::new("dbt-toggle-up", "Upstream")
                                                .toggle_state(self.show_upstream)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_upstream = !this.show_upstream;
                                                    log::info!(
                                                        "dbt lineage: global upstream -> {}",
                                                        this.show_upstream
                                                    );
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("dbt-toggle-down", "Downstream")
                                                .toggle_state(self.show_downstream)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_downstream =
                                                        !this.show_downstream;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("dbt-toggle-columns", "Columns")
                                                .toggle_state(self.show_columns)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_columns = !this.show_columns;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("dbt-arrange", "Arrange")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.node_offsets.clear();
                                                    this.pan = (0., 0.);
                                                    this.zoom = 1.0;
                                                    this.graph_drag = None;
                                                    this.depth_expansions.clear();
                                                    this.lineage_model = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("dbt-zoom-out", "−").on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_zoom(this.zoom / 1.2, cx);
                                                }),
                                            ),
                                        )
                                        .child(
                                            Label::new(format!(
                                                "{}%",
                                                (self.zoom * 100.).round() as i32
                                            ))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                        )
                                        .child(
                                            Button::new("dbt-zoom-in", "+").on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_zoom(this.zoom * 1.2, cx);
                                                }),
                                            ),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            IconButton::new(
                                                "dbt-toggle-tree",
                                                if self.show_tree {
                                                    IconName::ThreadsSidebarRightOpen
                                                } else {
                                                    IconName::ThreadsSidebarRightClosed
                                                },
                                            )
                                            .icon_size(IconSize::Small)
                                            .tooltip(ui::Tooltip::text(
                                                "Show/hide the lineage tree",
                                            ))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.show_tree = !this.show_tree;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            IconButton::new(
                                                "dbt-toggle-details",
                                                IconName::Info,
                                            )
                                            .icon_size(IconSize::Small)
                                            .toggle_state(self.show_details)
                                            .tooltip(ui::Tooltip::text(
                                                "Show/hide model details",
                                            ))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.show_details = !this.show_details;
                                                cx.notify();
                                            })),
                                        )
                                        .children(legend.into_iter().map(|materialization| {
                                            let color = Self::materialization_color(
                                                &materialization,
                                                cx,
                                            );
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .w_2()
                                                        .h_2()
                                                        .rounded_full()
                                                        .bg(color),
                                                )
                                                .child(
                                                    Label::new(materialization)
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted),
                                                )
                                        })),
                                )
                                .when_some(self.column_focus.clone(), |this, column| {
                                    this.child(
                                        h_flex()
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .gap_2()
                                            .items_center()
                                            .border_b_1()
                                            .border_color(cx.theme().colors().border)
                                            .bg(cx.theme().colors().elevated_surface_background)
                                            .child(
                                                Label::new(format!(
                                                    "⌖ Column focus: {}",
                                                    column.to_uppercase()
                                                ))
                                                .size(LabelSize::Small)
                                                .color(Color::Accent),
                                            )
                                            .child(
                                                Button::new("dbt-focus-back", "Back to graph")
                                                    .label_size(LabelSize::Small)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.exit_column_focus(cx);
                                                    })),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .relative()
                                        .flex_1()
                                        .w_full()
                                        .children(
                                            self.render_graph_viewport(
                                                "dbt-lineage-canvas",
                                                cx,
                                            ),
                                        )
                                        .when_some(
                                            self.render_column_trace(cx),
                                            |this, trace| this.child(trace),
                                        ),
                                ),
                        )
                    })
                    .when(self.show_details, |this| {
                        this.children(self.render_details_sidebar(cx))
                    })
                    .when(self.show_tree, |this| {
                        this.child(
                            div()
                                .id("dbt-lineage-tree")
                                .w(px(320.))
                                .h_full()
                                .flex_none()
                                .p_2()
                                .border_l_1()
                                .border_color(cx.theme().colors().border)
                                .overflow_y_scroll()
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .child(self.render_tree_section(
                                            "dbt-lineage-up",
                                            "Upstream",
                                            &tree.up,
                                            cx,
                                        ))
                                        .child(self.render_tree_section(
                                            "dbt-lineage-down",
                                            "Downstream",
                                            &tree.down,
                                            cx,
                                        )),
                                ),
                        )
                    })
                    .into_any_element(),
                None => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        Label::new(
                            "Run a model (cmd-enter) to see its lineage — requires target/manifest.json (dbt parse)",
                        )
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            };
        }

        if self.view == ResultsView::Compiled {
            if let Some(editor) = &self.compiled_editor {
                return div().size_full().child(editor.clone()).into_any_element();
            }
            let text: SharedString = match &self.state {
                ResultsState::Loaded { compiled: None, .. } => {
                    "No compiled SQL was captured for this run.".into()
                }
                _ => "Run a model or selection first (cmd-enter).".into(),
            };
            return div()
                .id("dbt-compiled-sql")
                .size_full()
                .p_2()
                .overflow_y_scroll()
                .child(Label::new(text).size(LabelSize::Small).buffer_font(cx))
                .into_any_element();
        }

        match &self.state {
            ResultsState::Empty => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    Label::new("Open a dbt model and press cmd-enter to show its data")
                        .color(Color::Muted),
                )
                .into_any_element(),
            ResultsState::Running { model } => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    Label::new(format!("Running dbt show --select {model}…")).color(Color::Muted),
                )
                .into_any_element(),
            ResultsState::Failed { message, .. } => v_flex()
                .size_full()
                .p_2()
                .overflow_hidden()
                .child(Label::new(message.clone()).color(Color::Error))
                .into_any_element(),
            ResultsState::Loaded { columns, rows, .. } => {
                if columns.is_empty() {
                    return v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(Label::new("Query returned no rows").color(Color::Muted))
                        .into_any_element();
                }
                let indices = Arc::new(self.display_indices(rows, cx));
                let rows = rows.clone();
                let display_rows = indices.clone();
                let mut headers =
                    vec![Label::new("#").color(Color::Muted).into_any_element()];
                headers.extend(columns.iter().enumerate().map(|(ix, column)| {
                        let indicator = match self.sort {
                            Some((sorted, true)) if sorted == ix => " ▲",
                            Some((sorted, false)) if sorted == ix => " ▼",
                            _ => "",
                        };
                        // Only the label is clickable, so the resize handles
                        // on the column boundaries stay easy to grab.
                        h_flex()
                            .child(
                                div()
                                    .id(SharedString::from(format!("dbt-sort-{ix}")))
                                    .cursor_pointer()
                                    .child(Label::new(format!("{column}{indicator}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.sort = match this.sort {
                                            Some((sorted, true)) if sorted == ix => {
                                                Some((ix, false))
                                            }
                                            Some((sorted, false)) if sorted == ix => None,
                                            _ => Some((ix, true)),
                                        };
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    }));
                let column_mask: Vec<bool> = std::iter::once(false)
                    .chain(
                        columns
                            .iter()
                            .enumerate()
                            .map(|(ix, _)| self.hidden_columns.contains(&ix)),
                    )
                    .collect();
                let table = div()
                    .flex_1()
                    .h_full()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(
                        Table::new(columns.len() + 1)
                            .striped()
                            .pin_cols(1)
                            .column_filter(ui::table_row::TableRow::from_vec(
                                column_mask,
                                columns.len() + 1,
                            ))
                            .interactable(&self.table_interaction)
                            .map(|table| match &self.column_widths {
                                Some(state)
                                    if state.read(cx).cols() == columns.len() + 1 =>
                                {
                                    table.width_config(ColumnWidthConfig::Resizable(
                                        state.clone(),
                                    ))
                                }
                                _ => table,
                            })
                            .header(headers)
                            .uniform_list(
                                "dbt-results-rows",
                                display_rows.len(),
                                move |range, _, _| {
                                    range
                                        .filter_map(|display_ix| {
                                            let original = *display_rows.get(display_ix)?;
                                            let row = rows.get(original)?;
                                            let mut cells: Vec<gpui::AnyElement> =
                                                Vec::with_capacity(row.len() + 1);
                                            cells.push(
                                                Label::new(format!("{}", original + 1))
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted)
                                                    .into_any_element(),
                                            );
                                            cells.extend(row.iter().map(|cell| {
                                                Label::new(cell.clone())
                                                    .size(LabelSize::Small)
                                                    .into_any_element()
                                            }));
                                            Some(cells)
                                        })
                                        .collect()
                                },
                            ),
                    );
                h_flex()
                    .size_full()
                    .child(table)
                    .when(self.show_column_picker, |this| {
                        this.child(
                            div()
                                .id("dbt-column-picker")
                                .w(px(220.))
                                .h_full()
                                .flex_none()
                                .p_2()
                                .border_l_1()
                                .border_color(cx.theme().colors().border)
                                .overflow_y_scroll()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Label::new("Show columns")
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                        .children(columns.iter().enumerate().map(
                                            |(ix, column)| {
                                                let hidden =
                                                    self.hidden_columns.contains(&ix);
                                                h_flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        ui::Checkbox::new(
                                                            ("dbt-col-visible", ix),
                                                            if hidden {
                                                                ui::ToggleState::Unselected
                                                            } else {
                                                                ui::ToggleState::Selected
                                                            },
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                if !this
                                                                    .hidden_columns
                                                                    .remove(&ix)
                                                                {
                                                                    this.hidden_columns
                                                                        .insert(ix);
                                                                }
                                                                cx.notify();
                                                            },
                                                        )),
                                                    )
                                                    .child(
                                                        Label::new(column.clone())
                                                            .size(LabelSize::Small),
                                                    )
                                            },
                                        )),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }
}

/// Resolves the profiles directory: the explicit setting when present,
/// otherwise auto-detected in-project profile locations (`local_profiles/`,
/// `profiles/`, `.dbt/`). A root-level profiles.yml needs nothing — dbt finds
/// it via the working directory.
fn resolve_profiles_dir(
    settings: &DbtSettings,
    root: &std::path::Path,
) -> Option<PathBuf> {
    if let Some(dir) = &settings.profiles_dir {
        let path = if std::path::Path::new(dir).is_absolute() {
            PathBuf::from(dir)
        } else {
            root.join(dir)
        };
        return Some(path);
    }
    ["local_profiles", "profiles", ".dbt"]
        .iter()
        .map(|candidate| root.join(candidate))
        .find(|dir| dir.join("profiles.yml").is_file())
}

/// Loads KEY=VALUE pairs from the project's `.env` / `.env.local` (dotenv
/// style: `export` prefixes, quoted values, `#` comments). Values are only
/// ever placed into the spawned dbt process environment — never logged or
/// persisted — and variables already exported in the real environment are
/// left untouched.
fn merge_env_file(path: &std::path::Path, vars: &mut Vec<(String, String)>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if std::env::var_os(key).is_some() {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        vars.retain(|(existing, _)| existing != key);
        vars.push((key.to_owned(), value.to_owned()));
    }
}

fn load_dotenv(root: &std::path::Path, env_file: Option<&str>) -> Vec<(String, String)> {
    // Search the project root and its ancestors up to the git repo root (a
    // dbt project often lives in a subdirectory of the repo, with .env at the
    // top). Never walk past .git — outer files load first so inner override.
    let mut dirs = vec![root.to_path_buf()];
    let mut cursor = root.to_path_buf();
    let mut found_repo = root.join(".git").exists();
    for _ in 0..5 {
        if found_repo {
            break;
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent.to_path_buf();
        dirs.push(cursor.clone());
        found_repo = cursor.join(".git").exists();
    }
    if !found_repo {
        dirs.truncate(1);
    }
    dirs.reverse();

    let mut vars: Vec<(String, String)> = Vec::new();
    for dir in &dirs {
        merge_env_file(&dir.join(".env"), &mut vars);
        merge_env_file(&dir.join(".env.local"), &mut vars);
    }
    // An explicitly configured env file loads last (overriding discovery).
    if let Some(env_file) = env_file {
        let path = if std::path::Path::new(env_file).is_absolute() {
            PathBuf::from(env_file)
        } else {
            root.join(env_file)
        };
        if path.is_file() {
            merge_env_file(&path, &mut vars);
        } else {
            log::warn!("dbt: configured env_file not found: {}", path.display());
        }
    }
    vars
}

/// Applies the settings-driven target, profiles dir, and environment to a dbt
/// invocation.
pub(crate) fn apply_common_args(
    command: &mut util::command::Command,
    settings: &DbtSettings,
    root: &std::path::Path,
) {
    if let Some(dbt_target) = &settings.target {
        command.args(["--target", dbt_target]);
    }
    if let Some(profiles_dir) = resolve_profiles_dir(settings, root) {
        command.arg("--profiles-dir");
        command.arg(profiles_dir);
    }
    let dotenv = load_dotenv(root, settings.env_file.as_deref());
    if !dotenv.is_empty() {
        // Names only would still leak project structure; log just the count.
        log::info!("dbt: loaded {} variable(s) from project .env", dotenv.len());
    }
    command.envs(dotenv);
    // Explicit settings override .env.
    command.envs(settings.env.iter().map(|(key, value)| (key.clone(), value.clone())));
}

/// Runs `dbt compile` for the same target and returns the compiled SQL:
/// for models, read from `target/compiled/<project>/<rel_path>`; for inline
/// queries, parsed from stdout.
async fn fetch_compiled_sql(
    binary: &str,
    settings: &DbtSettings,
    target: &ShowTarget,
    root: &std::path::Path,
) -> Option<String> {
    let mut command = new_command(binary);
    command.arg("compile");
    match target {
        ShowTarget::Model { name, .. } => {
            command.args(["--select", name.as_ref()]);
        }
        ShowTarget::Inline(sql) => {
            command.args(["--inline", sql]);
        }
    }
    apply_common_args(&mut command, settings, root);
    let output = command.current_dir(root).output().await.ok()?;

    match target {
        ShowTarget::Model { rel_path, .. } => {
            let compiled_root = root.join("target").join("compiled");
            for entry in std::fs::read_dir(&compiled_root).ok()?.flatten() {
                let candidate = entry.path().join(rel_path);
                if let Ok(text) = std::fs::read_to_string(&candidate) {
                    return Some(text.trim().to_owned());
                }
            }
            None
        }
        ShowTarget::Inline(_) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut collected = Vec::new();
            let mut in_sql = false;
            for line in stdout.lines() {
                if in_sql {
                    if line.starts_with("New version")
                        || line.starts_with("====")
                        || line.starts_with("Finished")
                    {
                        break;
                    }
                    collected.push(line);
                } else if line.trim() == "Compiled inline node is:" {
                    in_sql = true;
                }
            }
            let compiled = collected.join("\n").trim().to_owned();
            (!compiled.is_empty()).then_some(compiled)
        }
    }
}

pub(crate) fn parse_show_output(
    stdout: &[u8],
    stderr: &[u8],
    success: bool,
) -> Result<(Vec<SharedString>, Vec<Vec<SharedString>>)> {
    let stdout_str = String::from_utf8_lossy(stdout);
    for line in stdout_str.lines() {
        let trimmed = line.trim();
        // Fusion prints a bare JSON array; dbt Core prints {"show": [...]}.
        let json_rows = if trimmed.starts_with('[') {
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(serde_json::Value::Array(rows)) => rows,
                _ => continue,
            }
        } else if trimmed.starts_with('{') {
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(serde_json::Value::Object(mut object)) => {
                    match object.remove("show") {
                        Some(serde_json::Value::Array(rows)) => rows,
                        _ => continue,
                    }
                }
                _ => continue,
            }
        } else {
            continue;
        };
        let columns: Vec<SharedString> = json_rows
            .first()
            .and_then(|row| row.as_object())
            .map(|object| object.keys().map(|key| SharedString::from(key.clone())).collect())
            .unwrap_or_default();
        let rows = json_rows
            .iter()
            .filter_map(|row| row.as_object())
            .map(|object| {
                columns
                    .iter()
                    .map(|column| match object.get(column.as_ref()) {
                        None | Some(serde_json::Value::Null) => SharedString::default(),
                        Some(serde_json::Value::String(value)) => {
                            SharedString::from(value.clone())
                        }
                        Some(other) => SharedString::from(other.to_string()),
                    })
                    .collect()
            })
            .collect();
        return Ok((columns, rows));
    }

    let stderr_str = String::from_utf8_lossy(stderr);
    if success {
        if let Some(reason) = stderr_str
            .lines()
            .find(|line| line.contains("does not match any enabled nodes"))
        {
            anyhow::bail!(
                "dbt selected no models — {}\n\nIs this file inside the dbt project's \
                 model paths, and does the project parse cleanly?",
                reason.trim()
            );
        }
        anyhow::bail!("no result rows found in dbt show output:\n{stdout_str}\n{stderr_str}");
    }
    anyhow::bail!("dbt show failed:\n{stdout_str}\n{stderr_str}");
}

impl EventEmitter<PanelEvent> for DbtResultsPanel {}

impl Focusable for DbtResultsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DbtResultsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_center {
            if self.center_on_model() {
                self.pending_center = false;
                self.center_retry_frames = 0;
            } else if self.view == ResultsView::Lineage {
                // Canvas visible but not measured yet: nudge one more frame,
                // with a hard cap so this can never become a notify loop
                // (that exact loop once froze the app when the canvas was
                // hidden behind another tab).
                self.center_retry_frames += 1;
                if self.center_retry_frames <= 10 {
                    let entity = cx.entity_id();
                    window.on_next_frame(move |_, cx| cx.notify(entity));
                } else {
                    self.pending_center = false;
                    self.center_retry_frames = 0;
                }
            }
            // Hidden canvas: keep the flag armed silently — the next render
            // with the lineage view visible applies it.
        }
        v_flex()
            .key_context("DbtResultsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(self.render_toolbar(cx))
            .child(self.render_body(cx))
            .children(self.context_menu.as_ref().map(|(menu, position, _)| {
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

impl Panel for DbtResultsPanel {
    fn persistent_name() -> &'static str {
        "dbt Results Panel"
    }

    fn panel_key() -> &'static str {
        "DbtResultsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(280.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Table)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("dbt Results Panel")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}
