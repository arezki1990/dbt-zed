//! dbt lineage, backed by a sqlitegraph database built from
//! `target/manifest.json`. The graph is rebuilt whenever the manifest's mtime
//! changes and persists at `target/zed-dbt-lineage.db`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use layout::adt::dag::NodeHandle;
use layout::backends::svg::SVGWriter;
use layout::core::base::Orientation;
use layout::core::geometry::Point;
use layout::core::style::StyleAttr;
use layout::std_shapes::shapes::{Arrow, Element, ShapeKind};
use layout::topo::layout::VisualGraph;
use serde_json::json;
use sqlitegraph::GraphQuery;
use sqlitegraph::graph::{GraphEdge, GraphEntity, SqliteGraph};

#[derive(Clone, Debug)]
pub struct ModelNode {
    pub name: String,
    pub kind: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Lineage {
    pub parents: Vec<ModelNode>,
    pub children: Vec<ModelNode>,
}

/// A node in the deep lineage tree; `children` continue in the same direction
/// (a parent's parents when walking upstream, a child's children downstream).
#[derive(Clone, Debug)]
pub struct LineageTreeNode {
    pub node: ModelNode,
    pub children: Vec<LineageTreeNode>,
    /// The traversal depth/size budget cut this branch short.
    pub truncated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LineageTree {
    pub up: Vec<LineageTreeNode>,
    pub down: Vec<LineageTreeNode>,
}

pub const GRAPH_NODE_HEIGHT: f32 = 32.;
pub const GRAPH_PADDING: f32 = 24.;
pub const GRAPH_ROW_GAP: f32 = 14.;
pub const GRAPH_COLUMN_ROW_HEIGHT: f32 = 16.;
pub const GRAPH_MAX_COLUMNS: usize = 12;
pub const GRAPH_COL_GAP: f32 = 70.;

/// A positioned node in the interactive lineage graph.
/// Static summary of the SQL operations a model applies, extracted from its
/// compiled code. Rendered as badges and a tooltip on the lineage canvas so
/// grain changes (joins, aggregations, filters) are visible while debugging.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeOps {
    /// Join descriptions, e.g. "left join stg_payments".
    pub joins: Vec<String>,
    /// Aggregate functions used, e.g. "sum", "count".
    pub aggregations: Vec<String>,
    pub group_by: bool,
    /// Filter clauses present: "where", "having", "qualify".
    pub filters: Vec<String>,
    /// Window functions (OVER) present.
    pub windows: bool,
    /// SELECT DISTINCT present.
    pub distinct: bool,
    /// Number of UNION branches beyond the first.
    pub unions: usize,
}

impl NodeOps {
    pub fn is_empty(&self) -> bool {
        self.joins.is_empty()
            && self.aggregations.is_empty()
            && !self.group_by
            && self.filters.is_empty()
            && !self.windows
            && !self.distinct
            && self.unions == 0
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "joins": self.joins,
            "aggregations": self.aggregations,
            "group_by": self.group_by,
            "filters": self.filters,
            "windows": self.windows,
            "distinct": self.distinct,
            "unions": self.unions,
        })
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        let strings = |key: &str| -> Vec<String> {
            value
                .get(key)
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let flag = |key: &str| value.get(key).and_then(|value| value.as_bool()).unwrap_or(false);
        let ops = Self {
            joins: strings("joins"),
            aggregations: strings("aggregations"),
            group_by: flag("group_by"),
            filters: strings("filters"),
            windows: flag("windows"),
            distinct: flag("distinct"),
            unions: value.get("unions").and_then(|value| value.as_u64()).unwrap_or(0) as usize,
        };
        (!ops.is_empty()).then_some(ops)
    }
}

/// Identifiers a select expression references (last dot-segment, lowercased),
/// excluding SQL keywords and common functions.
pub(crate) fn expr_column_refs(expr: &str) -> Vec<String> {
    const SKIP: &[&str] = &[
        "sum", "count", "avg", "min", "max", "cast", "coalesce", "case", "when", "then",
        "else", "end", "as", "and", "or", "not", "null", "true", "false", "over",
        "partition", "by", "order", "asc", "desc", "row_number", "rank", "dense_rank",
        "lag", "lead", "nullif", "concat", "trim", "upper", "lower", "substring",
        "substr", "round", "floor", "ceil", "abs", "date", "timestamp", "interval",
        "extract", "from", "distinct", "int", "integer", "bigint", "varchar", "string",
        "numeric", "decimal", "float", "boolean", "char", "text", "iff", "ifnull",
        "listagg", "array_agg", "to_char", "to_date", "to_number", "try_cast", "left",
        "right", "replace", "split_part", "len", "length", "greatest", "least",
        "any_value", "first_value", "last_value", "current_date", "current_timestamp",
        "year", "month", "week", "day", "dateadd", "datediff", "date_trunc", "md5",
        "like", "in", "is", "between", "exists", "union", "all", "select",
    ];
    let lower = expr.to_lowercase();
    let mut refs = Vec::new();
    for token in lower.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.')) {
        if token.is_empty() {
            continue;
        }
        let ident = token.rsplit('.').next().unwrap_or(token);
        if ident.is_empty()
            || ident.chars().next().is_some_and(|ch| ch.is_ascii_digit())
            || SKIP.contains(&ident)
        {
            continue;
        }
        if !refs.iter().any(|existing| existing == ident) {
            refs.push(ident.to_owned());
        }
    }
    refs
}

