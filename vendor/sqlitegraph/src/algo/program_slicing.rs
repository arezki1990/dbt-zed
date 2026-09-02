//! Program slicing for bug isolation and impact analysis.
//!
//! This module provides backward and forward program slicing algorithms
//! for control flow graphs (CFGs). Program slicing answers fundamental questions
//! about program behavior and change impact.
//!
//! # What is Program Slicing?
//!
//! Program slicing extracts a subset of a program (the "slice") that affects
//! or is affected by a specified point (the "criterion"). It's a form of
//! program decomposition that focuses on relevant behavior.
//!
//! ## Backward Slicing ("what can affect this node?")
//!
//! Computes all statements that can influence the value at a given point.
//!
//! **Use cases:**
//! - **Bug isolation**: Find what causes a bug at line N
//! - **Root cause analysis**: Trace backwards from error to source
//! - **Refactoring safety**: Check if changing X affects Y
//! - **Program comprehension**: Understand data flow to a point
//!
//! **Source:** Weiser, M. "Program Slicing" IEEE TSE 1984
//!
//! ## Forward Slicing ("what does this node affect?")
//!
//! Computes all statements that can be influenced by a given point.
//!
//! **Use cases:**
//! - **Impact analysis**: What breaks if I change line N?
//! - **Regression testing**: What tests to run after this change?
//! - **Change propagation**: Where does this modification flow to?
//! - **Dead code elimination**: What code is unreachable from here?
//!
//! **Source:** Bergeretti & Carre, ACM TOPLAS 1985
//!
//! # Algorithm
//!
//! Program slicing is computed as the union of **control dependence** and **data dependence**:
//!
//! - **Control dependence**: "What conditions must execute?" (via CDG)
//! - **Data dependence**: "Where does data flow from/to?" (via reachability)
//!
//! ```text
//! Backward slice(target) = control_predecessors(target) + data_predecessors(target)
//! Forward slice(source) = control_successors(source) + data_successors(source)
//! ```
//!
//! ## Complexity
//!
//! - **Time**: O(|V| + |E|) - two BFS traversals (control + data)
//! - **Space**: O(|V|) - for visited sets and slice result
//!
//! Where:
//! - V = number of vertices
//! - E = number of edges
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{
//!     SqliteGraph,
//!     algo::{control_dependence_from_exit, backward_slice, forward_slice},
//! };
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... build CFG with entry 0 and exit 5 ...
//!
//! // Compute control dependence first (required for slicing)
//! let cdg = control_dependence_from_exit(&graph)?;
//!
//! // Backward: What affects node 4?
//! let backward = backward_slice(&graph, &cdg, 4)?;
//! println!("Slice affecting node 4: {:?}", backward.sorted_nodes());
//!
//! // Forward: What does node 0 affect?
//! let forward = forward_slice(&graph, &cdg, 0)?;
//! println!("Slice affected by node 0: {:?}", forward.sorted_nodes());
//! ```
//!
//! # Slice Result Structure
//!
//! Results separate **control_nodes** and **data_nodes** for debugging:
//!
//! - `control_nodes`: Nodes in the slice due to control flow (conditions, branches)
//! - `data_nodes`: Nodes in the slice due to data flow (definitions, uses)
//! - `slice_nodes`: Union of both (complete slice)
//!
//! # References
//!
//! - Weiser, M. "Program Slicing" IEEE Transactions on Software Engineering, 1984
//! - Ferrante et al. "The Program Dependence Graph" ACM TOPLAS, 1987
//! - Bergeretti & Carre "Information-flow and data-flow analysis" ACM TOPLAS, 1985

use std::collections::VecDeque;

use ahash::AHashSet;

use crate::errors::SqliteGraphError;
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

use super::control_dependence::ControlDependenceResult;
use super::reachability::{reachable_from, reverse_reachable_from};

/// Result of a program slicing operation.
///
/// Contains the complete slice with separation between control-dependent
/// and data-dependent nodes for debugging and analysis.
///
/// # Fields
///
/// - `criterion`: The slicing criterion node (the target/source node)
/// - `slice_nodes`: All nodes in the slice (union of control + data)
/// - `control_nodes`: Nodes in the slice due to control dependence
/// - `data_nodes`: Nodes in the slice due to data dependence
/// - `size`: Number of nodes in the slice
///
/// # Example
///
/// ```rust,ignore
/// let result = backward_slice(&graph, &cdg, target)?;
///
/// println!("Slice size: {}", result.size);
/// println!("Control nodes: {:?}", result.control_nodes);
/// println!("Data nodes: {:?}", result.data_nodes);
///
/// // Check if a node is in the slice
/// if result.contains(node_id) {
///     println!("Node {} is in the slice", node_id);
/// }
///
/// // Get deterministic sorted output
/// for node in result.sorted_nodes() {
///     println!("Slice node: {}", node);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SliceResult {
    /// The slicing criterion (node the slice is based on)
    pub criterion: i64,

    /// All nodes in the slice (union of control + data dependence)
    pub slice_nodes: AHashSet<i64>,

    /// Control-dependent nodes in the slice (via CDG)
    pub control_nodes: AHashSet<i64>,

    /// Data-dependent nodes in the slice (via reachability)
    pub data_nodes: AHashSet<i64>,

    /// Number of nodes in the slice
    pub size: usize,
}

