//! Call graph analysis for program dependence and cyclic region detection.
//!
//! This module provides algorithms for analyzing call graphs and other directed
//! graphs where cycles indicate mutual recursion or circular dependencies. The key
//! operation is SCC (Strongly Connected Component) collapse, which transforms
//! cyclic graphs into Directed Acyclic Graphs (DAGs) for easier analysis.
//!
//! # SCC Collapse (Graph Condensation)
//!
//! The core operation collapses each strongly connected component into a single
//! "supernode" and builds edges between supernodes. This produces the **condensation
//! graph**, which is always a DAG by the fundamental property of SCC decomposition.
//!
//! ## Why Collapse SCCs?
//!
//! - **Mutual Recursion**: Functions that call each other directly or indirectly
//! - **Circular Dependencies**: Modules that depend on each other
//! - **Cycle Detection**: Non-trivial SCCs indicate cycles in the original graph
//! - **Topological Analysis**: Condensation graph enables topological sorting
//! - **Visualization**: Simplifies complex graphs by merging cyclic regions
//!
//! ## Condensation Graph Properties
//!
//! - **Acyclic**: No edges from a supernode to itself (by definition)
//! - **DAG**: Always suitable for topological sorting
//! - **Hierarchical**: Preserves reachability relationships between SCCs
//! - **Reversible**: Bidirectional mappings enable original node lookup
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E|) - one SCC pass + one edge enumeration
//! - **Space**: O(|V|) for node mappings and condensed edges
//!
//! Where:
//! - V = number of vertices
//! - E = number of edges
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::collapse_sccs};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... build call graph with mutual recursion ...
//!
//! let collapsed = collapse_sccs(&graph)?;
//!
//! println!("Found {} SCCs", collapsed.num_sccs);
//! println!("Condensed graph has {} edges", collapsed.supernode_edges.len());
//!
//! // Find which SCC a node belongs to
//! if let Some(supernode) = collapsed.supernode_for(node_id) {
//!     println!("Node {} is in SCC {}", node_id, supernode);
//! }
//!
//! // Get all nodes in an SCC
//! if let Some(members) = collapsed.members_of(supernode) {
//!     println!("SCC {} has {} members", supernode, members.len());
//! }
//! ```
//!
//! # Use Cases
//!
//! ## Call Graph Analysis
//!
//! - **Mutual Recursion Detection**: Find functions that call each other
//! - **Dependency Clusters**: Group mutually dependent functions
//! - **Refactoring Safety**: Check if changing one function affects others
//!
//! ## Dependency Analysis
//!
//! - **Circular Dependencies**: Detect module circular dependencies
//! - **Build Order**: Topological sort on condensation DAG
//! - **Impact Analysis**: What gets affected by changing an SCC?
//!
//! ## Program Visualization
//!
//! - **Graph Simplification**: Collapse cycles for cleaner diagrams
//! - **Cluster Detection**: SCCs represent tightly-coupled regions
//! - **Hierarchical Views**: Multi-level zoom (original vs condensed)
//!
//! # References
//!
//! - CP-Algorithms: Strongly Connected Components and Condensation Graph
//!   https://cp-algorithms.com/graph/strongly-connected-components.html
//! - Tarjan, R. E. "Depth-First Search and Linear Graph Algorithms." SIAM 1972

use ahash::{AHashMap, AHashSet};

use crate::errors::SqliteGraphError;
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

use super::scc::strongly_connected_components;

