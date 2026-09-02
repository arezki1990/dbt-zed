//! Weakly Connected Components (WCC) algorithm for undirected connectivity analysis.
//!
//! This module provides algorithms for finding weakly connected components in directed graphs.
//! A weakly connected component is a maximal subgraph where all nodes are connected
//! when treating all edges as undirected.
//!
//! # When to Use WCC
//!
//! - **Dependency Clustering**: Find groups of interconnected dependencies
//! - **Graph Fragmentation**: Identify isolated subgraphs
//! - **Reachability Analysis**: Understand undirected connectivity structure
//! - **Preprocessing**: Step before algorithms that require connected graphs
//!
//! # WCC vs SCC vs Connected Components
//!
//! | Algorithm | Edge Treatment | Use Case |
//! |-----------|---------------|----------|
//! | **Weakly Connected Components** | Treats edges as bidirectional | Undirected connectivity analysis |
//! | **Strongly Connected Components** | Respects edge direction | Finds cycles and mutual reachability |
//! | **Connected Components** | Treats edges as bidirectional | Undirected graphs |
//!
//! # Algorithm
//!
//! Uses bidirectional breadth-first search (BFS):
//!
//! 1. Start with an unvisited node
//! 2. Explore all reachable nodes using both outgoing AND incoming edges
//! 3. All visited nodes form one weakly connected component
//! 4. Repeat until all nodes are visited
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E|) - visits each node and edge once
//! - **Space**: O(|V|) for visited set and BFS queue
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::weakly_connected_components};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... add nodes and edges ...
//!
//! let components = weakly_connected_components(&graph)?;
//!
//! for (i, component) in components.iter().enumerate() {
//!     println!("Component {}: {} nodes", i, component.len());
//! }
//! ```

use std::collections::VecDeque;

use crate::progress::ProgressCallback;
use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Finds all weakly connected components in the graph using bidirectional BFS.
///
/// A weakly connected component is a maximal subgraph where any two nodes are connected
/// by a path when ignoring edge direction. This function treats all edges as bidirectional.
///
/// # Arguments
/// * `graph` - The graph to analyze
///
/// # Returns
/// Vector of components, where each component is a sorted vector of node IDs.
/// Components are sorted by their smallest node ID for deterministic output.
///
/// # Complexity
/// Time: O(|V| + |E|) - visits each node and edge once
/// Space: O(|V|) for visited set and BFS queue
///
/// # Edge Cases
/// - Empty graph: Returns empty vector
/// - Single node: Returns [[node_id]]
/// - Disconnected graph: Returns multiple components
///
/// # Example
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::weakly_connected_components};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// let components = weakly_connected_components(&graph)?;
///
/// // Each component is a sorted vector of node IDs
/// for (i, component) in components.iter().enumerate() {
///     println!("Component {}: {:?}", i, component);
/// }
/// ```
pub fn weakly_connected_components(graph: &SqliteGraph) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    let mut components = Vec::new();
    let mut visited = ahash::AHashSet::new();

    // Get all node IDs
    let all_ids = graph.all_entity_ids()?;

    // Process each unvisited node
    for id in all_ids {
        if !visited.insert(id) {
            continue; // Already visited
        }

        // Start BFS from this node
        let mut queue = VecDeque::new();
        queue.push_back(id);
        let mut component = Vec::new();

        // Bidirectional BFS - explore both outgoing and incoming edges
        while let Some(node) = queue.pop_front() {
            component.push(node);

            // Explore outgoing edges
            for next in graph.fetch_outgoing(node)? {
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }

            // Explore incoming edges
            for prev in graph.fetch_incoming(node)? {
                if visited.insert(prev) {
                    queue.push_back(prev);
                }
            }
        }

        // Sort component for deterministic output
        component.sort();
        components.push(component);
    }

    // Sort components by their first element for deterministic output
    components.sort_by(|a, b| a.first().cmp(&b.first()));

    Ok(components)
}