/// Best-effort text scan of compiled SQL — a debugging aid, not a parser.
pub(crate) fn extract_ops(sql: &str) -> NodeOps {
    let lower = sql.to_lowercase();
    // Whitespace-normalized form so multi-word keywords match across newlines.
    let norm = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    let is_ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut ops = NodeOps::default();

    let mut from = 0;
    while let Some(found) = norm[from..].find(" join ") {
        let at = from + found;
        from = at + 6;
        let before = norm[..at].trim_end();
        let mut kind = "join";
        for candidate in [
            "left outer", "right outer", "full outer", "left", "right", "full", "inner", "cross",
        ] {
            if before.ends_with(candidate) {
                kind = candidate;
                break;
            }
        }
        let rest = &norm[at + 6..];
        let token: String = rest
            .chars()
            .take_while(|ch| is_ident(*ch) || *ch == '.' || *ch == '"')
            .collect();
        let target = token
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim_matches('"')
            .to_owned();
        let label = if target.is_empty() {
            format!("{kind} join (subquery)")
        } else if kind == "join" {
            format!("join {target}")
        } else {
            format!("{kind} join {target}")
        };
        if ops.joins.len() < 8 && !ops.joins.contains(&label) {
            ops.joins.push(label);
        }
    }

    for name in ["sum", "count", "avg", "min", "max", "array_agg", "listagg", "string_agg"] {
        let pattern = format!("{name}(");
        let mut from = 0;
        while let Some(found) = lower[from..].find(&pattern) {
            let at = from + found;
            from = at + pattern.len();
            if lower[..at].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            if !ops.aggregations.contains(&name.to_owned()) {
                ops.aggregations.push(name.to_owned());
            }
            break;
        }
    }
    ops.group_by = norm.contains(" group by ");
    for clause in ["where", "having", "qualify"] {
        if norm.contains(&format!(" {clause} ")) {
            ops.filters.push(clause.to_owned());
        }
    }
    ops.windows = norm.contains(" over (") || norm.contains(" over(") || lower.contains("over(");
    ops.distinct = norm.contains("select distinct ") || norm.contains("select distinct\n");
    ops.unions = norm.matches(" union ").count() + norm.matches(" union all ").count() / 2;
    ops
}

#[derive(Clone, Debug)]
pub struct GraphLayoutNode {
    pub name: String,
    pub kind: String,
    pub materialization: String,
    pub path: Option<PathBuf>,
    /// Summary of SQL operations this model applies, when derivable.
    pub ops: Option<NodeOps>,
    /// Lowercased column name -> the select-list expression producing it.
    pub col_exprs: std::collections::HashMap<String, String>,
    /// Lowercased column name -> upstream/CTE identifiers its full
    /// (untruncated) expression references.
    pub col_refs: std::collections::HashMap<String, Vec<String>>,
    /// Exact AST-resolved lineage: column -> (upstream node name, column).
    pub col_lineage: std::collections::HashMap<String, Vec<(String, String)>>,
    /// More parents/children exist beyond the loaded depth or node cap.
    pub truncated_up: bool,
    pub truncated_down: bool,
    /// Column names, ordered (from catalog.json when present, else the
    /// documented columns in manifest.json).
    pub columns: Vec<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Column relative to the model: negative upstream, 0 center, positive
    /// downstream.
    pub level: i32,
    pub is_center: bool,
}

/// A layered, positioned lineage graph (left-to-right): upstream columns,
/// the model, downstream columns.
#[derive(Clone, Debug, Default)]
pub struct LayoutGraph {
    pub nodes: Vec<GraphLayoutNode>,
    /// Edges as (from, to) indices into `nodes`, pointing downstream.
    pub edges: Vec<(usize, usize)>,
    pub width: f32,
    pub height: f32,
}

#[derive(Default)]
pub struct LineageStore {
    inner: Mutex<Option<Loaded>>,
}

struct Loaded {
    graph: SqliteGraph,
    manifest_mtime: SystemTime,
    catalog_mtime: Option<SystemTime>,
    by_name: HashMap<String, i64>,
}

fn catalog_mtime(project_root: &Path) -> Option<SystemTime> {
    std::fs::metadata(project_root.join("target").join("catalog.json"))
        .and_then(|metadata| metadata.modified())
        .ok()
}

