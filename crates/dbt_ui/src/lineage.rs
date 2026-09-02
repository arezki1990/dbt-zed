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
#[derive(Clone, Debug)]
pub struct GraphLayoutNode {
    pub name: String,
    pub kind: String,
    pub materialization: String,
    pub path: Option<PathBuf>,
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
    ) -> Result<LayoutGraph> {
        self.with_loaded(project_root, |loaded| {
            let center = Self::id_for(loaded, model)?;
            let query = GraphQuery::new(&loaded.graph);

            // BFS levels in both directions.
            let mut level_of: HashMap<i64, i32> = HashMap::new();
            level_of.insert(center, 0);
            for upstream in [true, false] {
                let mut frontier = vec![center];
                for depth in 1..=max_depth {
                    let mut next = Vec::new();
                    for &id in &frontier {
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
                                next.push(linked_id);
                                if upstream { -depth } else { depth }
                            });
                        }
                    }
                    frontier = next;
                    if frontier.is_empty() {
                        break;
                    }
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
                    .map(|(_, entity)| 26. + 8. * entity.name.len() as f32)
                    .fold(80.0_f32, f32::max);
                let y_offset =
                    GRAPH_PADDING + (content_height - column.len() as f32 * row_pitch) / 2.;
                for (row, (id, entity)) in column.iter().enumerate() {
                    index_of.insert(*id, nodes.len());
                    nodes.push(GraphLayoutNode {
                        name: entity.name.clone(),
                        kind: entity.kind.clone(),
                        materialization: entity
                            .data
                            .get("materialized")
                            .and_then(|value| value.as_str())
                            .unwrap_or(entity.kind.as_str())
                            .to_owned(),
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
fn parse_select_columns(sql: &str) -> Option<Vec<String>> {
    let lower = sql.to_lowercase();
    let select_pos = lower.find("select")?;
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
    if list.contains('*') || list.contains("{{") {
        return None;
    }
    if let Some(stripped) = list.trim_start().strip_prefix("distinct") {
        list = stripped;
    }

    let mut columns = Vec::new();
    let mut push = |segment: &str| {
        let segment = segment.trim();
        if segment.is_empty() {
            return;
        }
        let segment_lower = segment.to_lowercase();
        let name = if let Some(pos) = segment_lower.rfind(" as ") {
            segment[pos + 4..].trim()
        } else {
            segment.rsplit('.').next().unwrap_or(segment).trim()
        };
        let name = name.trim_matches('"');
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            columns.push(name.to_owned());
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

    (!columns.is_empty()).then_some(columns)
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
            pending.push(PendingNode {
                unique_id: unique_id.clone(),
                kind: resource_type.to_owned(),
                name: name.to_owned(),
                file_path,
                materialized,
                columns,
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

    // Phase 3: insert into the graph.
    for node in &pending {
        let id = graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: node.kind.clone(),
                name: node.name.clone(),
                file_path: node.file_path.clone(),
                data: json!({ "materialized": node.materialized, "columns": node.columns }),
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