/// Result of SCC collapse operation (condensation graph).
///
/// Contains the bidirectional mappings between original nodes and supernodes,
/// plus the condensed DAG edges between supernodes.
///
/// # Fields
///
/// - `node_to_supernode`: Maps each original node ID to its SCC supernode ID
/// - `supernode_members`: Maps each supernode ID to the set of original node IDs in that SCC
/// - `supernode_edges`: Edges between supernodes in the condensed DAG (sorted, no self-loops)
/// - `num_sccs`: Total number of SCCs found (equals number of supernodes)
///
/// # Example
///
/// ```rust,ignore
/// let collapsed = collapse_sccs(&graph)?;
///
/// // Check how many SCCs were found
/// println!("Found {} SCCs", collapsed.num_sccs);
///
/// // Find which SCC a node belongs to
/// if let Some(supernode) = collapsed.supernode_for(node_id) {
///     println!("Node {} is in SCC {}", node_id, supernode);
///     if let Some(members) = collapsed.members_of(supernode) {
///         println!("SCC members: {:?}", members);
///     }
///     // Check if this is a trivial SCC (single node, no self-loop)
///     if collapsed.is_trivial(supernode) {
///         println!("This is a trivial SCC (no cycle)");
///     } else {
///         println!("This is a non-trivial SCC (contains a cycle)");
///     }
/// }
///
/// // Iterate over condensed DAG edges
/// for &(from, to) in &collapsed.supernode_edges {
///     println!("SCC {} -> SCC {}", from, to);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SccCollapseResult {
    /// Maps each original node ID to its SCC supernode ID.
    /// Use `supernode_for()` for convenient lookup.
    pub node_to_supernode: AHashMap<i64, i64>,

    /// Maps each supernode ID to the set of original node IDs in that SCC.
    /// Use `members_of()` for convenient lookup.
    pub supernode_members: AHashMap<i64, AHashSet<i64>>,

    /// Edges between supernodes in the condensed DAG.
    /// No self-loops (from == to) exist by definition.
    /// Sorted for deterministic output.
    pub supernode_edges: Vec<(i64, i64)>,

    /// Total number of SCCs found.
    /// Equals the number of supernodes in the condensed graph.
    pub num_sccs: usize,
}

impl SccCollapseResult {
    /// Gets the supernode ID for a given original node.
    ///
    /// Returns `Some(supernode_id)` if the node exists in the graph,
    /// or `None` if the node is unknown.
    ///
    /// # Arguments
    /// * `node` - The original node ID to look up
    ///
    /// # Returns
    /// `Some(i64)` with the supernode ID, or `None` if node not found
    ///
    /// # Example
    /// ```rust,ignore
    /// let collapsed = collapse_sccs(&graph)?;
    /// if let Some(supernode) = collapsed.supernode_for(node_id) {
    ///     println!("Node {} is in SCC {}", node_id, supernode);
    /// }
    /// ```
    pub fn supernode_for(&self, node: i64) -> Option<i64> {
        self.node_to_supernode.get(&node).copied()
    }

    /// Gets the set of original node IDs belonging to a supernode.
    ///
    /// Returns `Some(&AHashSet<i64>)` with the member nodes, or `None` if the
    /// supernode ID is unknown.
    ///
    /// # Arguments
    /// * `supernode` - The supernode ID to look up
    ///
    /// # Returns
    /// `Some(&AHashSet<i64>)` with member nodes, or `None` if supernode not found
    ///
    /// # Example
    /// ```rust,ignore
    /// let collapsed = collapse_sccs(&graph)?;
    /// if let Some(members) = collapsed.members_of(supernode_id) {
    ///     println!("SCC {} has {} members:", supernode_id, members.len());
    ///     for &node in members {
    ///         println!("  - Node {}", node);
    ///     }
    /// }
    /// ```
    pub fn members_of(&self, supernode: i64) -> Option<&AHashSet<i64>> {
        self.supernode_members.get(&supernode)
    }

    /// Checks if an SCC is trivial (single node with no self-loop).
    ///
    /// A trivial SCC is a single node that doesn't participate in a cycle.
    /// Non-trivial SCCs have multiple nodes OR a single node with a self-loop,
    /// both indicating cyclic behavior.
    ///
    /// # Arguments
    /// * `supernode` - The supernode ID to check
    ///
    /// # Returns
    /// `true` if the SCC is trivial (single node), `false` if non-trivial or not found
    ///
    /// # Example
    /// ```rust,ignore
    /// let collapsed = collapse_sccs(&graph)?;
    /// for (&supernode, members) in &collapsed.supernode_members {
    ///     if collapsed.is_trivial(supernode) {
    ///         println!("SCC {} is trivial (no cycle)", supernode);
    ///     } else {
    ///         println!("SCC {} is non-trivial (contains cycle)", supernode);
    ///     }
    /// }
    /// ```
    pub fn is_trivial(&self, supernode: i64) -> bool {
        match self.members_of(supernode) {
            Some(members) => members.len() == 1,
            None => false,
        }
    }