/// Finds all weakly connected components with progress tracking.
///
/// Same algorithm as [`weakly_connected_components`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// Vector of components, where each component is a sorted vector of node IDs.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current node being processed (1-indexed)
/// - `total`: Total number of nodes in the graph
/// - `message`: "Finding weakly connected components"
///
/// # Example
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::weakly_connected_components_with_progress,
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let components = weakly_connected_components_with_progress(&graph, &progress)?;
/// // Output: Finding weakly connected components [10/100]...
/// ```
pub fn weakly_connected_components_with_progress<F>(
    graph: &SqliteGraph,
    progress: &F,
) -> Result<Vec<Vec<i64>>, SqliteGraphError>
where
    F: ProgressCallback,
{
    let mut components = Vec::new();
    let mut visited = ahash::AHashSet::new();

    // Get all node IDs
    let all_ids = graph.all_entity_ids()?;
    let total = all_ids.len();

    // Process each unvisited node
    for (idx, id) in all_ids.iter().enumerate() {
        if !visited.insert(*id) {
            continue; // Already visited
        }

        // Report progress
        progress.on_progress(idx + 1, Some(total), "Finding weakly connected components");

        // Start BFS from this node
        let mut queue = VecDeque::new();
        queue.push_back(*id);
        let mut component = Vec::new();

        // Bidirectional BFS - explore both outgoing and incoming edges
        while let Some(node) = queue.pop_front() {
            component.push(node);

            // Explore outgoing edges
            for next in graph.fetch_outgoing(node)? {
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }

            // Explore incoming edges
            for prev in graph.fetch_incoming(node)? {
                if visited.insert(prev) {
                    queue.push_back(prev);
                }
            }
        }

        // Sort component for deterministic output
        component.sort();
        components.push(component);
    }

    // Sort components by their first element for deterministic output
    components.sort_by(|a, b| a.first().cmp(&b.first()));

    // Report completion
    progress.on_complete();

    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GraphEntity;

    /// Helper to create a test graph with a linear chain: 0 -> 1 -> 2 -> 3
    fn create_linear_chain_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create nodes 0-3
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        // Get entity IDs and create edges: 0 -> 1 -> 2 -> 3
        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
        for i in 0..entity_ids.len().saturating_sub(1) {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        graph
    }

    /// Helper to create a disconnected graph: 0 -> 1 and 2 -> 3 (two components)
    fn create_disconnected_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create nodes 0-3
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        // Get entity IDs and create two disconnected chains: 0 -> 1 and 2 -> 3
        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

        // First chain: 0 -> 1
        let edge1 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge1).ok();

        // Second chain: 2 -> 3
        let edge2 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[2],
            to_id: entity_ids[3],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge2).ok();

        graph
    }

    #[test]
    fn test_wcc_empty_graph() {
        // Scenario: WCC on empty graph
        // Expected: Returns empty vector
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = weakly_connected_components(&graph);
        assert!(result.is_ok(), "WCC failed on empty graph");

        let components = result.unwrap();
        assert_eq!(components.len(), 0, "Expected 0 components in empty graph");
    }

    #[test]
    fn test_wcc_single_node() {
        // Scenario: WCC on graph with single node
        // Expected: Returns [[node_id]]
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: "single_node".to_string(),
            file_path: Some("single_node.rs".to_string()),
            data: serde_json::json!({}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        let result = weakly_connected_components(&graph);
        assert!(result.is_ok(), "WCC failed on single node");

        let components = result.unwrap();
        assert_eq!(components.len(), 1, "Expected 1 component");
        assert_eq!(components[0].len(), 1, "Expected 1 node in component");
    }

    #[test]
    fn test_wcc_linear_chain() {
        // Scenario: WCC on linear chain 0 -> 1 -> 2 -> 3
        // Expected: All nodes in one component (edges are bidirectional)
        let graph = create_linear_chain_graph();

        let result = weakly_connected_components(&graph);
        assert!(result.is_ok(), "WCC failed on linear chain");

        let components = result.unwrap();
        assert_eq!(components.len(), 1, "Expected 1 component in linear chain");
        assert_eq!(
            components[0].len(),
            4,
            "Expected all 4 nodes in single component"
        );

        // Verify all nodes appear exactly once
        let all_nodes: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let component_nodes: Vec<i64> = components[0].clone();
        assert_eq!(
            all_nodes.len(),
            component_nodes.len(),
            "Mismatch in node count"
        );
    }

    #[test]
    fn test_wcc_disconnected() {
        // Scenario: WCC on disconnected graph: 0 -> 1 and 2 -> 3
        // Expected: Two components, each with 2 nodes
        let graph = create_disconnected_graph();

        let result = weakly_connected_components(&graph);
        assert!(result.is_ok(), "WCC failed on disconnected graph");

        let components = result.unwrap();
        assert_eq!(
            components.len(),
            2,
            "Expected 2 components in disconnected graph"
        );

        // Each component should have 2 nodes
        assert_eq!(
            components[0].len(),
            2,
            "First component should have 2 nodes"
        );
        assert_eq!(
            components[1].len(),
            2,
            "Second component should have 2 nodes"
        );

        // Verify all nodes appear exactly once across all components
        let all_nodes: i64 = graph.list_entity_ids().expect("Failed to get IDs").len() as i64;
        let component_nodes: i64 = components.iter().map(|c| c.len() as i64).sum();
        assert_eq!(all_nodes, component_nodes, "Not all nodes accounted for");
    }

    #[test]
    fn test_wcc_with_progress() {
        // Scenario: WCC with progress callback
        // Expected: Progress callback is called, results match non-progress version
        use crate::progress::NoProgress;

        let graph = create_linear_chain_graph();

        let progress = NoProgress;
        let result =
            weakly_connected_components_with_progress(&graph, &progress).expect("WCC failed");

        let result_no_progress =
            weakly_connected_components(&graph).expect("WCC without progress failed");

        // Results should be identical
        assert_eq!(
            result.len(),
            result_no_progress.len(),
            "Component count mismatch"
        );
        for (comp_with, comp_without) in result.iter().zip(result_no_progress.iter()) {
            assert_eq!(comp_with, comp_without, "Component mismatch");
        }
    }

    #[test]
    fn test_wcc_deterministic_ordering() {
        // Scenario: WCC produces deterministic output
        // Expected: Multiple calls produce same component ordering
        let graph = create_disconnected_graph();

        let result1 = weakly_connected_components(&graph).expect("First WCC failed");
        let result2 = weakly_connected_components(&graph).expect("Second WCC failed");

        // Results should be identical (same ordering)
        assert_eq!(result1, result2, "WCC results are non-deterministic");
    }
}