impl LineageStore {
    fn with_loaded<R>(
        &self,
        project_root: &Path,
        f: impl FnOnce(&Loaded) -> Result<R>,
    ) -> Result<R> {
        let manifest_path = project_root.join("target").join("manifest.json");
        let manifest_mtime = std::fs::metadata(&manifest_path)
            .and_then(|metadata| metadata.modified())
            .context("no target/manifest.json — run `dbt parse` first")?;

        let current_catalog_mtime = catalog_mtime(project_root);
        let mut guard = self.inner.lock().unwrap();
        let needs_rebuild = match &*guard {
            Some(loaded) => {
                loaded.manifest_mtime != manifest_mtime
                    || loaded.catalog_mtime != current_catalog_mtime
            }
            None => true,
        };
        if needs_rebuild {
            *guard = Some(build_graph(project_root, &manifest_path, manifest_mtime)?);
        }
        f(guard.as_ref().unwrap())
    }

    fn id_for(loaded: &Loaded, model: &str) -> Result<i64> {
        loaded.by_name.get(model).copied().with_context(|| {
            format!("{model} not found in manifest.json — run `dbt parse` to refresh it")
        })
    }

    /// Returns the direct upstream and downstream nodes of `model`, rebuilding
    /// the graph if `target/manifest.json` changed since the last build.
    pub fn lineage_for(&self, project_root: &Path, model: &str) -> Result<Lineage> {
        self.with_loaded(project_root, |loaded| {
            let id = Self::id_for(loaded, model)?;
            let query = GraphQuery::new(&loaded.graph);
            let resolve = |ids: Vec<i64>| -> Vec<ModelNode> {
                let mut nodes: Vec<ModelNode> = ids
                    .into_iter()
                    .filter_map(|id| loaded.graph.get_entity(id).ok())
                    .map(|entity| entity_to_node(project_root, entity))
                    .collect();
                nodes.sort_by(|a, b| a.name.cmp(&b.name));
                nodes
            };
            Ok(Lineage {
                parents: resolve(query.incoming(id)?),
                children: resolve(query.outgoing(id)?),
            })
        })
    }