    /// Gets the number of non-trivial SCCs (cycles in the original graph).
    ///
    /// Non-trivial SCCs indicate cyclic regions in the original graph.
    /// This is useful for detecting mutual recursion or circular dependencies.
    ///
    /// # Returns
    /// Number of SCCs with more than one member
    ///
    /// # Example
    /// ```rust,ignore
    /// let collapsed = collapse_sccs(&graph)?;
    /// let cycle_count = collapsed.non_trivial_count();
    /// if cycle_count > 0 {
    ///     println!("Graph contains {} cycles (mutual recursion)", cycle_count);
    /// }
    /// ```
    pub fn non_trivial_count(&self) -> usize {
        self.supernode_members
            .values()
            .filter(|members| members.len() > 1)
            .count()
    }

    /// Gets all nodes that are part of non-trivial SCCs.
    ///
    /// Returns the set of all node IDs that participate in cycles.
    /// Useful for identifying which functions are involved in mutual recursion.
    ///
    /// # Returns
    /// Set of node IDs in non-trivial SCCs
    ///
    /// # Example
    /// ```rust,ignore
    /// let collapsed = collapse_sccs(&graph)?;
    /// let cyclic_nodes = collapsed.non_trivial_nodes();
    /// if !cyclic_nodes.is_empty() {
    ///     println!("Nodes involved in cycles: {:?}", cyclic_nodes);
    /// }
    /// ```
    pub fn non_trivial_nodes(&self) -> AHashSet<i64> {
        self.supernode_members
            .values()
            .filter(|members| members.len() > 1)
            .flat_map(|members| members.iter().copied())
            .collect()
    }
}

/// Collapses SCCs to form a condensation DAG.
///
/// Transforms a directed graph into a DAG by collapsing each strongly connected
/// component into a single "supernode". The resulting condensation graph is
/// always acyclic, enabling topological analysis on graphs with cycles.
///
/// This is particularly useful for:
/// - **Call graph analysis**: Collapse mutually recursive functions
/// - **Dependency analysis**: Detect and handle circular dependencies
/// - **Graph visualization**: Simplify cyclic regions for cleaner diagrams
///
/// # Arguments
/// * `graph` - The graph to analyze
///
/// # Returns
/// `SccCollapseResult` containing:
/// - Bidirectional mappings between original nodes and supernodes
/// - Condensed DAG edges (between supernodes)
/// - Number of SCCs found
///
/// # Complexity
/// - **Time**: O(|V| + |E|) - one SCC pass + one edge enumeration
/// - **Space**: O(|V|) for node mappings and condensed edges
///
/// # Algorithm Steps
///
/// 1. **Compute SCCs**: Call `strongly_connected_components()` to decompose graph
/// 2. **Handle empty graph**: Return empty `SccCollapseResult` if no nodes exist
/// 3. **Build node_to_supernode mapping**: Map each original node to its SCC supernode ID
/// 4. **Build supernode_members mapping**: Map each supernode to its member nodes
/// 5. **Build supernode_edges**: Enumerate edges, deduplicate, filter self-loops
/// 6. **Sort edges**: Ensure deterministic output
/// 7. **Return result**: Complete with count and mappings
///
/// # Condensation Graph Properties
///
/// - **No self-loops**: Edges only exist between different supernodes
/// - **Acyclic**: Always a DAG by fundamental property of SCC decomposition
/// - **Deterministic**: Edges are sorted for consistent output
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::collapse_sccs};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build call graph ...
///
/// let collapsed = collapse_sccs(&graph)?;
///
/// println!("Found {} SCCs", collapsed.num_sccs);
/// println!("Cycles detected: {}", collapsed.non_trivial_count());
/// println!("Condensed edges: {}", collapsed.supernode_edges.len());
/// ```
///
/// # References
///
/// - CP-Algorithms: Strongly Connected Components and Condensation Graph
///   https://cp-algorithms.com/graph/strongly-connected-components.html
pub fn collapse_sccs(graph: &SqliteGraph) -> Result<SccCollapseResult, SqliteGraphError> {
    // Step 1: Compute SCCs using Tarjan's algorithm
    let scc_result = strongly_connected_components(graph)?;

    // Step 2: Handle empty graph
    if scc_result.components.is_empty() {
        return Ok(SccCollapseResult {
            node_to_supernode: AHashMap::new(),
            supernode_members: AHashMap::new(),
            supernode_edges: Vec::new(),
            num_sccs: 0,
        });
    }

    // Step 3: Build node_to_supernode and supernode_members mappings
    // Use the component index as the supernode ID (deterministic)
    let mut node_to_supernode: AHashMap<i64, i64> = AHashMap::new();
    let mut supernode_members: AHashMap<i64, AHashSet<i64>> = AHashMap::new();

    for (&node, &component_idx) in &scc_result.node_to_component {
        // Use component index as supernode ID (ensures deterministic output)
        let supernode_id = component_idx as i64;

        node_to_supernode.insert(node, supernode_id);

        supernode_members
            .entry(supernode_id)
            .or_default()
            .insert(node);
    }

    // Step 4: Build supernode edges (condensed graph)
    // Use AHashSet to deduplicate edges
    let mut edge_set: AHashSet<(i64, i64)> = AHashSet::new();

    for &from_node in &graph.all_entity_ids()? {
        if let Some(&from_supernode) = node_to_supernode.get(&from_node) {
            for &to_node in &graph.fetch_outgoing(from_node)? {
                if let Some(&to_supernode) = node_to_supernode.get(&to_node) {
                    // Only add edges between different supernodes (no self-loops)
                    if from_supernode != to_supernode {
                        edge_set.insert((from_supernode, to_supernode));
                    }
                }
            }
        }
    }

    // Step 5: Convert to sorted Vec for deterministic output
    let mut supernode_edges: Vec<(i64, i64)> = edge_set.into_iter().collect();
    supernode_edges.sort();
    supernode_edges.dedup();

    // Step 6: Return result
    Ok(SccCollapseResult {
        node_to_supernode,
        supernode_members,
        supernode_edges,
        num_sccs: scc_result.components.len(),
    })
}

