//! Part of the xDI lineage graph as the dbt canvas's [`LayoutGraph`],
//! placed by [`crate::graph_layout`]: names made unique (the drawing knows
//! nodes by name), columns, and what each column is made of — a mapping's
//! `<target>.<column>` from its sources', a target's from the mapping's —
//! so the dbt canvas's column highlight traces through mappings unchanged.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use el_import::lineage::{Neighbourhood, Node};
use el_import::model::{ExprKind, JoinKind, Project};

use crate::graph_layout::{Spacing, layered};
use crate::lineage::{
    GRAPH_COL_GAP, GRAPH_COLUMN_ROW_HEIGHT, GRAPH_MAX_COLUMNS, GRAPH_NODE_HEIGHT, GRAPH_PADDING,
    GRAPH_ROW_GAP, GraphLayoutNode, LayoutGraph, NodeOps,
};

/// A layout, and the graph node each of its nodes is.
#[derive(Clone, Debug, Default)]
pub struct XdiLayout {
    pub graph: LayoutGraph,
    pub ids: Vec<String>,
}

impl XdiLayout {
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|candidate| candidate == id)
    }
}

/// The widest a node gets.
const MAX_WIDTH: f32 = 360.;

/// Lays `around` out; with `show_columns`, nodes are tall enough for their
/// first columns.
pub fn layout(around: &Neighbourhood, project: &Project, show_columns: bool) -> XdiLayout {
    let nodes = &around.graph.nodes;
    let names = unique_names(nodes);
    let ix_of: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(ix, node)| (node.id.as_str(), ix))
        .collect();

    // What each column is made of: the column edges into it.
    let mut lineage: Vec<HashMap<String, Vec<(String, String)>>> =
        vec![HashMap::new(); nodes.len()];
    for edge in &around.graph.columns {
        let (Some(&source), Some(&target)) = (
            ix_of.get(edge.source.as_str()),
            ix_of.get(edge.target.as_str()),
        ) else {
            continue;
        };
        let leaf = (names[source].clone(), edge.source_column.clone());
        let leaves = lineage[target]
            .entry(edge.target_column.clone())
            .or_default();
        if !leaves.contains(&leaf) {
            leaves.push(leaf);
        }
    }

    let mut layout_nodes: Vec<GraphLayoutNode> = Vec::with_capacity(nodes.len());
    for (ix, node) in nodes.iter().enumerate() {
        let job = (node.kind == "job")
            .then(|| project.job(&node.id))
            .flatten();
        let mut columns: Vec<String> = match job {
            Some(job) => {
                let mut seen = std::collections::HashSet::new();
                job.columns
                    .iter()
                    .filter(|column| !column.owner_is_stage && column.enabled)
                    .map(|column| format!("{}.{}", column.owner, column.column))
                    .filter(|column| seen.insert(column.to_lowercase()))
                    .collect()
            }
            None => detail_columns(node),
        };
        // Columns the edges name and the metadata did not (a table
        // without metadata).
        let known: std::collections::HashSet<String> =
            columns.iter().map(|column| column.to_lowercase()).collect();
        let mut extra: Vec<String> = lineage[ix]
            .keys()
            .filter(|column| !known.contains(*column))
            .cloned()
            .collect();
        for edge in around
            .graph
            .columns
            .iter()
            .filter(|edge| edge.source == node.id)
        {
            if !known.contains(&edge.source_column) && !extra.contains(&edge.source_column) {
                extra.push(edge.source_column.clone());
            }
        }
        extra.sort();
        columns.extend(extra);

        let mut col_lineage = std::mem::take(&mut lineage[ix]);
        // A column made of nothing upstream says so, rather than falling to
        // the name heuristic: a mapping's constants, a target's columns no
        // mapping here writes.
        if job.is_some() || !col_lineage.is_empty() {
            for column in &columns {
                col_lineage.entry(column.to_lowercase()).or_default();
            }
        }
        let mut col_exprs = HashMap::new();
        let mut ops = None;
        if let Some(job) = job {
            let mut by_column: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for column in job.columns.iter().filter(|column| !column.owner_is_stage) {
                let text = match &column.set {
                    Some(set) if column_sets(job, &column.owner) > 1 => {
                        format!("[{set}] {}", column.expression.text)
                    }
                    _ => column.expression.text.clone(),
                };
                by_column
                    .entry(format!("{}.{}", column.owner, column.column).to_lowercase())
                    .or_default()
                    .push(text);
            }
            col_exprs = by_column
                .into_iter()
                .map(|(column, texts)| (column, texts.join(" ∪ ")))
                .collect();
            ops = Some(job_ops(job));
        }

        let name = names[ix].clone();
        let mut width = (26. + 7.5 * name.chars().count() as f32).clamp(140., MAX_WIDTH);
        let mut height = GRAPH_NODE_HEIGHT;
        if show_columns && !columns.is_empty() {
            let shown = columns.len().min(GRAPH_MAX_COLUMNS);
            let more = usize::from(columns.len() > GRAPH_MAX_COLUMNS);
            height += 4. + (shown + more) as f32 * GRAPH_COLUMN_ROW_HEIGHT;
            let longest = columns
                .iter()
                .take(GRAPH_MAX_COLUMNS)
                .map(|column| column.chars().count())
                .max()
                .unwrap_or(0);
            width = width.max((28. + 7.2 * longest as f32).min(MAX_WIDTH));
        }
        layout_nodes.push(GraphLayoutNode {
            name,
            kind: node.kind.clone(),
            materialization: match node.kind.as_str() {
                "job" => "mapping".to_owned(),
                _ => node
                    .detail
                    .get("technology")
                    .and_then(|technology| technology.as_str())
                    .unwrap_or("unknown")
                    .to_lowercase(),
            },
            path: job.map(|job| project.root.join(Path::new(&job.path))),
            ops,
            col_exprs,
            col_refs: HashMap::new(),
            col_lineage,
            details: Some(node.detail.clone()),
            truncated_up: around.more_up.contains(&node.id),
            truncated_down: around.more_down.contains(&node.id),
            columns,
            x: 0.,
            y: 0.,
            width,
            height,
            level: 0,
            is_center: around.center.as_deref() == Some(node.id.as_str()),
        });
    }

    let edges: Vec<(usize, usize)> = around
        .graph
        .edges
        .iter()
        .filter_map(|edge| {
            Some((
                *ix_of.get(edge.source.as_str())?,
                *ix_of.get(edge.target.as_str())?,
            ))
        })
        .collect();
    let sizes: Vec<(f32, f32)> = layout_nodes
        .iter()
        .map(|node| (node.width, node.height))
        .collect();
    let center = layout_nodes.iter().position(|node| node.is_center);
    let placement = layered(
        &sizes,
        &edges,
        center,
        Spacing {
            padding: GRAPH_PADDING,
            column_gap: GRAPH_COL_GAP,
            row_gap: GRAPH_ROW_GAP,
        },
    );
    for (node, placed) in layout_nodes.iter_mut().zip(&placement.nodes) {
        node.x = placed.x;
        node.y = placed.y;
        node.level = placed.level;
    }
    XdiLayout {
        graph: LayoutGraph {
            nodes: layout_nodes,
            edges,
            width: placement.width,
            height: placement.height,
        },
        ids: nodes.iter().map(|node| node.id.clone()).collect(),
    }
}