    /// Computes a layered left-to-right layout of the lineage around `model`,
    /// bounded by `max_depth` levels per direction and `max_nodes` total.
    pub fn lineage_layout(
        &self,
        project_root: &Path,
        model: &str,
        max_depth: i32,
        max_nodes: usize,
        expansions: &std::collections::HashSet<String>,
    ) -> Result<LayoutGraph> {
        self.with_loaded(project_root, |loaded| {
            let center = Self::id_for(loaded, model)?;
            let query = GraphQuery::new(&loaded.graph);
            // Nodes the user expanded past the depth budget get a fresh one.
            let expansion_ids: std::collections::HashSet<i64> = expansions
                .iter()
                .filter_map(|name| Self::id_for(loaded, name).ok())
                .collect();

            // BFS levels in both directions, with a per-node depth budget so
            // expanded boundary nodes keep growing the graph.
            let mut level_of: HashMap<i64, i32> = HashMap::new();
            level_of.insert(center, 0);
            for upstream in [true, false] {
                let mut frontier: Vec<(i64, i32, i32)> = vec![(center, max_depth, 0)];
                while !frontier.is_empty() {
                    let mut next = Vec::new();
                    for &(id, budget, level) in &frontier {
                        let budget = if expansion_ids.contains(&id) {
                            budget.max(max_depth)
                        } else {
                            budget
                        };
                        if budget <= 0 {
                            continue;
                        }
                        let linked = if upstream {
                            query.incoming(id)?
                        } else {
                            query.outgoing(id)?
                        };
                        for linked_id in linked {
                            if level_of.len() >= max_nodes {
                                break;
                            }
                            level_of.entry(linked_id).or_insert_with(|| {
                                let child_level = if upstream { level - 1 } else { level + 1 };
                                next.push((linked_id, budget - 1, child_level));
                                child_level
                            });
                        }
                    }
                    frontier = next;
                }
            }

            // Longest-path re-layering: BFS assigns shortest-path depths, so a
            // node can land in the same column as its own dependency when
            // paths of different lengths meet. Push endpoints apart until
            // every edge points strictly rightward.
            let ids: Vec<i64> = level_of.keys().copied().collect();
            let mut set_edges: Vec<(i64, i64)> = Vec::new();
            for &id in &ids {
                for target in query.outgoing(id)? {
                    if level_of.contains_key(&target) {
                        set_edges.push((id, target));
                    }
                }
            }
            for _ in 0..ids.len() {
                let mut changed = false;
                for &(from, to) in &set_edges {
                    if to == center {
                        continue;
                    }
                    let from_level = level_of[&from];
                    let to_level = level_of[&to];
                    if to_level <= from_level {
                        if from == center {
                            level_of.insert(to, 1);
                        } else if to_level <= 0 {
                            // Upstream territory: push the source further left.
                            level_of.insert(from, to_level - 1);
                        } else {
                            // Downstream territory: push the target further right.
                            level_of.insert(to, from_level + 1);
                        }
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }

            // Group by level, resolve entities, and position them.
            let min_level = level_of.values().copied().min().unwrap_or(0);
            let max_level = level_of.values().copied().max().unwrap_or(0);
            let mut columns: Vec<Vec<(i64, GraphEntity)>> =
                vec![Vec::new(); (max_level - min_level + 1) as usize];
            for (&id, &level) in &level_of {
                if let Ok(entity) = loaded.graph.get_entity(id) {
                    columns[(level - min_level) as usize].push((id, entity));
                }
            }
            for column in &mut columns {
                column.sort_by(|a, b| a.1.name.cmp(&b.1.name));
            }

            // Crossing reduction: barycenter sweeps reorder each column by the
            // average row of connected nodes, untangling the arrangement.
            let mut neighbors: HashMap<i64, Vec<i64>> = HashMap::new();
            for &id in level_of.keys() {
                for target in query.outgoing(id)? {
                    if level_of.contains_key(&target) {
                        neighbors.entry(id).or_default().push(target);
                        neighbors.entry(target).or_default().push(id);
                    }
                }
            }
            let mut row_of: HashMap<i64, f32> = HashMap::new();
            for column in &columns {
                for (row, (id, _)) in column.iter().enumerate() {
                    row_of.insert(*id, row as f32);
                }
            }
            for sweep in 0..4 {
                let order: Vec<usize> = if sweep % 2 == 0 {
                    (0..columns.len()).collect()
                } else {
                    (0..columns.len()).rev().collect()
                };
                for column_ix in order {
                    let mut keyed: Vec<(f32, usize, (i64, GraphEntity))> = columns[column_ix]
                        .drain(..)
                        .enumerate()
                        .map(|(current_row, item)| {
                            let barycenter = neighbors
                                .get(&item.0)
                                .map(|linked| {
                                    let rows: Vec<f32> = linked
                                        .iter()
                                        .filter_map(|id| row_of.get(id).copied())
                                        .collect();
                                    if rows.is_empty() {
                                        current_row as f32
                                    } else {
                                        rows.iter().sum::<f32>() / rows.len() as f32
                                    }
                                })
                                .unwrap_or(current_row as f32);
                            (barycenter, current_row, item)
                        })
                        .collect();
                    keyed.sort_by(|a, b| {
                        a.0.partial_cmp(&b.0)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.1.cmp(&b.1))
                    });
                    columns[column_ix] = keyed.into_iter().map(|(_, _, item)| item).collect();
                    for (row, (id, _)) in columns[column_ix].iter().enumerate() {
                        row_of.insert(*id, row as f32);
                    }
                }
            }

            let row_pitch = GRAPH_NODE_HEIGHT + GRAPH_ROW_GAP;
            let tallest = columns.iter().map(Vec::len).max().unwrap_or(1) as f32;
            let content_height = tallest * row_pitch;

            let mut nodes = Vec::new();
            let mut index_of: HashMap<i64, usize> = HashMap::new();
            let mut x = GRAPH_PADDING;
            for (column_ix, column) in columns.iter().enumerate() {
                let level = min_level + column_ix as i32;
                let column_width = column
                    .iter()
                    .map(|(_, entity)| {
                        // Ops badges render beside the name; leave room for them.
                        let badge_pad = if entity.data.get("ops").is_some_and(|ops| !ops.is_null()) {
                            36.
                        } else {
                            0.
                        };
                        26. + badge_pad + 8. * entity.name.len() as f32
                    })
                    .fold(80.0_f32, f32::max);
                let y_offset =
                    GRAPH_PADDING + (content_height - column.len() as f32 * row_pitch) / 2.;
                for (row, (id, entity)) in column.iter().enumerate() {
                    index_of.insert(*id, nodes.len());
                    let truncated_up = query
                        .incoming(*id)?
                        .iter()
                        .any(|parent| !level_of.contains_key(parent));
                    let truncated_down = query
                        .outgoing(*id)?
                        .iter()
                        .any(|child| !level_of.contains_key(child));
                    nodes.push(GraphLayoutNode {
                        truncated_up,
                        truncated_down,
                        name: entity.name.clone(),
                        kind: entity.kind.clone(),
                        materialization: entity
                            .data
                            .get("materialized")
                            .and_then(|value| value.as_str())
                            .unwrap_or(entity.kind.as_str())
                            .to_owned(),
                        ops: entity.data.get("ops").and_then(NodeOps::from_json),
                        col_lineage: entity
                            .data
                            .get("col_lineage")
                            .and_then(|value| value.as_object())
                            .map(|map| {
                                map.iter()
                                    .filter_map(|(key, value)| {
                                        let leaves = value
                                            .as_array()?
                                            .iter()
                                            .filter_map(|pair| {
                                                let pair = pair.as_array()?;
                                                Some((
                                                    pair.first()?.as_str()?.to_owned(),
                                                    pair.get(1)?.as_str()?.to_owned(),
                                                ))
                                            })
                                            .collect();
                                        Some((key.clone(), leaves))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        col_refs: entity
                            .data
                            .get("col_refs")
                            .and_then(|value| value.as_object())
                            .map(|map| {
                                map.iter()
                                    .filter_map(|(key, value)| {
                                        let refs = value
                                            .as_array()?
                                            .iter()
                                            .filter_map(|item| {
                                                item.as_str().map(str::to_owned)
                                            })
                                            .collect();
                                        Some((key.clone(), refs))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        col_exprs: entity
                            .data
                            .get("col_exprs")
                            .and_then(|value| value.as_object())
                            .map(|map| {
                                map.iter()
                                    .filter_map(|(key, value)| {
                                        Some((key.clone(), value.as_str()?.to_owned()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        path: entity
                            .file_path
                            .as_ref()
                            .map(|path| project_root.join(path)),
                        columns: entity
                            .data
                            .get("columns")
                            .and_then(|columns| columns.as_array())
                            .map(|columns| {
                                columns
                                    .iter()
                                    .filter_map(|value| value.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        x,
                        y: y_offset + row as f32 * row_pitch,
                        width: column_width,
                        height: GRAPH_NODE_HEIGHT,
                        level,
                        is_center: *id == center,
                    });
                }
                x += column_width + GRAPH_COL_GAP;
            }

            let mut edges = Vec::new();
            for (&id, &from_ix) in &index_of {
                for target in query.outgoing(id)? {
                    if let Some(&to_ix) = index_of.get(&target) {
                        edges.push((from_ix, to_ix));
                    }
                }
            }

            Ok(LayoutGraph {
                nodes,
                edges,
                width: x - GRAPH_COL_GAP + GRAPH_PADDING,
                height: content_height + 2. * GRAPH_PADDING,
            })
        })
    }

    /// Returns the deep lineage of `model` in both directions, bounded by
    /// `max_depth` levels and `max_nodes` total nodes per direction.
    pub fn lineage_tree(
        &self,
        project_root: &Path,
        model: &str,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<LineageTree> {
        self.with_loaded(project_root, |loaded| {
            let id = Self::id_for(loaded, model)?;
            let mut up_budget = max_nodes;
            let mut down_budget = max_nodes;
            Ok(LineageTree {
                up: walk(loaded, project_root, id, true, max_depth, &mut up_budget)?,
                down: walk(loaded, project_root, id, false, max_depth, &mut down_budget)?,
            })
        })
    }
}

fn entity_to_node(project_root: &Path, entity: GraphEntity) -> ModelNode {
    ModelNode {
        name: entity.name,
        kind: entity.kind,
        path: entity.file_path.map(|path| project_root.join(path)),
    }
}

fn walk(
    loaded: &Loaded,
    project_root: &Path,
    from: i64,
    upstream: bool,
    depth_left: usize,
    budget: &mut usize,
) -> Result<Vec<LineageTreeNode>> {
    if depth_left == 0 {
        return Ok(Vec::new());
    }
    let query = GraphQuery::new(&loaded.graph);
    let ids = if upstream {
        query.incoming(from)?
    } else {
        query.outgoing(from)?
    };
    let mut nodes = Vec::new();
    for id in ids {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(entity) = loaded.graph.get_entity(id) else {
            continue;
        };
        let children = walk(loaded, project_root, id, upstream, depth_left - 1, budget)?;
        let truncated = children.is_empty() && {
            let next = if upstream {
                query.incoming(id)?
            } else {
                query.outgoing(id)?
            };
            !next.is_empty()
        };
        nodes.push(LineageTreeNode {
            node: entity_to_node(project_root, entity),
            children,
            truncated,
        });
    }
    nodes.sort_by(|a, b| a.node.name.cmp(&b.node.name));
    Ok(nodes)
}

/// Renders the lineage as a left-to-right SVG diagram: parents → model →
/// children.
pub fn lineage_svg(model: &str, lineage: &Lineage) -> String {
    fn add_node(graph: &mut VisualGraph, name: &str) -> NodeHandle {
        let shape = ShapeKind::new_box(name);
        let style = StyleAttr::simple();
        let width = 24. + 9. * name.len() as f64;
        graph.add_node(Element::create(
            shape,
            style,
            Orientation::LeftToRight,
            Point::new(width, 36.),
        ))
    }

    let mut graph = VisualGraph::new(Orientation::LeftToRight);
    let center = add_node(&mut graph, model);
    for parent in &lineage.parents {
        let handle = add_node(&mut graph, &parent.name);
        graph.add_edge(Arrow::simple(""), handle, center);
    }
    for child in &lineage.children {
        let handle = add_node(&mut graph, &child.name);
        graph.add_edge(Arrow::simple(""), center, handle);
    }

    let mut svg = SVGWriter::new();
    graph.do_it(false, false, false, &mut svg);
    svg.finalize()
}

/// Best-effort extraction of output column names from a compiled SELECT.
/// Returns None for anything it can't handle confidently (e.g. `select *`),
/// letting the caller fall back to parent inheritance.
/// Parses the final top-level select list into (column name, expression)
/// pairs — the per-model transformation story for column tracing.
fn parse_select_entries(sql: &str) -> Option<Vec<(String, String)>> {
    let lower = sql.to_lowercase();
    let select_pos = lower.find("select")?;
    let (entries, saw_star) = parse_select_entries_at(sql, &lower, select_pos)?;
    // A `select *` (or `t.*`) hides columns, so the list is not authoritative.
    (!saw_star).then_some(entries)
}

/// Every select statement's (column, expression) pairs, merged first-wins —
/// the earliest (deepest CTE) definition of a name is the real transformation;
/// later selects are usually passthroughs. One chaining pass resolves columns
/// whose expression is a bare rename of another parsed column.
fn parse_all_select_entries(sql: &str) -> std::collections::HashMap<String, String> {
    let lower = sql.to_lowercase();
    let is_ident = |ch: u8| ch.is_ascii_alphanumeric() || ch == b'_';
    let bytes = lower.as_bytes();
    let mut merged: std::collections::HashMap<String, String> = Default::default();
    let mut from = 0;
    while let Some(found) = lower[from..].find("select") {
        let at = from + found;
        from = at + 6;
        // Word boundaries so identifiers like `selected` don't match.
        if at > 0 && is_ident(bytes[at - 1]) {
            continue;
        }
        if bytes.get(at + 6).copied().is_some_and(is_ident) {
            continue;
        }
        if let Some((entries, _)) = parse_select_entries_at(sql, &lower, at) {
            for (name, expr) in entries {
                merged.entry(name.to_lowercase()).or_insert(expr);
            }
        }
    }
    // Chaining: `week_number` defined as bare `numero_semaine` picks up
    // numero_semaine's own expression when that one is a real transformation.
    let snapshot = merged.clone();
    for (name, expr) in merged.iter_mut() {
        let bare = expr
            .rsplit('.')
            .next()
            .unwrap_or(expr)
            .trim_matches('"')
            .to_lowercase();
        if bare == *name || !bare.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
            continue;
        }
        if let Some(base) = snapshot.get(&bare) {
            let base_bare = base.rsplit('.').next().unwrap_or(base).trim_matches('"');
            if !base_bare.eq_ignore_ascii_case(&bare) {
                *expr = base.clone();
            }
        }
    }
    merged
}

/// Returns the (name, expression) entries of one select list plus whether a
/// bare `*` / `alias.*` projection was seen.
fn parse_select_entries_at(
    sql: &str,
    lower: &str,
    select_pos: usize,
) -> Option<(Vec<(String, String)>, bool)> {
    let body = &sql[select_pos + 6..];
    let body_lower = &lower[select_pos + 6..];

    // Find the top-level FROM (paren depth 0).
    let bytes = body_lower.as_bytes();
    let mut depth = 0i32;
    let mut from_ix = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'f' if depth == 0
                && body_lower[i..].starts_with("from")
                && (i == 0
                    || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
                && body_lower[i + 4..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_') =>
            {
                from_ix = Some(i);
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let mut list = &body[..from_ix?];
    if list.contains("{{") {
        return None;
    }
    if let Some(stripped) = list.trim_start().strip_prefix("distinct") {
        list = stripped;
    }

    let mut columns = Vec::new();
    let mut saw_star = false;
    let mut push = |segment: &str| {
        let segment = segment.trim();
        if segment.is_empty() {
            return;
        }
        if segment == "*" || segment.ends_with(".*") {
            saw_star = true;
            return;
        }
        let segment_lower = segment.to_lowercase();
        let (name, expr) = if let Some(pos) = segment_lower.rfind(" as ") {
            (segment[pos + 4..].trim(), segment[..pos].trim())
        } else {
            (
                segment.rsplit('.').next().unwrap_or(segment).trim(),
                segment,
            )
        };
        let name = name.trim_matches('"');
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            // Whitespace-normalize; display truncation happens at storage
            // time so reference extraction sees the full expression.
            let expr = expr.split_whitespace().collect::<Vec<_>>().join(" ");
            columns.push((name.to_owned(), expr));
        }
    };
    let mut depth = 0i32;
    let mut start = 0;
    for (ix, ch) in list.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                push(&list[start..ix]);
                start = ix + 1;
            }
            _ => {}
        }
    }
    push(&list[start..]);

    (!columns.is_empty()).then_some((columns, saw_star))
}

fn parse_select_columns(sql: &str) -> Option<Vec<String>> {
    parse_select_entries(sql)
        .map(|entries| entries.into_iter().map(|(name, _)| name).collect())
}

fn build_graph(
    project_root: &Path,
    manifest_path: &Path,
    manifest_mtime: SystemTime,
) -> Result<Loaded> {
    let manifest: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(manifest_path).context("opening target/manifest.json")?,
    ))
    .context("parsing target/manifest.json")?;

    let db_path = project_root.join("target").join("zed-dbt-lineage.db");
    let _ = std::fs::remove_file(&db_path);
    let graph = SqliteGraph::open(&db_path)
        .map_err(|error| anyhow::anyhow!("opening lineage graph db: {error}"))?;

    // Column metadata: catalog.json (real warehouse columns, ordered) when
    // available, produced by `dbt compile --write-catalog`.
    let catalog: Option<serde_json::Value> =
        std::fs::File::open(project_root.join("target").join("catalog.json"))
            .ok()
            .and_then(|file| serde_json::from_reader(std::io::BufReader::new(file)).ok());
    let catalog_columns = |unique_id: &str| -> Option<Vec<String>> {
        let catalog = catalog.as_ref()?;
        for section in ["nodes", "sources"] {
            if let Some(columns) = catalog
                .get(section)
                .and_then(|section| section.get(unique_id))
                .and_then(|node| node.get("columns"))
                .and_then(|columns| columns.as_object())
            {
                let mut ordered: Vec<(i64, String)> = columns
                    .iter()
                    .map(|(name, meta)| {
                        (
                            meta.get("index").and_then(|index| index.as_i64()).unwrap_or(0),
                            name.clone(),
                        )
                    })
                    .collect();
                ordered.sort();
                return Some(ordered.into_iter().map(|(_, name)| name).collect());
            }
        }
        None
    };

    let mut by_uid: HashMap<String, i64> = HashMap::new();
    let mut by_name: HashMap<String, i64> = HashMap::new();

    // Phase 1: collect nodes with the best column information available.
    // Ephemeral models have no catalog entry, so fall back to parsing their
    // compiled select list, and finally to inheriting parent columns.
    struct PendingNode {
        unique_id: String,
        kind: String,
        name: String,
        file_path: Option<String>,
        materialized: String,
        columns: Vec<String>,
        ops: Option<NodeOps>,
        col_exprs: std::collections::HashMap<String, String>,
        col_refs: std::collections::HashMap<String, Vec<String>>,
        /// Lowercased last segment of relation_name (how FROM clauses see it).
        table_ident: Option<String>,
        compiled_code: Option<String>,
        /// Exact AST-resolved lineage: column -> (parent node name, column).
        col_lineage: std::collections::HashMap<String, Vec<(String, String)>>,
    }
    let mut pending: Vec<PendingNode> = Vec::new();
    for section in ["nodes", "sources"] {
        let Some(nodes) = manifest.get(section).and_then(|nodes| nodes.as_object()) else {
            continue;
        };
        for (unique_id, node) in nodes {
            let resource_type = node
                .get("resource_type")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !matches!(resource_type, "model" | "seed" | "snapshot" | "source") {
                continue;
            }
            let Some(name) = node.get("name").and_then(|value| value.as_str()) else {
                continue;
            };
            let file_path = node
                .get("original_file_path")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            let materialized = node
                .get("config")
                .and_then(|config| config.get("materialized"))
                .and_then(|value| value.as_str())
                .unwrap_or(resource_type)
                .to_owned();
            let columns = catalog_columns(unique_id)
                .or_else(|| {
                    let documented: Vec<String> = node
                        .get("columns")
                        .and_then(|columns| columns.as_object())
                        .map(|columns| columns.keys().cloned().collect())
                        .unwrap_or_default();
                    (!documented.is_empty()).then_some(documented)
                })
                .or_else(|| {
                    node.get("compiled_code")
                        .and_then(|value| value.as_str())
                        .and_then(parse_select_columns)
                })
                .unwrap_or_default();
            let ops = node
                .get("compiled_code")
                .or_else(|| node.get("raw_code"))
                .and_then(|value| value.as_str())
                .map(extract_ops)
                .filter(|ops| !ops.is_empty());
            let col_exprs_full: std::collections::HashMap<String, String> = node
                .get("compiled_code")
                .and_then(|value| value.as_str())
                .map(parse_all_select_entries)
                .unwrap_or_default();
            // References come from the full expression; the stored display
            // copy is truncated on a char boundary.
            let col_refs: std::collections::HashMap<String, Vec<String>> = col_exprs_full
                .iter()
                .map(|(name, expr)| (name.clone(), expr_column_refs(expr)))
                .filter(|(_, refs)| !refs.is_empty())
                .collect();
            let col_exprs: std::collections::HashMap<String, String> = col_exprs_full
                .into_iter()
                .map(|(name, mut expr)| {
                    if expr.len() > 160 {
                        let mut cut = 157;
                        while !expr.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        expr.truncate(cut);
                        expr.push_str("...");
                    }
                    (name, expr)
                })
                .collect();
            pending.push(PendingNode {
                unique_id: unique_id.clone(),
                kind: resource_type.to_owned(),
                name: name.to_owned(),
                file_path,
                materialized,
                columns,
                ops,
                col_exprs,
                col_refs,
                table_ident: node
                    .get("relation_name")
                    .and_then(|value| value.as_str())
                    .and_then(|relation| relation.rsplit('.').next())
                    .or_else(|| {
                        node.get("alias")
                            .and_then(|value| value.as_str())
                            .or(Some(name))
                    })
                    .map(|ident| ident.trim_matches('"').to_lowercase()),
                compiled_code: node
                    .get("compiled_code")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
                col_lineage: Default::default(),
            });
        }
    }

    // Phase 2: nodes still without columns (e.g. `select *` ephemerals)
    // inherit the union of their parents' columns.
    let parents_of: HashMap<String, Vec<String>> = manifest
        .get("parent_map")
        .and_then(|map| map.as_object())
        .map(|map| {
            map.iter()
                .map(|(child, parents)| {
                    (
                        child.clone(),
                        parents
                            .as_array()
                            .map(|parents| {
                                parents
                                    .iter()
                                    .filter_map(|value| value.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let index_of_uid: HashMap<String, usize> = pending
        .iter()
        .enumerate()
        .map(|(ix, node)| (node.unique_id.clone(), ix))
        .collect();
    for _ in 0..10 {
        let mut changed = false;
        for ix in 0..pending.len() {
            if !pending[ix].columns.is_empty() {
                continue;
            }
            let mut inherited: Vec<String> = Vec::new();
            for parent_uid in parents_of.get(&pending[ix].unique_id).into_iter().flatten() {
                if let Some(&parent_ix) = index_of_uid.get(parent_uid) {
                    for column in &pending[parent_ix].columns {
                        if !inherited.contains(column) {
                            inherited.push(column.clone());
                        }
                    }
                }
            }
            if !inherited.is_empty() {
                pending[ix].columns = inherited;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Phase 2.5: exact AST-based column lineage, now that every node's
    // column list is final. Falls back to the text heuristic per column at
    // match time when a model's SQL defeats the resolver.
    for ix in 0..pending.len() {
        let Some(sql) = pending[ix].compiled_code.clone() else {
            continue;
        };
        let mut upstream_map = std::collections::HashMap::new();
        for parent_uid in parents_of.get(&pending[ix].unique_id).into_iter().flatten() {
            if let Some(&parent_ix) = index_of_uid.get(parent_uid) {
                let parent = &pending[parent_ix];
                if let Some(ident) = parent.table_ident.clone() {
                    upstream_map.insert(
                        ident,
                        crate::lineage_sql::UpstreamRelation {
                            node: parent.name.clone(),
                            columns: parent
                                .columns
                                .iter()
                                .map(|column| column.to_lowercase())
                                .collect(),
                        },
                    );
                }
            }
        }
        if upstream_map.is_empty() {
            continue;
        }
        if let Some(lineage) = crate::lineage_sql::column_lineage(&sql, &upstream_map) {
            pending[ix].col_lineage = lineage
                .into_iter()
                .filter(|(_, leaves)| !leaves.is_empty())
                .collect();
        }
    }

    // Phase 3: insert into the graph.
    for node in &pending {
        let id = graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: node.kind.clone(),
                name: node.name.clone(),
                file_path: node.file_path.clone(),
                data: json!({
                    "materialized": node.materialized,
                    "columns": node.columns,
                    "ops": node.ops.as_ref().map(NodeOps::to_json),
                    "col_exprs": node.col_exprs,
                    "col_refs": node.col_refs,
                    "col_lineage": node.col_lineage,
                }),
            })
            .map_err(|error| anyhow::anyhow!("inserting lineage node: {error}"))?;
        by_uid.insert(node.unique_id.clone(), id);
        by_name.insert(node.name.clone(), id);
    }

    if let Some(parent_map) = manifest.get("parent_map").and_then(|map| map.as_object()) {
        for (child_uid, parents) in parent_map {
            let Some(&child_id) = by_uid.get(child_uid) else {
                continue;
            };
            let Some(parents) = parents.as_array() else {
                continue;
            };
            for parent_uid in parents.iter().filter_map(|value| value.as_str()) {
                let Some(&parent_id) = by_uid.get(parent_uid) else {
                    continue;
                };
                graph
                    .insert_edge(&GraphEdge {
                        id: 0,
                        from_id: parent_id,
                        to_id: child_id,
                        edge_type: "dependency".to_owned(),
                        data: json!({}),
                    })
                    .map_err(|error| anyhow::anyhow!("inserting lineage edge: {error}"))?;
            }
        }
    }

    Ok(Loaded {
        graph,
        manifest_mtime,
        catalog_mtime: catalog_mtime(project_root),
        by_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_asterisk_does_not_disable_expressions() {
        let sql = "SELECT\n  c.employee_id,\n  CASE WHEN x < c.reference_seniority_date THEN 0 ELSE 1 END AS seniority_in_years,\n  seniority_in_years * 12 AS seniority_in_months\nFROM t";
        let entries = parse_all_select_entries(sql);
        assert_eq!(
            entries.get("seniority_in_years").map(|expr| expr.contains("reference_seniority_date")),
            Some(true)
        );
        assert!(entries.contains_key("seniority_in_months"));
    }

    #[test]
    fn lone_star_disqualifies_column_list_but_not_expressions() {
        let sql = "SELECT t.*, a AS b FROM t";
        assert_eq!(parse_select_columns(sql), None);
        let entries = parse_all_select_entries(sql);
        assert_eq!(entries.get("b").map(String::as_str), Some("a"));
    }

    #[test]
    fn cte_rename_chain_resolves_to_base_expression() {
        let sql = "WITH base AS (SELECT sum(amount) AS total FROM x), o AS (SELECT total AS grand_total FROM base) SELECT grand_total FROM o";
        let entries = parse_all_select_entries(sql);
        assert_eq!(entries.get("grand_total").map(String::as_str), Some("sum(amount)"));
    }
}