impl SliceResult {
    /// Checks if a node is in the slice.
    ///
    /// Returns `true` if the node is in the complete slice (either control
    /// or data dependence).
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// `true` if node is in slice_nodes, `false` otherwise
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = backward_slice(&graph, &cdg, target)?;
    /// if result.contains(some_node) {
    ///     println!("Node {} is relevant to the criterion", some_node);
    /// }
    /// ```
    pub fn contains(&self, node: i64) -> bool {
        self.slice_nodes.contains(&node)
    }

    /// Gets sorted slice nodes for deterministic output.
    ///
    /// Returns nodes in ascending order by ID. Useful for testing,
    /// debugging, and consistent display.
    ///
    /// # Returns
    /// Vector of node IDs sorted in ascending order
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = backward_slice(&graph, &cdg, target)?;
    /// for node in result.sorted_nodes() {
    ///     println!("Slice node: {}", node);
    /// }
    /// ```
    pub fn sorted_nodes(&self) -> Vec<i64> {
        let mut nodes: Vec<i64> = self.slice_nodes.iter().copied().collect();
        nodes.sort();
        nodes
    }

    /// Gets sorted control nodes for deterministic output.
    ///
    /// Returns control-dependent nodes in ascending order by ID.
    pub fn sorted_control_nodes(&self) -> Vec<i64> {
        let mut nodes: Vec<i64> = self.control_nodes.iter().copied().collect();
        nodes.sort();
        nodes
    }

    /// Gets sorted data nodes for deterministic output.
    ///
    /// Returns data-dependent nodes in ascending order by ID.
    pub fn sorted_data_nodes(&self) -> Vec<i64> {
        let mut nodes: Vec<i64> = self.data_nodes.iter().copied().collect();
        nodes.sort();
        nodes
    }
}

