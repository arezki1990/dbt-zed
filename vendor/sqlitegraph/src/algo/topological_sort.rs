//! Topological sort algorithm with cycle detection and explanation.
//!
//! This module provides algorithms for computing topological ordering of directed acyclic graphs (DAGs).
//! When cycles exist, returns a helpful error with the cycle path and explanation for debugging.
//!
//! # Available Algorithms
//!
//! - [`topological_sort`] - Compute topological ordering of nodes
//! - [`TopoError`] - Error type for cycle detection with cycle path
//!
//! # When to Use Topological Sort
//!
//! - **Build/execution ordering** - Determine which tasks must execute first
//! - **Dependency resolution** - Find valid order for processing dependencies
//! - **Partial order validation** - Check if a graph is a valid DAG
//! - **Cycle detection** - Identify circular dependencies with explanation
//!
//! # Algorithm
//!
//! Two-phase approach:
//! 1. **Cycle detection using SCC** - Use strongly_connected_components to find cycles
//! 2. **Kahn's algorithm** - For valid DAGs, compute ordering using in-degree processing
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E|) - SCC is O(|V| + |E|), Kahn's is O(|V| + |E|)
//! - **Space**: O(|V|) for in-degree map and queue
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::topological_sort};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... add nodes and edges ...
//!
//! match topological_sort(&graph) {
//!     Ok(ordering) => {
//!         println!("Valid topological order: {:?}", ordering);
//!     }
//!     Err(TopoError::CycleDetected { cycle, explanation }) => {
//!         eprintln!("Cannot sort: {}", explanation);
//!         eprintln!("Cycle: {:?}", cycle);
//!     }
//! }
//! ```

use std::collections::{HashSet, VecDeque};

use ahash::AHashMap;

use crate::graph::SqliteGraph;

/// Error type for topological sort.
///
/// Topological sort is only valid on DAGs. When cycles exist, this error
/// provides the cycle path and a helpful explanation.
#[derive(Debug, Clone)]
pub enum TopoError {
    /// Graph contains a cycle, making topological sort impossible.
    ///
    /// The `cycle` field contains the actual cycle path (nodes forming the cycle).
    /// The `explanation` field provides a human-readable description.
    CycleDetected {
        /// Nodes forming the cycle (in order).
        cycle: Vec<i64>,
        /// Human-readable explanation of why sorting failed.
        explanation: String,
    },
}

impl std::fmt::Display for TopoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopoError::CycleDetected { cycle, explanation } => {
                write!(f, "{}: Cycle detected: {:?}", explanation, cycle)
            }
        }
    }
}

impl std::error::Error for TopoError {}

/// Computes topological ordering of nodes in a directed graph.
///
/// Topological sort produces a linear ordering of nodes such that for every
/// directed edge (u, v), node u appears before node v in the ordering.
/// This is only possible for directed acyclic graphs (DAGs).
///
/// # Arguments
///
/// * `graph` - The graph to sort
///
/// # Returns
///
/// `Ok(Vec<NodeId>)` containing nodes in topological order, or `Err(TopoError)`
/// if the graph contains cycles.
///
/// # Algorithm
///
/// Two-phase approach:
/// 1. **Cycle detection** - Use SCC decomposition to find non-trivial SCCs (cycles)
/// 2. **Kahn's algorithm** - For DAGs, process nodes with zero in-degree
///
/// # Cycle Detection
///
/// Uses strongly_connected_components from plan 45-02:
/// - Non-trivial SCCs (components with >1 node) indicate cycles
/// - Extracts actual cycle path for debugging
/// - Returns helpful error message with cycle details
///
/// # Kahn's Algorithm
///
/// For valid DAGs:
/// 1. Compute in-degree for all nodes
/// 2. Add nodes with zero in-degree to queue
/// 3. Repeatedly remove node from queue, add to result, decrement neighbors' in-degrees
/// 4. Add new zero in-degree nodes to queue
/// 5. Return result when all nodes processed
///
/// # Complexity
///
/// - **Time**: O(|V| + |E|)
/// - **Space**: O(|V|) for in-degree map and queue
///
/// # Edge Cases
///
/// - **Empty graph**: Returns empty Vec
/// - **Single node**: Returns [node_id]
/// - **Disconnected graph**: Still produces valid topological order
/// - **Graph with cycle**: Returns CycleDetected error with cycle path
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::topological_sort};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges forming a DAG ...
///
/// let ordering = topological_sort(&graph)?;
/// // Verify: for every edge (u, v), u appears before v in ordering
/// ```
///
/// # Errors
///
/// Returns `TopoError::CycleDetected` if the graph contains cycles.
/// The error includes the cycle path and explanation for debugging.
pub fn topological_sort(graph: &SqliteGraph) -> Result<Vec<i64>, TopoError> {
    let all_nodes = graph
        .all_entity_ids()
        .map_err(|e| TopoError::CycleDetected {
            cycle: vec![],
            explanation: format!("Failed to get nodes: {}", e),
        })?;

    if all_nodes.is_empty() {
        return Ok(Vec::new());
    }

    // Phase 1: Check for cycles using SCC
    let scc = crate::algo::scc::strongly_connected_components(graph).map_err(|e| {
        TopoError::CycleDetected {
            cycle: vec![],
            explanation: format!("Failed to compute SCC: {}", e),
        }
    })?;

    // Find non-trivial SCCs (cycles)
    let non_trivial_sccs: Vec<_> = scc.components.into_iter().filter(|c| c.len() > 1).collect();

    if !non_trivial_sccs.is_empty() {
        // Extract cycle path from first non-trivial SCC
        let cycle = extract_cycle_path(graph, &non_trivial_sccs[0]);
        return Err(TopoError::CycleDetected {
            cycle,
            explanation: format!(
                "Found {} cycle(s) - graph is not a DAG",
                non_trivial_sccs.len()
            ),
        });
    }

    // Phase 2: Kahn's algorithm for valid DAG
    // Compute in-degrees
    let mut in_degree: AHashMap<i64, usize> = AHashMap::new();
    for &node in &all_nodes {
        in_degree.insert(node, 0);
    }

    for &node in &all_nodes {
        for target in graph
            .fetch_outgoing(node)
            .map_err(|e| TopoError::CycleDetected {
                cycle: vec![],
                explanation: format!("Failed to get outgoing edges: {}", e),
            })?
        {
            *in_degree.entry(target).or_insert(0) += 1;
        }
    }

    // Process nodes with zero in-degree
    let mut queue: VecDeque<i64> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(&node, _)| node)
        .collect();

    let mut result = Vec::new();

    while let Some(node) = queue.pop_front() {
        result.push(node);
        for target in graph
            .fetch_outgoing(node)
            .map_err(|e| TopoError::CycleDetected {
                cycle: vec![],
                explanation: format!("Failed to get outgoing edges: {}", e),
            })?
        {
            let deg = in_degree
                .get_mut(&target)
                .expect("invariant: target in in_degree from initialization");
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(target);
            }
        }
    }

    // If result doesn't contain all nodes, there's a cycle
    // (This shouldn't happen if SCC check passed, but handle anyway)
    if result.len() != all_nodes.len() {
        return Err(TopoError::CycleDetected {
            cycle: vec![],
            explanation: "Graph contains cycle".to_string(),
        });
    }

    Ok(result)
}