/// How many sets a target or stage has.
fn column_sets(job: &el_import::model::Job, owner: &str) -> usize {
    job.target(owner)
        .map(|target| target.sets.len())
        .or_else(|| job.stage(owner).map(|stage| stage.sets.len()))
        .unwrap_or(0)
}

/// A mapping's badges: its joins, aggregates, filters, distinct rows and
/// unions.
fn job_ops(job: &el_import::model::Job) -> NodeOps {
    let unions = job
        .stages
        .iter()
        .map(|stage| stage.sets.len().saturating_sub(1))
        .chain(
            job.targets
                .iter()
                .map(|target| target.sets.len().saturating_sub(1)),
        )
        .sum();
    let aggregate = job.columns.iter().any(|column| column.aggregate);
    NodeOps {
        joins: job
            .joins
            .iter()
            .filter(|join| join.enabled)
            .map(|join| {
                let kind = match join.kind {
                    JoinKind::Unset => "join",
                    kind => kind.label(),
                };
                format!("{kind} {}", join.right)
            })
            .collect(),
        aggregations: if aggregate {
            vec!["aggregate".to_owned()]
        } else {
            Vec::new()
        },
        group_by: aggregate,
        filters: if job.filters.iter().any(|filter| filter.enabled) {
            vec!["where".to_owned()]
        } else {
            Vec::new()
        },
        windows: false,
        distinct: job
            .templates
            .iter()
            .any(|template| template.parameter("useDistinct") == Some("true")),
        unions,
    }
}

