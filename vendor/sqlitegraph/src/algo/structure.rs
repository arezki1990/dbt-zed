//! Structural analysis algorithms for graph topology.
//!
//! This module provides algorithms for analyzing the structural properties of graphs,
//! including connectivity, cycles, and degree distributions. These algorithms help
//! understand the overall shape and topology of a graph.
//!
//! # Available Algorithms
//!
//! - [`connected_components`] - Find all connected components in the graph
//! - [`find_cycles_limited`] - Enumerate cycles up to a limit
//! - [`nodes_by_degree`] - Rank nodes by degree (hub detection)
//!
//! # When to Use Structural Analysis
//!
//! - **Connected Components**: Graph connectivity analysis, finding isolated clusters,
//!   understanding graph fragmentation
//! - **Cycle Finding**: Detect feedback loops, find circular dependencies, analyze
//!   strongly connected components
//! - **Degree Ranking**: Find hub nodes, identify influential connectors, analyze
//!   network topology

use std::collections::VecDeque;

use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Finds all connected components in the graph using BFS.
///
/// A connected component is a maximal subgraph where any two nodes are connected
/// by a path. This function uses bidirectional BFS (both incoming and outgoing edges).
///
/// # Arguments
/// * `graph` - The graph to analyze
///
/// # Returns
/// Vector of components, where each component is a sorted vector of node IDs.
/// Components are sorted by their smallest node ID.
///
/// # Complexity
/// Time: O(|V| + |E|) - visits each node and edge once
/// Space: O(|V|) for visited set and BFS queue
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::connected_components};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let components = connected_components(&graph)?;
/// # Ok(())
/// # }
/// ```
pub fn connected_components(graph: &SqliteGraph) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    let mut components = Vec::new();
    let mut visited = ahash::AHashSet::new();
    for id in graph.all_entity_ids()? {
        if !visited.insert(id) {
            continue;
        }
        let mut queue = VecDeque::new();
        queue.push_back(id);
        let mut component = Vec::new();
        while let Some(node) = queue.pop_front() {
            component.push(node);
            for next in graph.fetch_outgoing(node)? {
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }
            for prev in graph.fetch_incoming(node)? {
                if visited.insert(prev) {
                    queue.push_back(prev);
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort_by(|a, b| a[0].cmp(&b[0]));
    Ok(components)
}

/// Finds cycles in the graph up to a specified limit.
///
/// Uses depth-first search to enumerate cycles starting from each node.
/// Cycles are normalized (rotated to start with smallest node) and deduplicated.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `limit` - Maximum number of cycles to find (0 returns empty result)
///
/// # Returns
/// Vector of cycles, where each cycle is a vector of node IDs starting and ending
/// with the same node. Cycles are sorted for determinism.
///
/// # Complexity
/// Time: O(limit * (|V| + |E|)) in practice, but worst-case exponential
/// Space: O(|V|) for DFS stack and cycle paths
///
/// # Caveats
/// - May return duplicate cycles in symmetric graphs
/// - Does not guarantee finding all cycles (stops at limit)
/// - Performance degrades on dense graphs with many cycles
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::find_cycles_limited};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let cycles = find_cycles_limited(&graph, 10)?;
/// # Ok(())
/// # }
/// ```
pub fn find_cycles_limited(
    graph: &SqliteGraph,
    limit: usize,
) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let all_ids = graph.all_entity_ids()?;
    let max_len = all_ids.len();
    let mut cycles = Vec::new();
    for &start in &all_ids {
        let mut stack = vec![(start, vec![start])];
        while let Some((node, path)) = stack.pop() {
            for next in graph.fetch_outgoing(node)? {
                if next == start && path.len() > 1 {
                    let mut cycle = path.clone();
                    cycle.push(start);
                    cycles.push(cycle);
                    if cycles.len() >= limit {
                        normalize_cycles(&mut cycles);
                        return Ok(cycles);
                    }
                    continue;
                }
                if path.contains(&next) {
                    continue;
                }
                let mut new_path = path.clone();
                new_path.push(next);
                if new_path.len() > max_len {
                    continue;
                }
                stack.push((next, new_path));
            }
        }
    }
    normalize_cycles(&mut cycles);
    Ok(cycles)
}

/// Computes node degrees (total number of incoming + outgoing edges).
///
/// Returns all nodes sorted by their degree, useful for finding hubs (high-degree nodes)
/// or isolates (zero-degree nodes) in the graph.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `descending` - If true, sort highest degree first; if false, sort lowest first
///
/// # Returns
/// Vector of (node_id, degree) tuples sorted by degree. Ties are broken by node ID.
///
/// # Complexity
/// Time: O(|V| + |E|) - visits each node and counts edges
/// Space: O(|V|) for degree storage
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::nodes_by_degree};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let degrees = nodes_by_degree(&graph, true)?;
/// # Ok(())
/// # }
/// ```
pub fn nodes_by_degree(
    graph: &SqliteGraph,
    descending: bool,
) -> Result<Vec<(i64, usize)>, SqliteGraphError> {
    let mut degrees = Vec::new();
    for id in graph.all_entity_ids()? {
        let outgoing = graph.fetch_outgoing(id)?.len();
        let incoming = graph.fetch_incoming(id)?.len();
        degrees.push((id, outgoing + incoming));
    }
    degrees.sort_by(|a, b| {
        if descending {
            b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))
        } else {
            a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0))
        }
    });
    Ok(degrees)
}

/// Normalizes cycles for deterministic output.
///
/// Rotates each cycle so it starts with the smallest node, then sorts
/// all cycles lexicographically.
fn normalize_cycles(cycles: &mut [Vec<i64>]) {
    for cycle in cycles.iter_mut() {
        // rotate so smallest node first for determinism
        if let Some((min_idx, _)) = cycle.iter().enumerate().min_by_key(|(_, value)| *value) {
            cycle.rotate_left(min_idx);
        }
    }
    cycles.sort();
}
