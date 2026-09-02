//! Strongly Connected Components (SCC) using Tarjan's algorithm.
//!
//! This module provides algorithms for finding strongly connected components
//! in directed graphs. A strongly connected component is a maximal subgraph
//! where every node can reach every other node.
//!
//! # Algorithm
//!
//! Uses Tarjan's single-pass DFS algorithm (O(|V| + |E|)):
//! - Assigns each node an index and lowlink value during DFS
//! - Maintains a stack to track nodes in the current SCC
//! - When lowlink equals index, we've found the root of an SCC
//! - Pops stack until root is popped to extract the SCC
//!
//! # When to Use SCC
//!
//! - **Loop Detection in CFGs**: Loops are SCCs with more than one node
//! - **Recursion Detection**: Find mutual recursion in call graphs
//! - **Cycle Detection**: Identify feedback loops in inference graphs
//! - **Graph Condensation**: Collapse SCCs to create condensed DAG
//! - **Topological Sort**: Prerequisite for sorting DAGs
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E|) - single DFS pass visits each node and edge once
//! - **Space**: O(|V|) for stack, indices, and lowlink maps
//!
//! # References
//!
//! - R. E. Tarjan, "Depth-First Search and Linear Graph Algorithms."
//!   *SIAM Journal on Computing*, 1972.
//! - https://en.wikipedia.org/wiki/Tarjan%27s_strongly_connected_components_algorithm

use std::collections::HashSet;

use ahash::{AHashMap, AHashSet};

use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Result of strongly connected components decomposition.
///
/// Contains the discovered SCCs, node-to-component mapping, and the
/// condensed DAG where each SCC becomes a supernode.
#[derive(Debug, Clone)]
pub struct SccResult {
    /// Each component is a set of nodes that are mutually reachable.
    /// Components are returned in reverse topological order (sinks first).
    pub components: Vec<HashSet<i64>>,

    /// Maps each node to its component index.
    /// Node -> component ID (0..components.len()-1)
    pub node_to_component: AHashMap<i64, usize>,

    /// The condensed DAG (each SCC becomes a supernode).
    /// Edge (i, j) exists if there's an edge from any node in SCC i
    /// to any node in SCC j (where i != j).
    /// Edges are sorted and deduplicated.
    pub condensed_edges: Vec<(usize, usize)>,
}

impl SccResult {
    /// Returns the number of non-trivial SCCs (components with more than one node).
    ///
    /// Non-trivial SCCs indicate cycles in the original graph.
    pub fn non_trivial_count(&self) -> usize {
        self.components.iter().filter(|c| c.len() > 1).count()
    }

    /// Returns all nodes in non-trivial SCCs (SCCs with more than one node).
    pub fn non_trivial_nodes(&self) -> AHashSet<i64> {
        self.components
            .iter()
            .filter(|c| c.len() > 1)
            .flat_map(|c| c.iter().copied())
            .collect()
    }

    /// Checks if a node is part of a non-trivial SCC (indicating a cycle).
    pub fn is_in_cycle(&self, node: i64) -> bool {
        if let Some(&component_idx) = self.node_to_component.get(&node) {
            self.components[component_idx].len() > 1
        } else {
            false
        }
    }
}

/// Computes strongly connected components using Tarjan's algorithm.
///
/// A strongly connected component (SCC) is a maximal subgraph where every
/// node can reach every other node. This function finds all SCCs in the graph
/// using Tarjan's single-pass DFS algorithm.
///
/// # Arguments
///
/// * `graph` - The graph to analyze
///
/// # Returns
///
/// `SccResult` containing:
/// - List of components (each component is a set of node IDs)
/// - Node-to-component mapping
/// - Condensed DAG edges (between SCCs)
///
/// # Components are returned in reverse topological order
///
/// The first components in the result are sink SCCs (no outgoing edges to other SCCs).
/// This is useful for algorithms that process components in topological order.
///
/// # Example
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::strongly_connected_components};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let scc = strongly_connected_components(&graph)?;
///
/// println!("Found {} SCCs", scc.components.len());
/// println!("Non-trivial SCCs (cycles): {}", scc.non_trivial_count());
/// # Ok(())
/// # }
/// ```
///
/// # Complexity
///
/// Time: O(|V| + |E|) - single DFS pass
/// Space: O(|V|) for stack, indices, and lowlink maps
///
/// # Edge Cases
///
/// - **Empty graph**: Returns empty SccResult
/// - **Single node**: One component with one node
/// - **Disconnected graph**: Multiple components (may all be trivial)
/// - **Linear chain**: Each node is its own SCC (all trivial)
/// - **Simple cycle**: One non-trivial SCC containing all nodes
pub fn strongly_connected_components(graph: &SqliteGraph) -> Result<SccResult, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        return Ok(SccResult {
            components: Vec::new(),
            node_to_component: AHashMap::new(),
            condensed_edges: Vec::new(),
        });
    }

    let mut index_counter: i64 = 0;
    let mut stack: Vec<i64> = Vec::new();
    let mut on_stack: AHashSet<i64> = AHashSet::new();
    let mut indices: AHashMap<i64, i64> = AHashMap::new();
    let mut lowlink: AHashMap<i64, i64> = AHashMap::new();
    let mut components: Vec<HashSet<i64>> = Vec::new();
    let mut node_to_component: AHashMap<i64, usize> = AHashMap::new();

    // Process each node
    for &node in &all_ids {
        if !indices.contains_key(&node) {
            strongconnect(
                graph,
                node,
                &mut index_counter,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut components,
                &mut node_to_component,
            )?;
        }
    }

    // Build condensed DAG edges
    let condensed_edges = build_condensed_dag(graph, &node_to_component, &components)?;

    Ok(SccResult {
        components,
        node_to_component,
        condensed_edges,
    })
}