/// Computes backward program slice: "what can affect this node?"
///
/// Returns all nodes that can influence the value at the target node.
/// Combines control dependence (what conditions must execute) and data
/// dependence (what definitions flow to this point).
///
/// The slice is computed as:
/// - **Control**: Follow reverse CDG edges backward (what does target depend on?)
/// - **Data**: Follow reverse reachability (what can reach target?)
/// - **Union**: Control nodes + Data nodes = complete backward slice
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `cdg_result` - Pre-computed control dependence result (from `control_dependence_graph`)
/// * `target` - The target node ID (slicing criterion)
///
/// # Returns
/// `SliceResult` containing all nodes affecting the target, separated by control/data dependence.
///
/// # Complexity
/// - **Time**: O(|V| + |E|) - BFS for control + BFS for data
/// - **Space**: O(|V|) - for visited sets and slice result
///
/// # Algorithm Steps
///
/// 1. **Initialize slice**: Add target to slice_nodes, control_nodes, data_nodes
/// 2. **Control dependence BFS**:
///    - Start from target
///    - Follow reverse_cdg edges (what does each node depend on?)
///    - Add dependencies to control_nodes and slice_nodes
///    - Continue until queue exhausted (visited set prevents cycles)
/// 3. **Data dependence**: Call `reverse_reachable_from(graph, target)` for data dependencies
/// 4. **Merge**: Add data dependencies to data_nodes and slice_nodes
/// 5. **Return**: SliceResult with size = slice_nodes.len()
///
/// # Self-Inclusion
///
/// The target node is always included in the slice (self-inclusion requirement).
/// This ensures the slice is never empty and the criterion itself is considered.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::{control_dependence_from_exit, backward_slice}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let cdg = control_dependence_from_exit(&graph)?;
/// let slice = backward_slice(&graph, &cdg, target_node)?;
///
/// println!("Backward slice from {}: {} nodes", target_node, slice.size);
/// println!("Control nodes: {:?}", slice.sorted_control_nodes());
/// println!("Data nodes: {:?}", slice.sorted_data_nodes());
/// ```
///
/// # References
///
/// - Weiser, M. "Program Slicing" IEEE TSE 1984
/// - Ferrante et al. "The Program Dependence Graph" ACM TOPLAS 1987
pub fn backward_slice(
    graph: &SqliteGraph,
    cdg_result: &ControlDependenceResult,
    target: i64,
) -> Result<SliceResult, SqliteGraphError> {
    let mut slice_nodes = AHashSet::new();
    let mut control_nodes = AHashSet::new();
    let mut data_nodes = AHashSet::new();

    // Include target itself (self-inclusion requirement)
    slice_nodes.insert(target);

    // Step 1: Follow reverse CDG for control dependencies
    // BFS to find all nodes that the target depends on for control
    let mut queue = VecDeque::new();
    let mut visited = AHashSet::new();

    queue.push_back(target);
    visited.insert(target);

    while let Some(node) = queue.pop_front() {
        // What does this node depend on for control?
        if let Some(deps) = cdg_result.reverse_cdg.get(&node) {
            for &dep in deps {
                if visited.insert(dep) {
                    control_nodes.insert(dep);
                    slice_nodes.insert(dep);
                    queue.push_back(dep);
                }
            }
        }
    }

    // Step 2: Follow data flow backward
    // Data dependence: what can reach the target via data flow edges?
    let data_reachable = reverse_reachable_from(graph, target)?;
    for &node in &data_reachable {
        data_nodes.insert(node);
        slice_nodes.insert(node);
    }

    // Step 3: Compute size before moving
    let size = slice_nodes.len();

    // Step 4: Return result
    Ok(SliceResult {
        criterion: target,
        slice_nodes,
        control_nodes,
        data_nodes,
        size,
    })
}