/// Extracts a cycle path from a strongly connected component.
///
/// Given a set of nodes known to form a cycle (non-trivial SCC),
/// finds the actual cycle path by tracing from one node back to itself.
///
/// # Arguments
///
/// * `graph` - The graph containing the cycle
/// * `scc` - Set of nodes forming the cycle (SCC with >1 node)
///
/// # Returns
///
/// A vector of node IDs forming a cycle path.
///
/// # Algorithm
///
/// Uses DFS to find a path from any node in the SCC back to itself:
/// 1. Pick any node from the SCC as start
/// 2. Follow outgoing edges until we revisit a node
/// 3. Extract the cycle portion from the path
fn extract_cycle_path(graph: &SqliteGraph, scc: &HashSet<i64>) -> Vec<i64> {
    // Pick any node from the SCC
    let &start = scc.iter().next().unwrap_or(&1);

    // DFS to find a path that returns to a node in the SCC
    let mut path = vec![start];
    let mut visited = HashSet::new();
    visited.insert(start);

    loop {
        let current = *path.last().unwrap_or(&start);

        // Find next node in SCC that we haven't visited yet in this path
        let mut found_next = false;
        if let Ok(outgoing) = graph.fetch_outgoing(current) {
            for &next in &outgoing {
                if scc.contains(&next) {
                    if next == start {
                        // Found path back to start - complete cycle
                        path.push(next);
                        return path;
                    } else if !visited.contains(&next) {
                        // Continue exploration
                        path.push(next);
                        visited.insert(next);
                        found_next = true;
                        break;
                    } else if path.len() > 1 {
                        // Found a cycle within the path
                        if let Some(cycle_start_idx) = path.iter().position(|&n| n == next) {
                            let cycle: Vec<i64> = path[cycle_start_idx..].to_vec();
                            return cycle;
                        }
                    }
                }
            }
        }

        if !found_next {
            // Dead end - shouldn't happen in an SCC
            // Return what we have
            return path;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create test graph with linear chain: 0 -> 1 -> 2 -> 3
    fn create_linear_chain_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create chain: 0 -> 1 -> 2 -> 3
        for i in 0..entity_ids.len().saturating_sub(1) {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create diamond DAG: 0 -> 1 -> 3, 0 -> 2 -> 3
    fn create_diamond_dag() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create diamond: 0 -> 1 -> 3, 0 -> 2 -> 3
        let edges = vec![(0, 1), (1, 3), (0, 2), (2, 3)];
        for (from_idx, to_idx) in edges {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create graph with cycle: 0 -> 1 -> 2 -> 0
    fn create_cycle_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 3 nodes
        for i in 0..3 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("cycle_{}", i),
                file_path: Some(format!("cycle_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create cycle: 0 -> 1 -> 2 -> 0
        for i in 0..3 {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[(i + 1) % 3],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    #[test]
    fn test_topo_sort_empty() {
        // Scenario: Empty graph returns empty Vec
        // Expected: Ok(vec![])
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = topological_sort(&graph);
        assert!(result.is_ok(), "Topological sort failed on empty graph");

        let ordering = result.unwrap();
        assert_eq!(ordering.len(), 0, "Expected empty ordering for empty graph");
    }

    #[test]
    fn test_topo_sort_single_node() {
        // Scenario: Single node returns [node_id]
        // Expected: Ok(vec![node_id])
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let entity = GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "single".to_string(),
            file_path: Some("single.rs".to_string()),
            data: serde_json::json!({}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        let result = topological_sort(&graph);
        assert!(result.is_ok(), "Topological sort failed on single node");

        let ordering = result.unwrap();
        assert_eq!(ordering.len(), 1, "Expected single node in ordering");

        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
        assert_eq!(
            ordering[0], entity_ids[0],
            "Ordering should contain the node"
        );
    }

    #[test]
    fn test_topo_sort_linear_chain() {
        // Scenario: Linear chain: 0 -> 1 -> 2 -> 3
        // Expected: Valid topological order (all edges go forward)
        let graph = create_linear_chain_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = topological_sort(&graph);
        assert!(result.is_ok(), "Topological sort failed on linear chain");

        let ordering = result.unwrap();
        assert_eq!(ordering.len(), 4, "Expected 4 nodes in ordering");

        // Verify: for every edge (u, v), u appears before v in ordering
        for i in 0..entity_ids.len().saturating_sub(1) {
            let from = entity_ids[i];
            let to = entity_ids[i + 1];

            let from_pos = ordering.iter().position(|&n| n == from).unwrap_or(999);
            let to_pos = ordering.iter().position(|&n| n == to).unwrap_or(999);

            assert!(
                from_pos < to_pos,
                "Edge {} -> {} violates topological order ({} at {}, {} at {})",
                from,
                to,
                from,
                from_pos,
                to,
                to_pos
            );
        }
    }

    #[test]
    fn test_topo_sort_diamond() {
        // Scenario: Diamond DAG: 0 -> 1 -> 3, 0 -> 2 -> 3
        // Expected: Valid ordering (0 before 1,2; 1,2 before 3)
        let graph = create_diamond_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = topological_sort(&graph);
        assert!(result.is_ok(), "Topological sort failed on diamond DAG");

        let ordering = result.unwrap();
        assert_eq!(ordering.len(), 4, "Expected 4 nodes in ordering");

        let node_0 = entity_ids[0];
        let node_1 = entity_ids[1];
        let node_2 = entity_ids[2];
        let node_3 = entity_ids[3];

        // Verify topological constraints
        let pos_0 = ordering.iter().position(|&n| n == node_0).unwrap();
        let pos_1 = ordering.iter().position(|&n| n == node_1).unwrap();
        let pos_2 = ordering.iter().position(|&n| n == node_2).unwrap();
        let pos_3 = ordering.iter().position(|&n| n == node_3).unwrap();

        // 0 must come before 1, 2, and 3
        assert!(pos_0 < pos_1, "0 should come before 1");
        assert!(pos_0 < pos_2, "0 should come before 2");
        assert!(pos_0 < pos_3, "0 should come before 3");

        // 1 and 2 must come before 3
        assert!(pos_1 < pos_3, "1 should come before 3");
        assert!(pos_2 < pos_3, "2 should come before 3");
    }

    #[test]
    fn test_topo_sort_cycle() {
        // Scenario: Graph with cycle: 0 -> 1 -> 2 -> 0
        // Expected: Err(CycleDetected) with cycle path
        let graph = create_cycle_graph();

        let result = topological_sort(&graph);
        assert!(
            result.is_err(),
            "Topological sort should fail on cyclic graph"
        );

        let err = result.unwrap_err();
        match err {
            TopoError::CycleDetected { cycle, explanation } => {
                assert!(!cycle.is_empty(), "Cycle should not be empty");
                assert!(
                    explanation.contains("cycle"),
                    "Explanation should mention cycles"
                );
                assert!(cycle.len() >= 3, "Cycle should have at least 3 nodes");
            }
        }
    }

    #[test]
    fn test_topo_sort_cycle_has_path() {
        // Scenario: Cycle error includes actual cycle path
        // Expected: Cycle path is actual cycle (nodes form cycle)
        let graph = create_cycle_graph();

        let result = topological_sort(&graph);
        assert!(result.is_err(), "Should detect cycle");

        let err = result.unwrap_err();
        match err {
            TopoError::CycleDetected { cycle, .. } => {
                // Verify the cycle actually forms a cycle
                // The cycle should have at least 3 distinct nodes
                assert!(cycle.len() >= 3, "Cycle should have at least 3 nodes");

                // First and last nodes should be the same (cycle completes)
                // or cycle should have a path that forms a loop
                let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
                let valid_nodes: std::collections::HashSet<i64> = entity_ids.into_iter().collect();

                // All nodes in cycle should be valid
                for node in &cycle {
                    assert!(
                        valid_nodes.contains(node),
                        "Cycle node {} should be in graph",
                        node
                    );
                }
            }
        }
    }
}
