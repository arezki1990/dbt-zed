use std::collections::VecDeque;

use ahash::{AHashMap, AHashSet};

use crate::{
    backend::BackendDirection,
    errors::SqliteGraphError,
    graph::SqliteGraph,
    multi_hop::{build_lookup, typed_adjacency},
};

pub fn bfs_neighbors(
    graph: &SqliteGraph,
    start: i64,
    max_depth: u32,
) -> Result<Vec<i64>, SqliteGraphError> {
    graph.get_entity(start)?;
    let mut visited = Vec::new();
    let mut seen = AHashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back((start, 0));
    seen.insert(start);
    while let Some((node, depth)) = queue.pop_front() {
        visited.push(node);
        if depth >= max_depth {
            continue;
        }
        for &next in graph.fetch_outgoing_shared(node)?.iter() {
            if seen.insert(next) {
                queue.push_back((next, depth + 1));
            }
        }
    }
    Ok(visited)
}

pub fn bfs_neighbors_filtered(
    graph: &SqliteGraph,
    start: i64,
    max_depth: u32,
    allowed_edge_types: &[&str],
    direction: BackendDirection,
) -> Result<Vec<i64>, SqliteGraphError> {
    graph.get_entity(start)?;
    if allowed_edge_types.is_empty() {
        return Ok(Vec::new());
    }
    let lookup = build_lookup(allowed_edge_types);
    let mut visited = Vec::new();
    let mut seen = AHashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back((start, 0));
    seen.insert(start);
    while let Some((node, depth)) = queue.pop_front() {
        visited.push(node);
        if depth >= max_depth {
            continue;
        }
        for next in typed_adjacency(graph, node, direction, Some(&lookup))? {
            if seen.insert(next) {
                queue.push_back((next, depth + 1));
            }
        }
    }
    Ok(visited)
}

pub fn shortest_path(
    graph: &SqliteGraph,
    start: i64,
    end: i64,
) -> Result<Option<Vec<i64>>, SqliteGraphError> {
    graph.get_entity(start)?;
    graph.get_entity(end)?;
    if start == end {
        return Ok(Some(vec![start]));
    }
    let mut queue = VecDeque::new();
    let mut parents = AHashMap::new();
    let mut seen = AHashSet::new();
    queue.push_back(start);
    seen.insert(start);
    let mut found = false;
    while let Some(node) = queue.pop_front() {
        for &next in graph.fetch_outgoing_shared(node)?.iter() {
            if seen.insert(next) {
                parents.insert(next, node);
                if next == end {
                    found = true;
                    break;
                }
                queue.push_back(next);
            }
        }
        if found {
            break;
        }
    }
    if !found {
        return Ok(None);
    }
    let mut path = vec![end];
    let mut current = end;
    while let Some(&parent) = parents.get(&current) {
        path.push(parent);
        if parent == start {
            break;
        }
        current = parent;
    }
    if path.last().map(|last| *last != start).unwrap_or(true) {
        return Ok(None);
    }
    path.reverse();
    Ok(Some(path))
}

pub fn shortest_path_filtered(
    graph: &SqliteGraph,
    start: i64,
    end: i64,
    allowed_edge_types: &[&str],
) -> Result<Option<Vec<i64>>, SqliteGraphError> {
    graph.get_entity(start)?;
    graph.get_entity(end)?;
    if start == end {
        return Ok(Some(vec![start]));
    }
    if allowed_edge_types.is_empty() {
        return Ok(None);
    }
    let lookup = build_lookup(allowed_edge_types);
    let mut queue = VecDeque::new();
    let mut parents = AHashMap::new();
    let mut seen = AHashSet::new();
    queue.push_back(start);
    seen.insert(start);
    let mut found = false;
    while let Some(node) = queue.pop_front() {
        for next in typed_adjacency(graph, node, BackendDirection::Outgoing, Some(&lookup))? {
            if seen.insert(next) {
                parents.insert(next, node);
                if next == end {
                    found = true;
                    break;
                }
                queue.push_back(next);
            }
        }
        if found {
            break;
        }
    }
    if !found {
        return Ok(None);
    }
    let mut path = vec![end];
    let mut current = end;
    while let Some(&parent) = parents.get(&current) {
        path.push(parent);
        if parent == start {
            break;
        }
        current = parent;
    }
    if path.last().map(|last| *last != start).unwrap_or(true) {
        return Ok(None);
    }
    path.reverse();
    Ok(Some(path))
}