/// Computes backward slice with progress tracking.
///
/// Same algorithm as [`backward_slice`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `cdg_result` - Pre-computed control dependence result
/// * `target` - The target node ID (slicing criterion)
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `SliceResult` containing all nodes affecting the target.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current number of nodes visited
/// - `total`: None (unknown total for BFS)
/// - `message`: "Backward slice: visited {current} nodes, {control} control, {data} data"
///
/// Progress is reported periodically (every ~10 nodes visited) to avoid
/// excessive callback overhead while still providing feedback.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::backward_slice_with_progress,
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let slice = backward_slice_with_progress(&graph, &cdg, target, &progress)?;
/// // Output: Backward slice: visited 10 nodes, 3 control, 7 data...
/// ```
pub fn backward_slice_with_progress<F>(
    graph: &SqliteGraph,
    cdg_result: &ControlDependenceResult,
    target: i64,
    progress: &F,
) -> Result<SliceResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    let mut slice_nodes = AHashSet::new();
    let mut control_nodes = AHashSet::new();
    let mut data_nodes = AHashSet::new();

    // Include target itself
    slice_nodes.insert(target);

    // Step 1: Follow reverse CDG for control dependencies
    let mut queue = VecDeque::new();
    let mut visited = AHashSet::new();
    let mut nodes_processed = 0;

    queue.push_back(target);
    visited.insert(target);

    while let Some(node) = queue.pop_front() {
        nodes_processed += 1;

        // Report progress every 10 nodes
        if nodes_processed % 10 == 0 {
            progress.on_progress(
                nodes_processed,
                None,
                &format!(
                    "Backward slice: visited {} nodes, {} control, {} data",
                    nodes_processed,
                    control_nodes.len(),
                    data_nodes.len()
                ),
            );
        }

        if let Some(deps) = cdg_result.reverse_cdg.get(&node) {
            for &dep in deps {
                if visited.insert(dep) {
                    control_nodes.insert(dep);
                    slice_nodes.insert(dep);
                    queue.push_back(dep);
                }
            }
        }
    }

    // Step 2: Follow data flow backward
    let data_reachable = reverse_reachable_from(graph, target)?;
    for &node in &data_reachable {
        data_nodes.insert(node);
        slice_nodes.insert(node);
    }

    // Step 3: Compute size before moving
    let size = slice_nodes.len();

    // Report completion
    progress.on_complete();

    Ok(SliceResult {
        criterion: target,
        slice_nodes,
        control_nodes,
        data_nodes,
        size,
    })
}

/// Computes forward program slice: "what does this node affect?"
///
/// Returns all nodes influenced by the source node.
/// Combines control dependence (what branches does this control?) and
/// forward reachability (where does data flow from here?).
///
/// The slice is computed as:
/// - **Control**: Follow CDG edges forward (what does source control?)
/// - **Data**: Follow forward reachability (what can source reach?)
/// - **Union**: Control nodes + Data nodes = complete forward slice
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `cdg_result` - Pre-computed control dependence result (from `control_dependence_graph`)
/// * `source` - The source node ID (slicing criterion)
///
/// # Returns
/// `SliceResult` containing all nodes affected by the source, separated by control/data dependence.
///
/// # Complexity
/// - **Time**: O(|V| + |E|) - BFS for control + BFS for data
/// - **Space**: O(|V|) - for visited sets and slice result
///
/// # Algorithm Steps
///
/// 1. **Initialize slice**: Add source to slice_nodes, control_nodes, data_nodes
/// 2. **Control dependence BFS**:
///    - Start from source
///    - Follow cdg edges (what does each node control?)
///    - Add controlled nodes to control_nodes and slice_nodes
///    - Continue until queue exhausted (visited set prevents cycles)
/// 3. **Data dependence**: Call `reachable_from(graph, source)` for data flow
/// 4. **Merge**: Add data flow nodes to data_nodes and slice_nodes
/// 5. **Return**: SliceResult with size = slice_nodes.len()
///
/// # Self-Inclusion
///
/// The source node is always included in the slice (self-inclusion requirement).
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::{control_dependence_from_exit, forward_slice}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let cdg = control_dependence_from_exit(&graph)?;
/// let slice = forward_slice(&graph, &cdg, source_node)?;
///
/// println!("Forward slice from {}: {} nodes", source_node, slice.size);
/// println!("Control nodes: {:?}", slice.sorted_control_nodes());
/// println!("Data nodes: {:?}", slice.sorted_data_nodes());
/// ```
///
/// # References
///
/// - Bergeretti & Carre "Information-flow and data-flow analysis" ACM TOPLAS 1985
/// - Silva "A Vocabulary of Program Slicing" 2007
pub fn forward_slice(
    graph: &SqliteGraph,
    cdg_result: &ControlDependenceResult,
    source: i64,
) -> Result<SliceResult, SqliteGraphError> {
    let mut slice_nodes = AHashSet::new();
    let mut control_nodes = AHashSet::new();
    let mut data_nodes = AHashSet::new();

    // Include source itself (self-inclusion requirement)
    slice_nodes.insert(source);

    // Step 1: Follow CDG forward for controlled nodes
    // BFS to find all nodes that the source controls
    let mut queue = VecDeque::new();
    let mut visited = AHashSet::new();

    queue.push_back(source);
    visited.insert(source);

    while let Some(node) = queue.pop_front() {
        // What does this node control?
        if let Some(controlled) = cdg_result.cdg.get(&node) {
            for &controlled_node in controlled {
                if visited.insert(controlled_node) {
                    control_nodes.insert(controlled_node);
                    slice_nodes.insert(controlled_node);
                    queue.push_back(controlled_node);
                }
            }
        }
    }

    // Step 2: Follow data flow forward
    // Data dependence: what can the source reach via data flow edges?
    let data_affected = reachable_from(graph, source)?;
    for &node in &data_affected {
        data_nodes.insert(node);
        slice_nodes.insert(node);
    }

    // Step 3: Compute size before moving
    let size = slice_nodes.len();

    // Step 4: Return result
    Ok(SliceResult {
        criterion: source,
        slice_nodes,
        control_nodes,
        data_nodes,
        size,
    })
}