/// Recursive helper for Tarjan's algorithm.
///
/// Performs DFS from the given node, assigning indices and computing lowlink values.
/// When a root SCC is found (lowlink == index), pops the stack to extract the component.
fn strongconnect(
    graph: &SqliteGraph,
    v: i64,
    index_counter: &mut i64,
    stack: &mut Vec<i64>,
    on_stack: &mut AHashSet<i64>,
    indices: &mut AHashMap<i64, i64>,
    lowlink: &mut AHashMap<i64, i64>,
    components: &mut Vec<HashSet<i64>>,
    node_to_component: &mut AHashMap<i64, usize>,
) -> Result<(), SqliteGraphError> {
    // Set the depth index for v to the smallest unused index
    indices.insert(v, *index_counter);
    lowlink.insert(v, *index_counter);
    *index_counter += 1;
    stack.push(v);
    on_stack.insert(v);

    // Consider successors of v
    for &w in &graph.fetch_outgoing(v)? {
        if !indices.contains_key(&w) {
            // Successor w has not yet been visited; recurse on it
            strongconnect(
                graph,
                w,
                index_counter,
                stack,
                on_stack,
                indices,
                lowlink,
                components,
                node_to_component,
            )?;
            lowlink.insert(
                v,
                (*lowlink
                    .get(&v)
                    .expect("invariant: v in lowlink from DFS entry"))
                .min(
                    *lowlink
                        .get(&w)
                        .expect("invariant: w in lowlink from DFS entry"),
                ),
            );
        } else if on_stack.contains(&w) {
            // Successor w is in stack S and hence in the current SCC
            lowlink.insert(
                v,
                (*lowlink
                    .get(&v)
                    .expect("invariant: v in lowlink from DFS entry"))
                .min(
                    *indices
                        .get(&w)
                        .expect("invariant: w has index from DFS visit"),
                ),
            );
        }
    }

    // If v is a root node, pop the stack and generate an SCC
    if lowlink.get(&v) == indices.get(&v) {
        let mut component = HashSet::new();
        loop {
            let w = stack
                .pop()
                .expect("invariant: stack non-empty when processing SCC root");
            on_stack.remove(&w);
            component.insert(w);
            node_to_component.insert(w, components.len());
            if w == v {
                break;
            }
        }
        components.push(component);
    }

    Ok(())
}

