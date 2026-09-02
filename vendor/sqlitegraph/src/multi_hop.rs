use std::collections::VecDeque;

use ahash::AHashSet;

use crate::{SqliteGraphError, backend::BackendDirection, graph::SqliteGraph};

const OUTGOING_TYPED_SQL: &str =
    "SELECT to_id, edge_type FROM graph_edges WHERE from_id=?1 ORDER BY to_id, edge_type, id";
const INCOMING_TYPED_SQL: &str =
    "SELECT from_id, edge_type FROM graph_edges WHERE to_id=?1 ORDER BY from_id, edge_type, id";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainStep {
    pub direction: BackendDirection,
    pub edge_type: Option<String>,
}

pub fn k_hop(
    graph: &SqliteGraph,
    start: i64,
    depth: u32,
    direction: BackendDirection,
) -> Result<Vec<i64>, SqliteGraphError> {
    k_hop_internal(graph, start, depth, direction, None)
}

pub fn k_hop_multi(
    graph: &SqliteGraph,
    starts: &[i64],
    depth: u32,
    direction: BackendDirection,
) -> Result<Vec<i64>, SqliteGraphError> {
    if depth == 0 || starts.is_empty() {
        return Ok(Vec::new());
    }
    let mut visited = AHashSet::new();
    let mut queue = VecDeque::new();
    for &start in starts {
        if visited.insert(start) {
            queue.push_back((start, 0));
        }
    }
    let mut ordered = Vec::new();
    while let Some((node, level)) = queue.pop_front() {
        if level == depth {
            continue;
        }
        let neighbors = adjacency_for(graph, node, direction, None)?;
        for neighbor in neighbors {
            if visited.insert(neighbor) {
                ordered.push((level + 1, neighbor));
                queue.push_back((neighbor, level + 1));
            }
        }
    }
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(ordered.into_iter().map(|(_, node)| node).collect())
}

pub fn k_hop_filtered(
    graph: &SqliteGraph,
    start: i64,
    depth: u32,
    direction: BackendDirection,
    allowed_edge_types: &[&str],
) -> Result<Vec<i64>, SqliteGraphError> {
    if allowed_edge_types.is_empty() {
        return Ok(Vec::new());
    }
    k_hop_internal(graph, start, depth, direction, Some(allowed_edge_types))
}

pub fn chain_query(
    graph: &SqliteGraph,
    start: i64,
    chain: &[ChainStep],
) -> Result<Vec<i64>, SqliteGraphError> {
    if chain.is_empty() {
        return Ok(vec![start]);
    }
    let mut current = vec![start];
    for step in chain {
        let mut next = Vec::new();
        for node in &current {
            let neighbors = if let Some(edge_type) = step.edge_type.as_ref() {
                let mut allowed = AHashSet::with_capacity(1);
                allowed.insert(edge_type.as_str());
                adjacency_for(graph, *node, step.direction, Some(&allowed))?
            } else {
                adjacency_for(graph, *node, step.direction, None)?
            };
            next.extend(neighbors);
        }
        if next.is_empty() {
            return Ok(Vec::new());
        }
        next.sort();
        next.dedup();
        current = next;
    }
    Ok(current)
}

fn k_hop_internal(
    graph: &SqliteGraph,
    start: i64,
    depth: u32,
    direction: BackendDirection,
    allowed_edge_types: Option<&[&str]>,
) -> Result<Vec<i64>, SqliteGraphError> {
    if depth == 0 {
        return Ok(Vec::new());
    }
    let allowed_lookup = allowed_edge_types.map(build_lookup);
    let mut visited = AHashSet::new();
    let mut queue = VecDeque::new();
    let mut ordered = Vec::new();
    queue.push_back((start, 0));
    visited.insert(start);

    while let Some((node, level)) = queue.pop_front() {
        if level == depth {
            continue;
        }
        let neighbors = adjacency_for(graph, node, direction, allowed_lookup.as_ref())?;
        for neighbor in neighbors {
            if visited.insert(neighbor) {
                ordered.push((level + 1, neighbor));
                queue.push_back((neighbor, level + 1));
            }
        }
    }
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(ordered.into_iter().map(|(_, node)| node).collect())
}

fn adjacency_for(
    graph: &SqliteGraph,
    node: i64,
    direction: BackendDirection,
    allowed_types: Option<&AHashSet<&str>>,
) -> Result<Vec<i64>, SqliteGraphError> {
    typed_adjacency(graph, node, direction, allowed_types)
}

pub(crate) fn typed_adjacency(
    graph: &SqliteGraph,
    node: i64,
    direction: BackendDirection,
    allowed_types: Option<&AHashSet<&str>>,
) -> Result<Vec<i64>, SqliteGraphError> {
    match allowed_types {
        Some(set) => filter_neighbors(graph, node, direction, set),
        None => match direction {
            BackendDirection::Outgoing => graph.fetch_outgoing(node),
            BackendDirection::Incoming => graph.fetch_incoming(node),
        },
    }
}

fn filter_neighbors(
    graph: &SqliteGraph,
    node: i64,
    direction: BackendDirection,
    allowed_types: &AHashSet<&str>,
) -> Result<Vec<i64>, SqliteGraphError> {
    if allowed_types.is_empty() {
        return Ok(Vec::new());
    }
    let sql = match direction {
        BackendDirection::Outgoing => OUTGOING_TYPED_SQL,
        BackendDirection::Incoming => INCOMING_TYPED_SQL,
    };
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map([node], |row| {
            let neighbor: i64 = row.get(0)?;
            let edge_type: String = row.get(1)?;
            Ok((neighbor, edge_type))
        })
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let mut result = Vec::new();
    for row in rows {
        let (neighbor, edge_type) = row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
        if allowed_types.contains(edge_type.as_str()) && result.last().copied() != Some(neighbor) {
            result.push(neighbor);
        }
    }
    Ok(result)
}

pub(crate) fn build_lookup<'a>(types: &'a [&'a str]) -> AHashSet<&'a str> {
    let mut lookup = AHashSet::with_capacity(types.len());
    for ty in types {
        lookup.insert(*ty);
    }
    lookup
}