/// A dataset node's columns, as its detail lists them.
fn detail_columns(node: &Node) -> Vec<String> {
    node.detail
        .get("columns")
        .and_then(|columns| columns.as_array())
        .map(|columns| {
            columns
                .iter()
                .filter_map(|column| column.get("name")?.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Names the drawing can tell apart: a table's `SCHEMA.NAME`, with its
/// server when two share it; a mapping's name, with its folder when it
/// clashes.
fn unique_names(nodes: &[Node]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for node in nodes {
        *counts.entry(node.label.as_str()).or_default() += 1;
    }
    let mut names: Vec<String> = nodes
        .iter()
        .map(|node| {
            if counts.get(node.label.as_str()).copied().unwrap_or(0) <= 1 {
                return node.label.clone();
            }
            if node.kind == "job" {
                let folder = node
                    .detail
                    .get("folder")
                    .and_then(|folder| folder.as_str())
                    .unwrap_or_default();
                format!("{} ({folder})", node.label)
            } else {
                match &node.connection {
                    Some(server) => format!("{}@{server}", node.label),
                    None => node.label.clone(),
                }
            }
        })
        .collect();
    // Still clashing: numbered.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for name in &mut names {
        let count = seen.entry(name.clone()).or_default();
        *count += 1;
        if *count > 1 {
            name.push_str(&format!(" #{count}"));
        }
    }
    names
}

/// Whether a mapping column is a plain copy of one column.
pub fn is_copy(project: &Project, job: &str, column: &str) -> Option<bool> {
    let job = project.job(job)?;
    let (owner, name) = column.split_once('.')?;
    let mapping = job.columns.iter().find(|mapping| {
        mapping.owner.eq_ignore_ascii_case(owner) && mapping.column.eq_ignore_ascii_case(name)
    })?;
    Some(mapping.expression.kind == ExprKind::Copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results_panel::DbtResultsPanel;
    use crate::xdi::store::fixture_snapshot;
    use el_import::lineage::neighbourhood;

    const FACT: &str = "table:snowflake://xy12345.eu-west-1/sales_db/analytics/fact_orders";

    #[test]
    fn a_column_is_traced_through_a_mapping_and_every_set_of_its_stage() {
        let dir = tempfile::tempdir().unwrap();
        let (_, snapshot) = fixture_snapshot(dir.path());
        let around = neighbourhood(&snapshot.index, FACT, 1, 1, 150);
        let layout = layout(&around, &snapshot.project, true);
        let graph = &layout.graph;
        let name = |id: &str| graph.nodes[layout.index_of(id).unwrap()].name.clone();

        // LOAD_ORDERS and its copy share a name: the folder tells them apart.
        let names: Vec<&str> = graph.nodes.iter().map(|node| node.name.as_str()).collect();
        assert!(names.contains(&"ANALYTICS.FACT_ORDERS"), "{names:?}");
        assert!(names.contains(&"LOAD_ORDERS"), "{names:?}");
        assert!(names.contains(&"LOAD_ORDERS_old"), "{names:?}");
        assert_eq!(
            names.len(),
            names.iter().collect::<std::collections::HashSet<_>>().len()
        );

        let marks = DbtResultsPanel::column_highlights(graph, "amount");
        let marked = |id: &str| marks[layout.index_of(id).unwrap()].clone();
        let job = "job:xdi:Orders/Mappings/LOAD_ORDERS.map";
        assert!(
            marked(job).contains("fact_orders.amount"),
            "{:?}",
            marked(job)
        );
        let eu = "table:oracle://crm-db.example:1521/crm/crm_owner/orders_eu";
        let us = "table:oracle://crm-db.example:1521/crm/crm_owner/orders_us";
        assert!(marked(eu).contains("amount"));
        assert!(marked(us).contains("amount_usd"), "{:?}", marked(us));
        // Downstream too: the export reads the fact table's amount.
        let export = "job:xdi:Orders/Mappings/EXPORT_ORDERS.map";
        assert!(marked(export).contains("orders_csv.amount"));
        // And nothing else: the order id stays unmarked.
        assert!(!marked(eu).contains("order_id"));
        assert_eq!(name(FACT), "ANALYTICS.FACT_ORDERS");
    }

    #[test]
    fn the_center_sits_in_column_zero_and_edges_point_right() {
        let dir = tempfile::tempdir().unwrap();
        let (_, snapshot) = fixture_snapshot(dir.path());
        let around = neighbourhood(&snapshot.index, FACT, 2, 2, 150);
        for show_columns in [false, true] {
            let layout = layout(&around, &snapshot.project, show_columns);
            let graph = &layout.graph;
            let center = graph.nodes.iter().find(|node| node.is_center).unwrap();
            assert_eq!(center.level, 0);
            for &(from, to) in &graph.edges {
                assert!(graph.nodes[from].x < graph.nodes[to].x);
            }
            let job = &graph.nodes[layout
                .index_of("job:xdi:Orders/Mappings/LOAD_ORDERS.map")
                .unwrap()];
            assert!(job.ops.as_ref().is_some_and(|ops| ops.unions == 1));
            assert_eq!(job.materialization, "mapping");
            assert!(
                job.path
                    .as_ref()
                    .is_some_and(|path| path.ends_with("LOAD_ORDERS.map"))
            );
            assert_eq!(center.materialization, "snowflake");
            assert!(center.path.is_none(), "a metadata file never opens");
        }
        assert_eq!(
            is_copy(
                &snapshot.project,
                "job:xdi:Orders/Mappings/LOAD_ORDERS.map",
                "FACT_ORDERS.ORDER_ID"
            ),
            Some(true)
        );
    }
}