/// Computes forward slice with progress tracking.
///
/// Same algorithm as [`forward_slice`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `cdg_result` - Pre-computed control dependence result
/// * `source` - The source node ID (slicing criterion)
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `SliceResult` containing all nodes affected by the source.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current number of nodes visited
/// - `total`: None (unknown total for BFS)
/// - `message`: "Forward slice: visited {current} nodes, {control} control, {data} data"
///
/// Progress is reported periodically (every ~10 nodes visited) to avoid
/// excessive callback overhead while still providing feedback.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::forward_slice_with_progress,
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let slice = forward_slice_with_progress(&graph, &cdg, source, &progress)?;
/// // Output: Forward slice: visited 10 nodes, 3 control, 7 data...
/// ```
pub fn forward_slice_with_progress<F>(
    graph: &SqliteGraph,
    cdg_result: &ControlDependenceResult,
    source: i64,
    progress: &F,
) -> Result<SliceResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    let mut slice_nodes = AHashSet::new();
    let mut control_nodes = AHashSet::new();
    let mut data_nodes = AHashSet::new();

    // Include source itself
    slice_nodes.insert(source);

    // Step 1: Follow CDG forward for controlled nodes
    let mut queue = VecDeque::new();
    let mut visited = AHashSet::new();
    let mut nodes_processed = 0;

    queue.push_back(source);
    visited.insert(source);

    while let Some(node) = queue.pop_front() {
        nodes_processed += 1;

        // Report progress every 10 nodes
        if nodes_processed % 10 == 0 {
            progress.on_progress(
                nodes_processed,
                None,
                &format!(
                    "Forward slice: visited {} nodes, {} control, {} data",
                    nodes_processed,
                    control_nodes.len(),
                    data_nodes.len()
                ),
            );
        }

        if let Some(controlled) = cdg_result.cdg.get(&node) {
            for &controlled_node in controlled {
                if visited.insert(controlled_node) {
                    control_nodes.insert(controlled_node);
                    slice_nodes.insert(controlled_node);
                    queue.push_back(controlled_node);
                }
            }
        }
    }

    // Step 2: Follow data flow forward
    let data_affected = reachable_from(graph, source)?;
    for &node in &data_affected {
        data_nodes.insert(node);
        slice_nodes.insert(node);
    }

    // Step 3: Compute size before moving
    let size = slice_nodes.len();

    // Report completion
    progress.on_complete();

    Ok(SliceResult {
        criterion: source,
        slice_nodes,
        control_nodes,
        data_nodes,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create linear chain CFG: 0 -> 1 -> 2 -> 3 (no control dependence)
    fn create_linear_chain() -> SqliteGraph {
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

    /// Helper: Create if-then-else CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    /// Expected: Node 3 is control-dependent on node 0 (merge point depends on branch)
    fn create_if_then_else_cfg() -> SqliteGraph {
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

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create if-then-else: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
        for (from_idx, to_idx) in edges {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create nested CFG for multi-level control dependence
    /// Structure: 0 -> 1, 0 -> 5, 1 -> 2, 1 -> 3, 2 -> 4, 3 -> 4, 4 -> 5
    /// Expected: Nested control dependencies (4 depends on 1 and 0)
    fn create_nested_cfg() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 6 nodes
        for i in 0..6 {
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

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create nested: 0 -> 1, 0 -> 5, 1 -> 2, 1 -> 3, 2 -> 4, 3 -> 4, 4 -> 5
        let edges = vec![(0, 1), (0, 5), (1, 2), (1, 3), (2, 4), (3, 4), (4, 5)];
        for (from_idx, to_idx) in edges {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    // Tests for backward_slice

    #[test]
    fn test_backward_slice_linear() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: All nodes via data flow (no control dependence)
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            backward_slice(&graph, &cdg, entity_ids[3]).expect("Failed to compute backward slice");

        // All 4 nodes should be in slice (via data flow)
        assert_eq!(result.size, 4, "Expected 4 nodes in slice");
        assert_eq!(
            result.criterion, entity_ids[3],
            "Criterion should be target"
        );

        // Nodes should be in slice (via data flow and/or control dependence)
        assert!(
            result.data_nodes.len() + result.control_nodes.len() >= 3,
            "Should have nodes in slice"
        );

        // Target should be in slice
        assert!(result.contains(entity_ids[3]), "Target should be in slice");
    }

    #[test]
    fn test_backward_slice_if_then_else() {
        // Scenario: If-then-else: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Expected: Node 0 in control_nodes (controls node 3)
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            backward_slice(&graph, &cdg, entity_ids[3]).expect("Failed to compute backward slice");

        // All nodes should be in slice
        assert_eq!(result.size, 4, "Expected 4 nodes in slice");

        // Node 0 should be in control_nodes (it controls node 3)
        assert!(
            result.control_nodes.contains(&entity_ids[0]),
            "Node 0 should be in control_nodes (controls merge point)"
        );
    }

    #[test]
    fn test_backward_slice_self_inclusion() {
        // Scenario: Self-inclusion requirement
        // Expected: Target node is always in results
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        // Test from node 1
        let result =
            backward_slice(&graph, &cdg, entity_ids[1]).expect("Failed to compute backward slice");

        assert_eq!(
            result.criterion, entity_ids[1],
            "Criterion should be target"
        );
        assert!(result.contains(entity_ids[1]), "Target should be in slice");
    }

    #[test]
    fn test_backward_slice_control_data_separation() {
        // Scenario: Verify both control and data nodes populated
        // Expected: Both sets non-empty for conditional CFG
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            backward_slice(&graph, &cdg, entity_ids[3]).expect("Failed to compute backward slice");

        // Should have both control and data nodes
        assert!(
            !result.control_nodes.is_empty(),
            "Control nodes should be non-empty"
        );
        assert!(
            !result.data_nodes.is_empty(),
            "Data nodes should be non-empty"
        );

        // Union should be slice_nodes
        let expected_union: AHashSet<i64> = result
            .control_nodes
            .union(&result.data_nodes)
            .copied()
            .collect();
        assert_eq!(
            result.slice_nodes, expected_union,
            "Slice nodes should be union of control + data"
        );
    }

    #[test]
    fn test_backward_slice_empty_graph() {
        // Scenario: Empty graph
        // Expected: Handles gracefully with minimal slice
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result = backward_slice(&graph, &cdg, 999).expect("Failed to compute backward slice");

        // Should have just the target (self-inclusion)
        assert_eq!(result.size, 1, "Empty graph should have minimal slice");
        assert_eq!(result.criterion, 999, "Criterion should be target");
    }

    // Tests for forward_slice

    #[test]
    fn test_forward_slice_linear() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: All nodes via data flow (no control dependence)
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        // All 4 nodes should be in slice (via data flow)
        assert_eq!(result.size, 4, "Expected 4 nodes in slice");
        assert_eq!(
            result.criterion, entity_ids[0],
            "Criterion should be source"
        );

        // Nodes should be in slice (via data flow and/or control dependence)
        assert!(
            result.data_nodes.len() + result.control_nodes.len() >= 3,
            "Should have nodes in slice"
        );
    }

    #[test]
    fn test_forward_slice_if_then_else() {
        // Scenario: If-then-else: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Expected: Node 0 controls all branches (nodes 1, 2, 3 in control_nodes)
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        // All nodes should be in slice
        assert_eq!(result.size, 4, "Expected 4 nodes in slice");

        // Node 3 should be in control_nodes (controlled by node 0)
        assert!(
            result.control_nodes.contains(&entity_ids[3]),
            "Node 3 should be in control_nodes (controlled by branch)"
        );
    }

    #[test]
    fn test_forward_slice_self_inclusion() {
        // Scenario: Self-inclusion requirement
        // Expected: Source node is always in results
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[1]).expect("Failed to compute forward slice");

        assert_eq!(
            result.criterion, entity_ids[1],
            "Criterion should be source"
        );
        assert!(result.contains(entity_ids[1]), "Source should be in slice");
    }

    #[test]
    fn test_forward_slice_control_data_separation() {
        // Scenario: Verify both control and data nodes populated
        // Expected: Both sets non-empty for conditional CFG
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        // Should have both control and data nodes
        assert!(
            !result.control_nodes.is_empty(),
            "Control nodes should be non-empty"
        );
        assert!(
            !result.data_nodes.is_empty(),
            "Data nodes should be non-empty"
        );

        // Union should be slice_nodes
        let expected_union: AHashSet<i64> = result
            .control_nodes
            .union(&result.data_nodes)
            .copied()
            .collect();
        assert_eq!(
            result.slice_nodes, expected_union,
            "Slice nodes should be union of control + data"
        );
    }

    // Tests for SliceResult methods

    #[test]
    fn test_slice_result_contains() {
        // Scenario: contains() method works correctly
        // Expected: Returns true for nodes in slice, false otherwise
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        // All nodes should be in slice
        for &node_id in &entity_ids {
            assert!(
                result.contains(node_id),
                "Node {} should be in slice",
                node_id
            );
        }

        // Non-existent node should not be in slice
        assert!(
            !result.contains(9999),
            "Non-existent node should not be in slice"
        );
    }

    #[test]
    fn test_slice_result_sorted_nodes() {
        // Scenario: sorted_nodes() returns deterministic output
        // Expected: Nodes in ascending order
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        let sorted = result.sorted_nodes();

        // Should be sorted (ascending order)
        for i in 1..sorted.len() {
            assert!(
                sorted[i - 1] <= sorted[i],
                "sorted_nodes should be in ascending order"
            );
        }

        // All slice nodes should be present
        assert_eq!(
            sorted.len(),
            result.size,
            "All slice nodes should be in sorted output"
        );
    }

    #[test]
    fn test_slice_result_sorted_control_nodes() {
        // Scenario: sorted_control_nodes() returns deterministic output
        // Expected: Control nodes in ascending order
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        let sorted = result.sorted_control_nodes();

        // Should be sorted
        for i in 1..sorted.len() {
            assert!(
                sorted[i - 1] <= sorted[i],
                "sorted_control_nodes should be in ascending order"
            );
        }

        // All control nodes should be present
        assert_eq!(sorted.len(), result.control_nodes.len());
    }

    #[test]
    fn test_slice_result_sorted_data_nodes() {
        // Scenario: sorted_data_nodes() returns deterministic output
        // Expected: Data nodes in ascending order
        let graph = create_if_then_else_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        let sorted = result.sorted_data_nodes();

        // Should be sorted
        for i in 1..sorted.len() {
            assert!(
                sorted[i - 1] <= sorted[i],
                "sorted_data_nodes should be in ascending order"
            );
        }

        // All data nodes should be present
        assert_eq!(sorted.len(), result.data_nodes.len());
    }

    #[test]
    fn test_backward_slice_nested_cfg() {
        // Scenario: Nested CFG with multi-level control dependence
        // Expected: Correctly captures nested dependencies
        let graph = create_nested_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            backward_slice(&graph, &cdg, entity_ids[4]).expect("Failed to compute backward slice");

        // Node 4 (inner merge) should have control dependencies
        assert!(
            !result.control_nodes.is_empty(),
            "Nested CFG should have control dependencies"
        );

        // Most nodes should be in slice via data flow
        assert!(
            result.size >= 4,
            "Should have at least 4 nodes in slice, got {}",
            result.size
        );
    }

    #[test]
    fn test_forward_slice_nested_cfg() {
        // Scenario: Nested CFG with multi-level control flow
        // Expected: Node 0 controls downstream nodes
        let graph = create_nested_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let result =
            forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed to compute forward slice");

        // All nodes should be in slice
        assert_eq!(result.size, 6, "All 6 nodes should be in slice");

        // Should have control nodes (node 0 controls branches)
        assert!(
            !result.control_nodes.is_empty(),
            "Should have control nodes from branching"
        );
    }

    // Tests for progress variants

    #[test]
    fn test_backward_slice_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results
        use crate::progress::NoProgress;

        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let progress = NoProgress;
        let result_with =
            backward_slice_with_progress(&graph, &cdg, entity_ids[3], &progress).expect("Failed");
        let result_without = backward_slice(&graph, &cdg, entity_ids[3]).expect("Failed");

        assert_eq!(
            result_with.size, result_without.size,
            "Progress and non-progress results should match"
        );

        for &node in &result_with.slice_nodes {
            assert!(
                result_without.contains(node),
                "Progress result contains node not in non-progress result"
            );
        }
    }

    #[test]
    fn test_forward_slice_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results
        use crate::progress::NoProgress;

        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        let progress = NoProgress;
        let result_with =
            forward_slice_with_progress(&graph, &cdg, entity_ids[0], &progress).expect("Failed");
        let result_without = forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed");

        assert_eq!(
            result_with.size, result_without.size,
            "Progress and non-progress results should match"
        );

        for &node in &result_with.slice_nodes {
            assert!(
                result_without.contains(node),
                "Progress result contains node not in non-progress result"
            );
        }
    }

    #[test]
    fn test_backward_forward_symmetry() {
        // Scenario: Test relationship between backward and forward slices
        // Expected: In a linear chain, backward from N should contain same as forward from first
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let cdg = super::super::control_dependence::control_dependence_from_exit(&graph)
            .expect("Failed to compute CDG");

        // In linear chain, backward from last node should contain all nodes
        let backward = backward_slice(&graph, &cdg, entity_ids[3]).expect("Failed");

        // Forward from first node should also contain all nodes
        let forward = forward_slice(&graph, &cdg, entity_ids[0]).expect("Failed");

        // Both should contain all nodes
        assert_eq!(backward.size, 4, "Backward slice should contain all nodes");
        assert_eq!(forward.size, 4, "Forward slice should contain all nodes");

        // Slice nodes should be equal (in linear chain with no control dependence)
        assert_eq!(
            backward.slice_nodes, forward.slice_nodes,
            "In linear chain, backward and forward slices should match"
        );
    }
}