/// Collapses SCCs with progress tracking.
///
/// Same algorithm as [`collapse_sccs`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `SccCollapseResult` containing the condensed graph and mappings.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current number of edges processed
/// - `total`: Total number of edges (if known)
/// - `message`: Progress status message
///
/// Progress milestones:
/// - After SCC computation: "SCC collapse: computed {count} SCCs"
/// - During edge enumeration: Every ~100 edges processed
/// - On completion: Calls `on_complete()`
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{algo::collapse_sccs_with_progress, progress::ConsoleProgress};
///
/// let progress = ConsoleProgress::new();
/// let collapsed = collapse_sccs_with_progress(&graph, &progress)?;
/// // Output:
/// // SCC collapse: computed 5 SCCs [0 edges processed]
/// // SCC collapse: building condensed graph... [100 edges processed]
/// // Complete
/// ```
pub fn collapse_sccs_with_progress<F>(
    graph: &SqliteGraph,
    progress: &F,
) -> Result<SccCollapseResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    // Step 1: Compute SCCs using Tarjan's algorithm
    let scc_result = strongly_connected_components(graph)?;

    progress.on_progress(
        0,
        None,
        &format!(
            "SCC collapse: computed {} SCCs",
            scc_result.components.len()
        ),
    );

    // Step 2: Handle empty graph
    if scc_result.components.is_empty() {
        progress.on_complete();
        return Ok(SccCollapseResult {
            node_to_supernode: AHashMap::new(),
            supernode_members: AHashMap::new(),
            supernode_edges: Vec::new(),
            num_sccs: 0,
        });
    }

    // Step 3: Build node_to_supernode and supernode_members mappings
    let mut node_to_supernode: AHashMap<i64, i64> = AHashMap::new();
    let mut supernode_members: AHashMap<i64, AHashSet<i64>> = AHashMap::new();

    for (&node, &component_idx) in &scc_result.node_to_component {
        let supernode_id = component_idx as i64;

        node_to_supernode.insert(node, supernode_id);

        supernode_members
            .entry(supernode_id)
            .or_default()
            .insert(node);
    }

    progress.on_progress(0, None, "SCC collapse: building condensed graph...");

    // Step 4: Build supernode edges with progress tracking
    let mut edge_set: AHashSet<(i64, i64)> = AHashSet::new();
    let all_nodes = graph.all_entity_ids()?;
    let total_edges_hint = all_nodes.len().saturating_mul(2); // Rough estimate

    let mut edges_processed = 0;

    for &from_node in &all_nodes {
        if let Some(&from_supernode) = node_to_supernode.get(&from_node) {
            for &to_node in &graph.fetch_outgoing(from_node)? {
                edges_processed += 1;

                if let Some(&to_supernode) = node_to_supernode.get(&to_node)
                    && from_supernode != to_supernode
                {
                    edge_set.insert((from_supernode, to_supernode));
                }
            }
        }

        // Report progress every ~100 edges
        if edges_processed % 100 == 0 {
            progress.on_progress(
                edges_processed,
                Some(total_edges_hint),
                &format!(
                    "SCC collapse: processed {} edges, {} unique supernode edges",
                    edges_processed,
                    edge_set.len()
                ),
            );
        }
    }

    // Step 5: Convert to sorted Vec
    let mut supernode_edges: Vec<(i64, i64)> = edge_set.into_iter().collect();
    supernode_edges.sort();
    supernode_edges.dedup();

    // Report completion
    progress.on_complete();

    // Step 6: Return result
    Ok(SccCollapseResult {
        node_to_supernode,
        supernode_members,
        supernode_edges,
        num_sccs: scc_result.components.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create empty graph
    fn create_empty_graph() -> SqliteGraph {
        SqliteGraph::open_in_memory().expect("Failed to create graph")
    }

    /// Helper: Create graph with single node
    fn create_single_node_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: "single".to_string(),
            file_path: Some("single.rs".to_string()),
            data: serde_json::json!({}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        graph
    }

    /// Helper: Create DAG: 0 -> 1 -> 2 -> 3 (all trivial SCCs)
    fn create_dag() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes
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

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

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

    /// Helper: Create mutual recursion: 0 <-> 1, and 2 -> 3 -> 4 (linear)
    fn create_mutual_recursion_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 5 nodes
        for i in 0..5 {
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

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Create mutual recursion: 0 <-> 1, and 2 -> 3 -> 4 (linear)
        let edges = vec![(0, 1), (1, 0), (2, 3), (3, 4)];
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

    /// Helper: Create triangle SCC: 0 -> 1 -> 2 -> 0
    fn create_triangle_scc() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 3 nodes
        for i in 0..3 {
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

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Create triangle: 0 -> 1 -> 2 -> 0
        let cycle = vec![(0, 1), (1, 2), (2, 0)];
        for (from_idx, to_idx) in cycle {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    // Tests for collapse_sccs

    #[test]
    fn test_collapse_sccs_empty_graph() {
        // Scenario: Empty graph
        // Expected: Returns empty SccCollapseResult
        let graph = create_empty_graph();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        assert_eq!(collapsed.num_sccs, 0);
        assert_eq!(collapsed.node_to_supernode.len(), 0);
        assert_eq!(collapsed.supernode_members.len(), 0);
        assert_eq!(collapsed.supernode_edges.len(), 0);
        assert_eq!(collapsed.non_trivial_count(), 0);
    }

    #[test]
    fn test_collapse_sccs_single_node() {
        // Scenario: Single node, no edges
        // Expected: One trivial SCC
        let graph = create_single_node_graph();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        assert_eq!(collapsed.num_sccs, 1);
        assert_eq!(collapsed.node_to_supernode.len(), 1);
        assert_eq!(collapsed.supernode_members.len(), 1);
        assert_eq!(collapsed.supernode_edges.len(), 0);

        // Verify it's a trivial SCC
        assert_eq!(collapsed.non_trivial_count(), 0);

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");
        let node_id = entity_ids[0];

        // Check node_to_supernode mapping
        let supernode = collapsed.supernode_for(node_id);
        assert!(supernode.is_some(), "Node should have a supernode");

        // Check supernode_members mapping
        let members = collapsed.members_of(supernode.unwrap());
        assert!(members.is_some(), "Supernode should have members");
        assert_eq!(members.unwrap().len(), 1, "SCC should have 1 member");

        // Check is_trivial
        assert!(
            collapsed.is_trivial(supernode.unwrap()),
            "Single node should be trivial"
        );
    }

    #[test]
    fn test_collapse_sccs_dag() {
        // Scenario: DAG 0 -> 1 -> 2 -> 3
        // Expected: All nodes are separate supernodes (4 trivial SCCs)
        let graph = create_dag();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        // All 4 nodes are separate SCCs
        assert_eq!(collapsed.num_sccs, 4);
        assert_eq!(collapsed.node_to_supernode.len(), 4);
        assert_eq!(collapsed.non_trivial_count(), 0);

        // 3 edges in condensed graph (same as original)
        assert_eq!(collapsed.supernode_edges.len(), 3);

        // Verify no self-loops
        for &(from, to) in &collapsed.supernode_edges {
            assert_ne!(from, to, "Condensed graph should have no self-loops");
        }

        // Verify all SCCs are trivial
        for (&supernode, members) in &collapsed.supernode_members {
            assert!(
                collapsed.is_trivial(supernode),
                "DAG nodes should be trivial SCCs"
            );
            assert_eq!(members.len(), 1, "Each SCC should have 1 member");
        }
    }

    #[test]
    fn test_collapse_sccs_mutual_recursion() {
        // Scenario: Mutual recursion 0 <-> 1, plus chain 2 -> 3 -> 4
        // Expected: {0, 1} becomes one supernode, {2}, {3}, {4} are separate
        let graph = create_mutual_recursion_graph();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        // 4 SCCs: {0,1}, {2}, {3}, {4}
        assert_eq!(collapsed.num_sccs, 4);
        assert_eq!(collapsed.node_to_supernode.len(), 5);

        // 1 non-trivial SCC (the mutual recursion)
        assert_eq!(collapsed.non_trivial_count(), 1);

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Nodes 0 and 1 should be in the same SCC
        let scc0 = collapsed.supernode_for(entity_ids[0]);
        let scc1 = collapsed.supernode_for(entity_ids[1]);
        assert_eq!(scc0, scc1, "Nodes 0 and 1 should be in same SCC");

        // Nodes 2, 3, 4 should each be in their own SCC
        let scc2 = collapsed.supernode_for(entity_ids[2]);
        let scc3 = collapsed.supernode_for(entity_ids[3]);
        let scc4 = collapsed.supernode_for(entity_ids[4]);

        assert_ne!(scc2, scc0, "Node 2 should not be in SCC 0/1");
        assert_ne!(scc3, scc2, "Node 3 should not be in SCC 2");
        assert_ne!(scc4, scc3, "Node 4 should not be in SCC 3");

        // Verify the mutual recursion SCC is non-trivial
        if let Some(scc_id) = scc0 {
            assert!(
                !collapsed.is_trivial(scc_id),
                "Mutual recursion SCC should be non-trivial"
            );
            if let Some(members) = collapsed.members_of(scc_id) {
                assert_eq!(
                    members.len(),
                    2,
                    "Mutual recursion SCC should have 2 members"
                );
            }
        }

        // Verify condensed edges (should have no self-loops)
        for &(from, to) in &collapsed.supernode_edges {
            assert_ne!(from, to, "Condensed graph should have no self-loops");
        }
    }

    #[test]
    fn test_collapse_sccs_triangle() {
        // Scenario: Triangle 0 -> 1 -> 2 -> 0
        // Expected: All three nodes become one supernode
        let graph = create_triangle_scc();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        // 1 SCC containing all 3 nodes
        assert_eq!(collapsed.num_sccs, 1);
        assert_eq!(collapsed.node_to_supernode.len(), 3);

        // 1 non-trivial SCC
        assert_eq!(collapsed.non_trivial_count(), 1);

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // All nodes should be in the same SCC
        let scc0 = collapsed.supernode_for(entity_ids[0]);
        let scc1 = collapsed.supernode_for(entity_ids[1]);
        let scc2 = collapsed.supernode_for(entity_ids[2]);

        assert_eq!(scc0, scc1, "Nodes 0 and 1 should be in same SCC");
        assert_eq!(scc1, scc2, "Nodes 1 and 2 should be in same SCC");

        // No edges in condensed graph (single supernode)
        assert_eq!(collapsed.supernode_edges.len(), 0);

        // Verify SCC is non-trivial
        if let Some(scc_id) = scc0 {
            assert!(
                !collapsed.is_trivial(scc_id),
                "Triangle SCC should be non-trivial"
            );
            if let Some(members) = collapsed.members_of(scc_id) {
                assert_eq!(members.len(), 3, "Triangle SCC should have 3 members");
            }
        }
    }

    #[test]
    fn test_collapse_sccs_no_self_loops() {
        // Scenario: Graph with mutual recursion
        // Expected: Condensed graph has no self-loops
        let graph = create_mutual_recursion_graph();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        // Verify no self-loops in condensed graph
        for &(from, to) in &collapsed.supernode_edges {
            assert_ne!(from, to, "Condensed DAG should not have self-loops");
        }
    }

    #[test]
    fn test_collapse_sccs_bidirectional_mapping() {
        // Scenario: Mutual recursion graph
        // Expected: node_to_supernode and supernode_members are consistent
        let graph = create_mutual_recursion_graph();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // For each node, verify bidirectional mapping consistency
        for &node in &entity_ids {
            if let Some(supernode) = collapsed.supernode_for(node)
                && let Some(members) = collapsed.members_of(supernode)
            {
                assert!(
                    members.contains(&node),
                    "Supernode {} should contain node {}",
                    supernode,
                    node
                );
            }
        }

        // Verify each supernode's members map back to it
        for (&supernode, members) in &collapsed.supernode_members {
            for &member in members {
                let mapped = collapsed.supernode_for(member);
                assert_eq!(
                    Some(supernode),
                    mapped,
                    "Node {} should map back to supernode {}",
                    member,
                    supernode
                );
            }
        }
    }

    #[test]
    fn test_collapse_sccs_deterministic_edges() {
        // Scenario: Graph with multiple edges
        // Expected: Edges are sorted deterministically
        let graph = create_dag();
        let result = collapse_sccs(&graph);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        // Check that edges are sorted
        let mut edges_copy = collapsed.supernode_edges.clone();
        edges_copy.sort();

        assert_eq!(
            collapsed.supernode_edges, edges_copy,
            "Edges should be sorted"
        );

        // Check that there are no duplicates
        let mut unique_edges = collapsed.supernode_edges.clone();
        unique_edges.dedup();

        assert_eq!(
            collapsed.supernode_edges.len(),
            unique_edges.len(),
            "Edges should be deduplicated"
        );
    }

    // Tests for SccCollapseResult methods

    #[test]
    fn test_supernode_for() {
        // Scenario: Query supernode for existing and non-existent nodes
        // Expected: Returns Some(supernode) for existing, None for non-existent
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Existing node should return Some
        let supernode = collapsed.supernode_for(entity_ids[0]);
        assert!(supernode.is_some(), "Existing node should have supernode");

        // Non-existent node should return None
        let non_existent = collapsed.supernode_for(99999);
        assert!(
            non_existent.is_none(),
            "Non-existent node should return None"
        );
    }

    #[test]
    fn test_members_of() {
        // Scenario: Query members for existing and non-existent supernodes
        // Expected: Returns Some(members) for existing, None for non-existent
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Get the supernode for node 0
        if let Some(supernode) = collapsed.supernode_for(entity_ids[0]) {
            // Existing supernode should return Some
            let members = collapsed.members_of(supernode);
            assert!(members.is_some(), "Existing supernode should have members");
            assert_eq!(members.unwrap().len(), 2, "Should have 2 members");
        }

        // Non-existent supernode should return None
        let non_existent = collapsed.members_of(99999);
        assert!(
            non_existent.is_none(),
            "Non-existent supernode should return None"
        );
    }

    #[test]
    fn test_is_trivial() {
        // Scenario: Check trivial vs non-trivial SCCs
        // Expected: Single node SCCs are trivial, multi-node are not
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Find the mutual recursion SCC (nodes 0 and 1)
        let scc0 = collapsed.supernode_for(entity_ids[0]);
        let scc2 = collapsed.supernode_for(entity_ids[2]);

        if let Some(scc_id) = scc0 {
            // Multi-node SCC should be non-trivial
            assert!(
                !collapsed.is_trivial(scc_id),
                "Multi-node SCC should not be trivial"
            );
        }

        if let Some(scc_id) = scc2 {
            // Single node SCC should be trivial
            assert!(
                collapsed.is_trivial(scc_id),
                "Single node SCC should be trivial"
            );
        }

        // Non-existent SCC should return false (not found)
        assert!(
            !collapsed.is_trivial(99999),
            "Non-existent SCC should return false"
        );
    }

    #[test]
    fn test_non_trivial_count() {
        // Scenario: Count non-trivial SCCs
        // Expected: Returns correct count of multi-node SCCs
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        // Should have 1 non-trivial SCC (the mutual recursion)
        assert_eq!(collapsed.non_trivial_count(), 1);

        // DAG should have 0 non-trivial SCCs
        let dag = create_dag();
        let dag_collapsed = collapse_sccs(&dag).expect("Failed");
        assert_eq!(dag_collapsed.non_trivial_count(), 0);

        // Triangle should have 1 non-trivial SCC
        let triangle = create_triangle_scc();
        let triangle_collapsed = collapse_sccs(&triangle).expect("Failed");
        assert_eq!(triangle_collapsed.non_trivial_count(), 1);
    }

    #[test]
    fn test_non_trivial_nodes() {
        // Scenario: Get all nodes in non-trivial SCCs
        // Expected: Returns set of nodes participating in cycles
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        let cyclic_nodes = collapsed.non_trivial_nodes();

        // Should have 2 cyclic nodes (0 and 1 in mutual recursion)
        assert_eq!(cyclic_nodes.len(), 2);

        let entity_ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        // Nodes 0 and 1 should be in the set (they form the mutual recursion)
        assert!(cyclic_nodes.contains(&entity_ids[0]));
        assert!(cyclic_nodes.contains(&entity_ids[1]));

        // Nodes 2, 3, 4 should not be in the set (they're in a chain)
        assert!(!cyclic_nodes.contains(&entity_ids[2]));
        assert!(!cyclic_nodes.contains(&entity_ids[3]));
        assert!(!cyclic_nodes.contains(&entity_ids[4]));
    }

    // Tests for progress variant

    #[test]
    fn test_collapse_sccs_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results
        use crate::progress::NoProgress;

        let graph = create_mutual_recursion_graph();

        let progress = NoProgress;
        let result_with = collapse_sccs_with_progress(&graph, &progress).expect("Failed");
        let result_without = collapse_sccs(&graph).expect("Failed");

        assert_eq!(
            result_with.num_sccs, result_without.num_sccs,
            "Progress and non-progress results should match"
        );

        assert_eq!(
            result_with.supernode_edges.len(),
            result_without.supernode_edges.len(),
            "Edges should match"
        );

        assert_eq!(
            result_with.non_trivial_count(),
            result_without.non_trivial_count(),
            "Non-trivial count should match"
        );
    }

    #[test]
    fn test_collapse_sccs_empty_with_progress() {
        // Scenario: Empty graph with progress
        // Expected: Returns empty result without error
        use crate::progress::NoProgress;

        let graph = create_empty_graph();

        let progress = NoProgress;
        let result = collapse_sccs_with_progress(&graph, &progress);

        assert!(result.is_ok());
        let collapsed = result.unwrap();

        assert_eq!(collapsed.num_sccs, 0);
        assert_eq!(collapsed.supernode_edges.len(), 0);
    }

    #[test]
    fn test_condensation_is_dag() {
        // Scenario: Verify condensation graph is acyclic
        // Expected: No cycles in supernode_edges
        let graph = create_mutual_recursion_graph();
        let collapsed = collapse_sccs(&graph).expect("Failed");

        // Build adjacency map for cycle detection
        let mut adj: AHashMap<i64, Vec<i64>> = AHashMap::new();
        for &(from, to) in &collapsed.supernode_edges {
            adj.entry(from).or_default().push(to);
        }

        // Simple DFS cycle detection
        let mut visited = AHashSet::new();
        let mut rec_stack = AHashSet::new();

        fn has_cycle(
            node: i64,
            adj: &AHashMap<i64, Vec<i64>>,
            visited: &mut AHashSet<i64>,
            rec_stack: &mut AHashSet<i64>,
        ) -> bool {
            visited.insert(node);
            rec_stack.insert(node);

            if let Some(neighbors) = adj.get(&node) {
                for &neighbor in neighbors {
                    if !visited.contains(&neighbor) {
                        if has_cycle(neighbor, adj, visited, rec_stack) {
                            return true;
                        }
                    } else if rec_stack.contains(&neighbor) {
                        return true;
                    }
                }
            }

            rec_stack.remove(&node);
            false
        }

        // Check all supernodes
        for (&supernode, _) in &collapsed.supernode_members {
            if !visited.contains(&supernode) {
                assert!(
                    !has_cycle(supernode, &adj, &mut visited, &mut rec_stack),
                    "Condensation graph should be acyclic"
                );
            }
        }
    }
}
