use ahash::AHashMap;
use rusqlite::params;

use crate::{
    SqliteGraphError,
    backend::BackendDirection,
    graph::{GraphEntity, SqliteGraph},
};

const OUTGOING_FILTER_SQL: &str =
    "SELECT to_id FROM graph_edges WHERE from_id=?1 AND edge_type=?2 ORDER BY to_id, id";
const INCOMING_FILTER_SQL: &str =
    "SELECT from_id FROM graph_edges WHERE to_id=?1 AND edge_type=?2 ORDER BY from_id, id";

#[derive(Clone, Debug, Default)]
pub struct NodeConstraint {
    pub kind: Option<String>,
    pub name_prefix: Option<String>,
}

impl NodeConstraint {
    pub fn kind(kind: &str) -> Self {
        Self {
            kind: Some(kind.to_string()),
            ..Self::default()
        }
    }

    pub fn name_prefix(prefix: &str) -> Self {
        Self {
            name_prefix: Some(prefix.to_string()),
            ..Self::default()
        }
    }

    pub fn matches(&self, entity: &GraphEntity) -> bool {
        if self.kind.as_ref().is_some_and(|kind| &entity.kind != kind) {
            return false;
        }
        if self
            .name_prefix
            .as_ref()
            .is_some_and(|prefix| !entity.name.starts_with(prefix))
        {
            return false;
        }
        true
    }
}

pub fn entity_ids_with_constraint(
    graph: &SqliteGraph,
    constraint: &NodeConstraint,
) -> Result<Vec<i64>, SqliteGraphError> {
    match (
        constraint.kind.as_deref(),
        constraint.name_prefix.as_deref(),
    ) {
        (None, None) => graph.all_entity_ids(),
        (Some(kind), Some(prefix)) => query_kind_and_prefix(graph, kind, prefix),
        (Some(kind), None) => query_kind(graph, kind),
        (None, Some(prefix)) => query_prefix(graph, prefix),
    }
}

fn query_kind(graph: &SqliteGraph, kind: &str) -> Result<Vec<i64>, SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT id FROM graph_entities WHERE kind=?1 ORDER BY id")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![kind], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    collect_ids(rows)
}

fn query_prefix(graph: &SqliteGraph, prefix: &str) -> Result<Vec<i64>, SqliteGraphError> {
    let like = format!("{prefix}%");
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT id FROM graph_entities WHERE name LIKE ?1 ORDER BY id")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![like], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    collect_ids(rows)
}

fn query_kind_and_prefix(
    graph: &SqliteGraph,
    kind: &str,
    prefix: &str,
) -> Result<Vec<i64>, SqliteGraphError> {
    let like = format!("{prefix}%");
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT id FROM graph_entities WHERE kind=?1 AND name LIKE ?2 ORDER BY id")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![kind, like], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    collect_ids(rows)
}

fn collect_ids<F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<i64>, SqliteGraphError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<i64>,
{
    let mut ids = Vec::new();
    for entry in rows {
        ids.push(entry.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }
    Ok(ids)
}

#[derive(Clone, Debug)]
pub struct PatternLeg {
    pub direction: BackendDirection,
    pub edge_type: Option<String>,
    pub constraint: Option<NodeConstraint>,
}

#[derive(Clone, Debug, Default)]
pub struct PatternQuery {
    pub root: Option<NodeConstraint>,
    pub legs: Vec<PatternLeg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternMatch {
    pub nodes: Vec<i64>,
}

pub fn execute_pattern(
    graph: &SqliteGraph,
    start: i64,
    query: &PatternQuery,
) -> Result<Vec<PatternMatch>, SqliteGraphError> {
    if let Some(root_constraint) = &query.root {
        let root = graph.get_entity(start)?;
        if !root_constraint.matches(&root) {
            return Ok(Vec::new());
        }
    }
    let mut cache: AHashMap<i64, GraphEntity> = AHashMap::new();
    let mut sequences: Vec<Vec<i64>> = vec![vec![start]];
    for leg in &query.legs {
        let mut next_sequences = Vec::new();
        for seq in &sequences {
            let current = *seq.last().expect("sequence non-empty");
            let neighbors =
                neighbors_with_filters(graph, current, leg.direction, leg.edge_type.as_deref())?;
            for neighbor in neighbors {
                if matches_constraint(graph, neighbor, leg.constraint.as_ref(), &mut cache)? {
                    let mut new_seq = seq.clone();
                    new_seq.push(neighbor);
                    next_sequences.push(new_seq);
                }
            }
        }
        if next_sequences.is_empty() {
            return Ok(Vec::new());
        }
        next_sequences.sort();
        next_sequences.dedup();
        sequences = next_sequences;
    }
    let mut matches: Vec<PatternMatch> = sequences
        .into_iter()
        .map(|nodes| PatternMatch { nodes })
        .collect();
    matches.sort_by(|a, b| a.nodes.cmp(&b.nodes));
    Ok(matches)
}

fn neighbors_with_filters(
    graph: &SqliteGraph,
    node: i64,
    direction: BackendDirection,
    edge_type: Option<&str>,
) -> Result<Vec<i64>, SqliteGraphError> {
    match (direction, edge_type) {
        (BackendDirection::Outgoing, None) => graph.fetch_outgoing(node),
        (BackendDirection::Incoming, None) => graph.fetch_incoming(node),
        (BackendDirection::Outgoing, Some(ty)) => {
            filter_neighbors(graph, node, OUTGOING_FILTER_SQL, ty)
        }
        (BackendDirection::Incoming, Some(ty)) => {
            filter_neighbors(graph, node, INCOMING_FILTER_SQL, ty)
        }
    }
}

fn filter_neighbors(
    graph: &SqliteGraph,
    node: i64,
    sql: &str,
    edge_type: &str,
) -> Result<Vec<i64>, SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![node, edge_type], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let mut values = Vec::new();
    for value in rows {
        values.push(value.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }
    Ok(values)
}

fn matches_constraint(
    graph: &SqliteGraph,
    node: i64,
    constraint: Option<&NodeConstraint>,
    cache: &mut AHashMap<i64, GraphEntity>,
) -> Result<bool, SqliteGraphError> {
    if constraint.is_none() {
        return Ok(true);
    }
    let entry = if let Some(entity) = cache.get(&node) {
        entity.clone()
    } else {
        let entity = graph.get_entity(node)?;
        cache.insert(node, entity.clone());
        entity
    };
    match constraint {
        Some(constraint) => Ok(constraint.matches(&entry)),
        None => Err(SqliteGraphError::invalid_input(
            "Pattern constraint not available for matching",
        )),
    }
}