/// Build the condensed DAG from SCC decomposition.
///
/// The condensed DAG has one node per SCC. An edge (i, j) exists if there's
/// an edge from any node in SCC i to any node in SCC j (where i != j).
fn build_condensed_dag(
    graph: &SqliteGraph,
    node_to_component: &AHashMap<i64, usize>,
    _components: &[HashSet<i64>],
) -> Result<Vec<(usize, usize)>, SqliteGraphError> {
    let mut edge_set: AHashSet<(usize, usize)> = AHashSet::new();

    // For each edge in the original graph
    for &from_node in &graph.all_entity_ids()? {
        if let Some(&from_comp) = node_to_component.get(&from_node) {
            for &to_node in &graph.fetch_outgoing(from_node)? {
                if let Some(&to_comp) = node_to_component.get(&to_node)
                    && from_comp != to_comp
                {
                    edge_set.insert((from_comp, to_comp));
                }
            }
        }
    }

    // Convert to sorted vector for deterministic output
    let mut edges: Vec<(usize, usize)> = edge_set.into_iter().collect();
    edges.sort();
    edges.dedup();

    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GraphEntity;

    fn create_test_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create test entities
        for i in 0..10 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("test_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        graph
    }

    fn create_linear_chain_graph() -> SqliteGraph {
        let graph = create_test_graph();

        // Create linear chain: 0 -> 1 -> 2 -> 3 -> ... -> 9
        let entity_ids = graph.all_entity_ids().expect("Failed to get IDs");
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

    fn create_simple_cycle_graph() -> SqliteGraph {
        let graph = create_test_graph();

        let entity_ids = graph.all_entity_ids().expect("Failed to get IDs");
        // Create cycle: 0 -> 1 -> 2 -> 0
        let cycle = vec![(0, 1), (1, 2), (2, 0)];
        for (from_idx, to_idx) in cycle {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        graph
    }

    fn create_mutual_recursion_graph() -> SqliteGraph {
        let graph = create_test_graph();

        let entity_ids = graph.all_entity_ids().expect("Failed to get IDs");
        // Create mutual recursion: 0 <-> 1, and 2 -> 3 -> 4 (linear)
        let edges = vec![(0, 1), (1, 0), (2, 3), (3, 4)];
        for (from_idx, to_idx) in edges {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        graph
    }

    #[test]
    fn test_scc_empty_graph() {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let result = strongly_connected_components(&graph);

        assert!(result.is_ok());
        let scc = result.unwrap();
        assert_eq!(scc.components.len(), 0);
        assert_eq!(scc.node_to_component.len(), 0);
        assert_eq!(scc.condensed_edges.len(), 0);
    }

    #[test]
    fn test_scc_single_node() {
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

        let result = strongly_connected_components(&graph);
        assert!(result.is_ok());

        let scc = result.unwrap();
        assert_eq!(scc.components.len(), 1);
        assert_eq!(scc.node_to_component.len(), 1);
        assert_eq!(scc.components[0].len(), 1); // Single trivial SCC
        assert_eq!(scc.non_trivial_count(), 0);
    }

    #[test]
    fn test_scc_linear_chain() {
        let graph = create_linear_chain_graph();
        let result = strongly_connected_components(&graph);

        assert!(result.is_ok());
        let scc = result.unwrap();

        // Linear chain: each node is its own SCC
        assert_eq!(scc.components.len(), 10);
        assert_eq!(scc.node_to_component.len(), 10);

        // All SCCs are trivial (single node)
        assert_eq!(scc.non_trivial_count(), 0);

        // Condensed DAG should have 9 edges (chain)
        assert_eq!(scc.condensed_edges.len(), 9);
    }

    #[test]
    fn test_scc_simple_cycle() {
        let graph = create_simple_cycle_graph();
        let result = strongly_connected_components(&graph);

        assert!(result.is_ok());
        let scc = result.unwrap();

        // Cycle 0 -> 1 -> 2 -> 0: one SCC with 3 nodes
        // Plus 7 isolated nodes (3-9): total 8 SCCs
        assert_eq!(scc.components.len(), 8); // 1 cycle + 7 isolated nodes
        assert_eq!(scc.node_to_component.len(), 10);

        // Check that we have one non-trivial SCC
        assert_eq!(scc.non_trivial_count(), 1);

        // Find the cycle component
        let cycle_component = scc
            .components
            .iter()
            .find(|c| c.len() == 3)
            .expect("Should have a 3-node SCC");

        let entity_ids = graph.all_entity_ids().expect("Failed to get IDs");
        assert!(cycle_component.contains(&entity_ids[0]));
        assert!(cycle_component.contains(&entity_ids[1]));
        assert!(cycle_component.contains(&entity_ids[2]));

        // Verify cycle detection
        for node in cycle_component {
            assert!(scc.is_in_cycle(*node));
        }
    }

    #[test]
    fn test_scc_mutual_recursion() {
        let graph = create_mutual_recursion_graph();
        let result = strongly_connected_components(&graph);

        assert!(result.is_ok());
        let scc = result.unwrap();

        // 0 <-> 1: one SCC with 2 nodes
        // 2 -> 3 -> 4: each is its own SCC
        // 5, 6, 7, 8, 9: isolated nodes
        assert_eq!(scc.components.len(), 9);
        assert_eq!(scc.non_trivial_count(), 1);

        // Verify the mutual recursion component has 2 nodes
        let recursion_component = scc
            .components
            .iter()
            .find(|c| c.len() == 2)
            .expect("Should have a 2-node SCC");

        assert_eq!(recursion_component.len(), 2);
    }

    #[test]
    fn test_scc_condensed_dag() {
        let graph = create_mutual_recursion_graph();
        let result = strongly_connected_components(&graph);

        assert!(result.is_ok());
        let scc = result.unwrap();

        // Condensed DAG should be acyclic
        // Check for no self-loops in condensed DAG
        for &(from, to) in &scc.condensed_edges {
            assert_ne!(from, to, "Condensed DAG should not have self-loops");
        }

        // Verify condensed edges connect different SCCs
        for &(from, to) in &scc.condensed_edges {
            assert!(from < scc.components.len());
            assert!(to < scc.components.len());
        }
    }
}
