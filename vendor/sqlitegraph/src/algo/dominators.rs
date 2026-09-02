//! Dominator computation using Cooper et al. simple_fast algorithm.
//!
//! This module provides algorithms for computing dominators and immediate dominators
//! in control flow graphs (CFGs). Dominance is a fundamental concept in compiler
//! optimization, program analysis, and SSA construction.
//!
//! # Dominance Definition
//!
//! A node `d` **dominates** node `n` (written `d dom n`) if every path from the entry
//! node to `n` must pass through `d`. Every node dominates itself, and the entry node
//! dominates all nodes in the graph.
//!
//! The **immediate dominator** of a node `n` (written `idom(n)`) is the unique strict
// dominator of `n` that is closest to `n` in the CFG. If we think of dominance as a
//! tree, the immediate dominator is the parent of `n` in the dominator tree.
//!
//! # Algorithm
//!
//! This module implements Cooper et al.'s **simple_fast** algorithm (2001), an
//! iterative data-flow analysis that computes dominance sets using:
//!
//! 1. **Initialization**: Each node initially dominates all nodes (optimistic start)
//! 2. **Reverse postorder traversal**: Nodes are processed in reverse postorder
//!    to accelerate convergence
//! 3. **Iterative refinement**: For each node, intersect predecessor dominator sets
//!    and union with the node itself until a fixed point is reached
//! 4. **Immediate dominator extraction**: Compute the dominator tree from final
//!    dominance sets by finding each node's closest strict dominator
//!
//! # Complexity
//!
//! - **Time**: O(N²) worst case, O(E) to O(N log N) in practice for typical CFGs
//! - **Space**: O(N²) for dominance sets, O(N) for immediate dominator tree
//!
//! Where:
//! - N = number of vertices
//! - E = number of edges
//!
//! The simple_fast algorithm is simpler than Lengauer-Tarjan but performs well
//! for realistic CFGs. For very large graphs, consider the iterative variant
//! with progress tracking.
//!
//! # When to Use Dominator Analysis
//!
//! ## Compiler Optimization
//!
//! - **SSA Construction**: Compute dominance frontiers for placing φ-nodes
//! - **Loop detection**: Find natural loops using back edges in dominator tree
//! - **Code motion**: Move invariant code out of loops using dominance
//! - **Dead code elimination**: Identify unreachable code from entry
//!
//! ## Program Analysis
//!
//! - **Control flow analysis**: Understand structure of program control flow
//! - **Data flow analysis**: Use dominance to prune data flow constraints
//! - **Slicing**: Compute program slices using dominance relationships
//! - **Impact analysis**: Determine what code affects a given point
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::dominators};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... build CFG with entry node 0 ...
//!
//! let result = dominators(&graph, 0)?;
//!
//! // Check dominance: does 0 dominate 5?
//! let dominates = result.dom.get(&5)
//!     .map(|set| set.contains(&0))
//!     .unwrap_or(false);
//!
//! // Get immediate dominator of node 5
//! let idom = result.idom.get(&5); // Option<&i64>
//!
//! // Traverse dominator tree from entry
//! let mut current = Some(0);
//! while let Some(node) = current {
//!     println!("Node {} in dominator tree", node);
//!     // Find children by checking idom values
//!     current = /* ... */;
//! }
//! ```
//!
//! # References
//!
//! - Cooper, Keith D., Harvey, Timothy J., and Kennedy, Ken. "A simple, fast
//!   dominance algorithm." Software Practice & Experience, 2001.
//! - Cytron, Ron, et al. "Efficiently computing static single assignment form
//!   and the control dependence graph." ACM TOPLAS, 1991.

use ahash::{AHashMap, AHashSet};

use crate::errors::SqliteGraphError;
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

/// Result of dominator computation.
///
/// Contains both full dominance sets and the immediate dominator tree.
/// The dominance sets are complete: for each node `n`, `dom[n]` contains all
/// nodes that dominate `n` (including `n` itself). The immediate dominator
/// tree is a compact representation: `idom[n]` is the parent of `n` in the
/// dominator tree (None for the entry node).
#[derive(Debug, Clone)]
pub struct DominatorResult {
    /// Full dominance sets: node -> set of its dominators.
    ///
    /// For each node `n`, `dom[n]` contains all nodes `d` such that every
    /// path from entry to `n` passes through `d`. Every node dominates itself,
    /// and the entry node dominates all nodes reachable from it.
    pub dom: AHashMap<i64, AHashSet<i64>>,

    /// Immediate dominator tree: node -> immediate dominator.
    ///
    /// For each node `n` (except entry), `idom[n]` is the unique strict dominator
    /// of `n` that is closest to `n`. The entry node has `idom[entry] = None`.
    /// This forms a tree rooted at the entry node.
    pub idom: AHashMap<i64, Option<i64>>,
}

impl DominatorResult {
    /// Checks if one node dominates another.
    ///
    /// Returns `true` if `dominator` dominates `node` (every path from entry to
    /// `node` passes through `dominator`). Every node dominates itself.
    ///
    /// # Arguments
    /// * `dominator` - Potential dominator node ID
    /// * `node` - Node ID to check dominance for
    ///
    /// # Returns
    /// `true` if `dominator` dominates `node`, `false` otherwise.
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = dominators(&graph, entry)?;
    /// assert!(result.dominates(entry, 5)); // Entry dominates all
    /// assert!(result.dominates(5, 5));    // Every node dominates itself
    /// ```
    pub fn dominates(&self, dominator: i64, node: i64) -> bool {
        self.dom
            .get(&node)
            .map(|set| set.contains(&dominator))
            .unwrap_or(false)
    }

    /// Gets the immediate dominator of a node.
    ///
    /// Returns `None` if the node has no immediate dominator (only the entry node
    /// should have `None`). Returns `Some(idom)` if the node has an immediate
    /// dominator.
    ///
    /// # Arguments
    /// * `node` - Node ID to get immediate dominator for
    ///
    /// # Returns
    /// `Some(idom)` if node has an immediate dominator, `None` for entry node.
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = dominators(&graph, entry)?;
    /// assert_eq!(result.immediate_dominator(entry), None); // Entry has no idom
    /// assert!(result.immediate_dominator(child).is_some()); // Others have idom
    /// ```
    pub fn immediate_dominator(&self, node: i64) -> Option<i64> {
        self.idom.get(&node).copied().flatten()
    }

    /// Checks if a node is the entry node (has no immediate dominator).
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// `true` if node is the entry node, `false` otherwise.
    pub fn is_entry(&self, node: i64) -> bool {
        self.idom
            .get(&node)
            .map(|idom| idom.is_none())
            .unwrap_or(false)
    }
}

/// Computes dominators and immediate dominators for a CFG entry node.
///
/// Uses Cooper et al.'s simple_fast iterative algorithm (2001) to compute:
/// - Full dominance sets: for each node, all nodes that dominate it
/// - Immediate dominator tree: each node's closest strict dominator
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `entry` - The entry node ID (must exist in graph)
///
/// # Returns
/// `DominatorResult` containing dominance sets and immediate dominator tree.
///
/// # Algorithm Steps
///
/// 1. **Get all nodes**: Fetch all entity IDs from the graph
/// 2. **Initialize**: Each node dominates all nodes (optimistic), entry dominates only itself
/// 3. **Compute reverse postorder**: DFS from entry, process nodes in postorder
/// 4. **Iterate to fixed point**:
///    - For each node in reverse postorder (except entry):
///      - Intersect all predecessors' dominator sets
///      - Union with {node} (node dominates itself)
///      - Update if changed
/// 5. **Extract immediate dominators**: Find closest strict dominator for each node
///
/// # Complexity
/// - **Time**: O(N²) worst case, O(E) to O(N log N) in practice
/// - **Space**: O(N²) for dominance sets
///
/// # Error Handling
///
/// - Returns `SqliteGraphError::NotFound` if entry node doesn't exist
/// - Handles unreachable nodes gracefully (they have empty/undefined dominators)
/// - Has iteration limit (1000) to prevent infinite loops
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::dominators};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let result = dominators(&graph, 0)?;
///
/// // Check entry dominates all reachable nodes
/// for &node in result.dom.keys() {
///     assert!(result.dominates(0, node));
/// }
///
/// // Immediate dominator forms a tree
/// assert_eq!(result.immediate_dominator(0), None);
/// ```
pub fn dominators(graph: &SqliteGraph, entry: i64) -> Result<DominatorResult, SqliteGraphError> {
    // Get all nodes in the graph
    let all_nodes = graph.all_entity_ids()?;

    // Validate entry exists
    if !all_nodes.contains(&entry) {
        return Err(SqliteGraphError::not_found(format!(
            "Entry node {} not found in graph",
            entry
        )));
    }

    // Initialize dominance sets
    let mut dom = initialize_dominators(&all_nodes, entry);

    // Compute reverse postorder traversal
    let order = reverse_postorder(graph, entry)?;

    // Iterate until fixed point
    let max_iterations = 1000;
    let mut iteration = 0;
    loop {
        let changed = iterate_dominators(graph, &mut dom, &order)?;

        if !changed {
            break;
        }

        iteration += 1;
        if iteration >= max_iterations {
            return Err(SqliteGraphError::query(format!(
                "Dominator computation failed to converge after {} iterations",
                max_iterations
            )));
        }
    }

    // Extract immediate dominators
    let idom = extract_immediate_dominators(&dom, entry);

    Ok(DominatorResult { dom, idom })
}

/// Computes dominators with progress tracking.
///
/// Same algorithm as [`dominators`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `entry` - The entry node ID (must exist in graph)
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `DominatorResult` containing dominance sets and immediate dominator tree.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current iteration number
/// - `total`: None (unknown iterations until convergence)
/// - `message`: "Dominator iteration {current}: {nodes_processed} nodes processed"
///
/// Progress is reported after each iteration completes.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::dominators_with_progress,
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let result = dominators_with_progress(&graph, entry, &progress)?;
/// // Output: Dominator iteration 1: 50 nodes processed...
/// // Output: Dominator iteration 2: 50 nodes processed...
/// ```
pub fn dominators_with_progress<F>(
    graph: &SqliteGraph,
    entry: i64,
    progress: &F,
) -> Result<DominatorResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    // Get all nodes in the graph
    let all_nodes = graph.all_entity_ids()?;

    // Validate entry exists
    if !all_nodes.contains(&entry) {
        return Err(SqliteGraphError::not_found(format!(
            "Entry node {} not found in graph",
            entry
        )));
    }

    // Initialize dominance sets
    let mut dom = initialize_dominators(&all_nodes, entry);

    // Compute reverse postorder traversal
    let order = reverse_postorder(graph, entry)?;

    // Iterate until fixed point
    let max_iterations = 1000;
    let mut iteration = 0;

    loop {
        let changed = iterate_dominators(graph, &mut dom, &order)?;

        // Report progress after each iteration
        progress.on_progress(
            iteration + 1,
            None,
            &format!(
                "Dominator iteration {}: {} nodes processed",
                iteration + 1,
                order.len()
            ),
        );

        if !changed {
            break;
        }

        iteration += 1;
        if iteration >= max_iterations {
            return Err(SqliteGraphError::query(format!(
                "Dominator computation failed to converge after {} iterations",
                max_iterations
            )));
        }
    }

    // Report completion
    progress.on_complete();

    // Extract immediate dominators
    let idom = extract_immediate_dominators(&dom, entry);

    Ok(DominatorResult { dom, idom })
}

/// Initializes dominance sets for the simple_fast algorithm.
///
/// Follows Cooper et al. initialization strategy:
/// - Entry node dominates only itself
/// - All other nodes initially dominate all nodes (optimistic start)
///
/// This optimistic initialization accelerates convergence because the algorithm
/// only removes nodes from dominance sets, never adds them.
fn initialize_dominators(all_nodes: &[i64], entry: i64) -> AHashMap<i64, AHashSet<i64>> {
    let mut dom = AHashMap::new();

    // Create universal set (all nodes)
    let universal: AHashSet<i64> = all_nodes.iter().copied().collect();

    // Initialize each node's dominance set
    for &node in all_nodes {
        if node == entry {
            // Entry dominates only itself
            let mut entry_dom = AHashSet::new();
            entry_dom.insert(entry);
            dom.insert(entry, entry_dom);
        } else {
            // Other nodes initially dominate all nodes (optimistic)
            dom.insert(node, universal.clone());
        }
    }

    dom
}

/// Computes reverse postorder traversal of the CFG starting from entry.
///
/// Uses depth-first search (DFS) to perform a postorder traversal, then
/// reverses it. This ordering accelerates convergence in iterative data-flow
/// analysis because predecessors are typically processed before successors.
///
/// # Arguments
/// * `graph` - The control flow graph
/// * `entry` - The entry node ID
///
/// # Returns
/// Vector of node IDs in reverse postorder (entry first if graph is a tree).
///
/// # Algorithm
/// 1. DFS from entry, marking nodes as visited
/// 2. Add node to postorder list after visiting all successors
/// 3. Reverse the postorder list
/// 4. Filter to only include nodes reachable from entry
///
/// # Note
/// Nodes not reachable from entry are excluded from the ordering. This is
/// correct because unreachable nodes have no paths from entry, so their
/// dominance sets are undefined (they have no predecessors from entry).
fn reverse_postorder(graph: &SqliteGraph, entry: i64) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = AHashSet::new();
    let mut postorder = Vec::new();
    let mut stack = vec![(entry, false)]; // (node, visited_children)

    while let Some((node, visited_children)) = stack.pop() {
        if visited_children {
            // All children visited, add to postorder
            postorder.push(node);
        } else {
            // First time seeing this node, check if already visited
            if !visited.insert(node) {
                // Already visited, skip
                continue;
            }

            // Push it back to add after children
            stack.push((node, true));

            // Push children onto stack (will be visited first)
            let successors = graph.fetch_outgoing(node)?;
            for &succ in successors.iter().rev() {
                // Reverse to maintain order
                if !visited.contains(&succ) {
                    stack.push((succ, false));
                }
            }
        }
    }

    // Reverse to get reverse postorder
    postorder.reverse();
    Ok(postorder)
}

/// Performs one iteration of the dominator data-flow analysis.
///
/// For each node in reverse postorder (except entry):
/// 1. Get all predecessors
/// 2. Intersect their dominance sets (a node is dominated by d only if all
///    predecessors are dominated by d)
/// 3. Add the node itself (every node dominates itself)
/// 4. Update dominance set if changed
///
/// # Arguments
/// * `graph` - The control flow graph
/// * `dom` - Dominance sets (mutable, updated in place)
/// * `order` - Reverse postorder traversal
///
/// # Returns
/// `true` if any dominance set changed, `false` if fixed point reached.
///
/// # Data-Flow Equation
///
/// For each node n (except entry):
/// ```text
/// dom[n] = {n} ∪ (∩_{p ∈ preds(n)} dom[p])
/// ```
///
/// Where:
/// - `dom[n]` is the set of nodes that dominate n
/// - `preds(n)` are the predecessors of n (nodes with edges to n)
/// - `∩` is set intersection
/// - `∪` is set union
fn iterate_dominators(
    graph: &SqliteGraph,
    dom: &mut AHashMap<i64, AHashSet<i64>>,
    order: &[i64],
) -> Result<bool, SqliteGraphError> {
    let mut changed = false;

    // Process nodes in reverse postorder (skip entry)
    for &node in order.iter().skip(1) {
        // Skip entry node
        if node == order.first().copied().unwrap_or(node) {
            continue;
        }

        // Get predecessors
        let predecessors = graph.fetch_incoming(node)?;

        // Handle unreachable nodes (no predecessors from entry)
        if predecessors.is_empty() {
            // Keep existing dominance set (will be all nodes from initialization)
            continue;
        }

        // Intersect predecessors' dominance sets
        let mut new_dom_set: Option<AHashSet<i64>> = None;

        for &pred in &predecessors {
            if let Some(pred_dom) = dom.get(&pred) {
                if let Some(current) = &mut new_dom_set {
                    // Intersect with current set
                    current.retain(|n| pred_dom.contains(n));
                } else {
                    // First predecessor: copy its dominance set
                    new_dom_set = Some(pred_dom.clone());
                }
            }
        }

        // If we have at least one predecessor with dominance set
        if let Some(mut intersected) = new_dom_set {
            // Add node itself (node dominates itself)
            intersected.insert(node);

            // Check if changed
            let old_dom_set = dom.get(&node);
            if old_dom_set.map(|old| old != &intersected).unwrap_or(true) {
                dom.insert(node, intersected);
                changed = true;
            }
        }
    }

    Ok(changed)
}

/// Extracts immediate dominators from full dominance sets.
///
/// The immediate dominator of a node n is the unique strict dominator of n
/// that is dominated by all other strict dominators of n. In other words,
/// it's the strict dominator closest to n in the dominator tree.
///
/// # Arguments
/// * `dom` - Full dominance sets
/// * `entry` - The entry node ID
///
/// # Returns
/// Map from node to its immediate dominator (None for entry).
///
/// # Algorithm
///
/// For each node n (except entry):
/// 1. Get strict dominators: `dom[n] - {n}`
/// 2. Find the strict dominator that is NOT dominated by any other strict dominator
/// 3. This is the immediate dominator (closest strict dominator)
///
/// The entry node has `idom[entry] = None` by definition.
fn extract_immediate_dominators(
    dom: &AHashMap<i64, AHashSet<i64>>,
    entry: i64,
) -> AHashMap<i64, Option<i64>> {
    let mut idom = AHashMap::new();

    // Entry has no immediate dominator
    idom.insert(entry, None);

    // For each other node, find its immediate dominator
    for (&node, dom_set) in dom {
        if node == entry {
            continue;
        }

        // Get strict dominators (all dominators except node itself)
        let strict_dominators: Vec<i64> = dom_set.iter().copied().filter(|&d| d != node).collect();

        if strict_dominators.is_empty() {
            // Node has no strict dominators (shouldn't happen in valid CFG)
            idom.insert(node, None);
            continue;
        }

        // Find immediate dominator: the strict dominator that IS dominated
        // by all other strict dominators (i.e., the one CLOSEST to the node, not the entry).
        // This candidate has the largest dominator set (it's dominated by everyone else).
        let mut immediate_dominator = None;
        let mut max_dom_size = 0;

        for &candidate in &strict_dominators {
            if let Some(candidate_dom) = dom.get(&candidate) {
                // Check if this candidate is dominated by all other strict dominators
                let is_dominated_by_all_others = strict_dominators
                    .iter()
                    .all(|&other| other == candidate || candidate_dom.contains(&other));

                if is_dominated_by_all_others {
                    // This is the immediate dominator (closest to node)
                    immediate_dominator = Some(candidate);
                    break;
                }

                // Fallback: select candidate with largest dominator set
                let dom_size = candidate_dom.len();
                if dom_size > max_dom_size {
                    max_dom_size = dom_size;
                    immediate_dominator = Some(candidate);
                }
            }
        }

        idom.insert(node, immediate_dominator);
    }

    idom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create linear chain graph: 0 -> 1 -> 2 -> 3
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

    /// Helper: Create diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    fn create_diamond_cfg() -> SqliteGraph {
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

        // Create diamond: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
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

    /// Helper: Create CFG with loop: 0 -> 1 -> 2 -> 1
    fn create_loop_cfg() -> SqliteGraph {
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

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create loop: 0 -> 1, 1 -> 2, 2 -> 1
        let edges = vec![(0, 1), (1, 2), (2, 1)];
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

    /// Helper: Create CFG with nested loops
    fn create_nested_loops() -> SqliteGraph {
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

        // Create nested loops: 0 -> 1 -> 2 -> 3, 3 -> 2 (inner), 3 -> 1 (outer)
        let edges = vec![(0, 1), (1, 2), (2, 3), (3, 2), (3, 1)];
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

    /// Helper: Create CFG with multiple exits
    fn create_multiple_exits() -> SqliteGraph {
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

        // Create multiple exits: 0 -> 1 -> 2, 0 -> 1 -> 3
        let edges = vec![(0, 1), (1, 2), (1, 3)];
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

    #[test]
    fn test_dominators_linear_chain() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: 0 dominates all, 1 dominates {1,2,3}, 2 dominates {2,3}, 3 dominates {3}
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Entry dominates all nodes
        for &node in &entity_ids {
            assert!(
                result.dominates(entry, node),
                "Entry should dominate node {}",
                node
            );
        }

        // Node 1 should dominate {1, 2, 3}
        assert!(result.dominates(entity_ids[1], entity_ids[1]));
        assert!(result.dominates(entity_ids[1], entity_ids[2]));
        assert!(result.dominates(entity_ids[1], entity_ids[3]));

        // Node 2 should dominate {2, 3}
        assert!(result.dominates(entity_ids[2], entity_ids[2]));
        assert!(result.dominates(entity_ids[2], entity_ids[3]));

        // Node 3 should dominate only itself
        assert!(result.dominates(entity_ids[3], entity_ids[3]));
        assert!(!result.dominates(entity_ids[3], entity_ids[0]));
    }

    #[test]
    fn test_dominators_diamond() {
        // Scenario: Diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Entity IDs: [1, 2, 3, 4] where 1 is entry, 4 is exit
        // Edges: 1->2, 1->3, 2->4, 3->4
        // Expected:
        // - Node 1 dominates all (on all paths from entry)
        // - Node 2 dominates only itself (path 1->3->4 doesn't go through 2)
        // - Node 3 dominates only itself (path 1->2->4 doesn't go through 3)
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Entry dominates all
        for &node in &entity_ids {
            assert!(
                result.dominates(entry, node),
                "Entry should dominate {}",
                node
            );
        }

        // Node 2 dominates only itself (not 3 or 4, since there's a path 1->3->4)
        assert!(
            result.dominates(entity_ids[1], entity_ids[1]),
            "Node 2 should dominate itself"
        );
        assert!(
            !result.dominates(entity_ids[1], entity_ids[3]),
            "Node 2 should NOT dominate exit"
        );
        assert!(
            !result.dominates(entity_ids[1], entity_ids[2]),
            "Node 2 should NOT dominate node 3"
        );

        // Node 3 dominates only itself (not 2 or 4, since there's a path 1->2->4)
        assert!(
            result.dominates(entity_ids[2], entity_ids[2]),
            "Node 3 should dominate itself"
        );
        assert!(
            !result.dominates(entity_ids[2], entity_ids[3]),
            "Node 3 should NOT dominate exit"
        );
        assert!(
            !result.dominates(entity_ids[2], entity_ids[1]),
            "Node 3 should NOT dominate node 2"
        );
    }

    #[test]
    fn test_dominators_loop() {
        // Scenario: Loop CFG: 0 -> 1 -> 2 -> 1
        // Expected: Loop header (1) dominates loop body
        let graph = create_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Node 1 (loop header) should dominate nodes 1 and 2
        assert!(result.dominates(entity_ids[1], entity_ids[1]));
        assert!(result.dominates(entity_ids[1], entity_ids[2]));

        // Node 2 is in the loop, doesn't dominate others
        assert!(result.dominates(entity_ids[2], entity_ids[2]));
        assert!(!result.dominates(entity_ids[2], entity_ids[1]));
    }

    #[test]
    fn test_immediate_dominators_linear() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: idom(0) = None, idom(1) = 0, idom(2) = 1, idom(3) = 2
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        eprintln!("Entity IDs: {:?}", entity_ids);
        eprintln!("Entry: {}", entry);
        eprintln!("Dominators: {:?}", result.dom);
        eprintln!("IDOM: {:?}", result.idom);

        assert_eq!(
            result.immediate_dominator(entry),
            None,
            "Entry should have no immediate dominator"
        );
        assert_eq!(
            result.immediate_dominator(entity_ids[1]),
            Some(entry),
            "idom(1) should be 0"
        );
        assert_eq!(
            result.immediate_dominator(entity_ids[2]),
            Some(entity_ids[1]),
            "idom(2) should be 1"
        );
        assert_eq!(
            result.immediate_dominator(entity_ids[3]),
            Some(entity_ids[2]),
            "idom(3) should be 2"
        );
    }

    #[test]
    fn test_immediate_dominators_diamond() {
        // Scenario: Diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Expected: idom(0) = None, idom(1) = 0, idom(2) = 0, idom(3) = 0
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        assert_eq!(result.immediate_dominator(entry), None);
        assert_eq!(
            result.immediate_dominator(entity_ids[1]),
            Some(entry),
            "idom(1) should be entry"
        );
        assert_eq!(
            result.immediate_dominator(entity_ids[2]),
            Some(entry),
            "idom(2) should be entry"
        );
        assert_eq!(
            result.immediate_dominator(entity_ids[3]),
            Some(entry),
            "idom(3) should be entry (both paths converge)"
        );
    }

    #[test]
    fn test_dominators_single_node() {
        // Scenario: Single node graph
        // Expected: Node dominates only itself
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

        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Entry dominates only itself
        assert!(result.dominates(entry, entry));
        assert_eq!(result.immediate_dominator(entry), None);
        assert_eq!(result.dom.len(), 1, "Should have 1 node in dom sets");
    }

    #[test]
    fn test_dominators_empty_graph() {
        // Scenario: Empty graph
        // Expected: Returns error (entry not found)
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = dominators(&graph, 999);
        assert!(result.is_err(), "Should fail on empty graph");
    }

    #[test]
    fn test_dominators_nonexistent_entry() {
        // Scenario: Entry node doesn't exist
        // Expected: Returns NotFound error
        let graph = create_linear_chain();

        let result = dominators(&graph, 999);
        assert!(result.is_err(), "Should fail for nonexistent entry");

        if let Err(SqliteGraphError::NotFound(msg)) = result {
            assert!(msg.contains("999"), "Error should mention node 999");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[test]
    fn test_dominators_self_dominance() {
        // Scenario: Every node should dominate itself
        // Expected: For all nodes n, n ∈ dom[n]
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        for &node in &entity_ids {
            assert!(
                result.dominates(node, node),
                "Node {} should dominate itself",
                node
            );
        }
    }

    #[test]
    fn test_dominators_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results, progress callback called
        use crate::progress::NoProgress;

        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let progress = NoProgress;
        let result_with = dominators_with_progress(&graph, entry, &progress).expect("Failed");
        let result_without = dominators(&graph, entry).expect("Failed");

        // Check dominance sets match
        assert_eq!(
            result_with.dom.len(),
            result_without.dom.len(),
            "Should have same number of nodes"
        );

        for (&node, dom_set) in &result_without.dom {
            assert!(
                result_with.dom.contains_key(&node),
                "Progress result missing node {}",
                node
            );
            assert_eq!(
                result_with.dom.get(&node),
                Some(dom_set),
                "Dominance sets differ for node {}",
                node
            );
        }

        // Check immediate dominators match
        assert_eq!(result_with.idom, result_without.idom);
    }

    #[test]
    fn test_dominators_is_entry() {
        // Scenario: Check is_entry method
        // Expected: Only entry node returns true
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        assert!(
            result.is_entry(entry),
            "Entry should be identified as entry"
        );
        assert!(
            !result.is_entry(entity_ids[1]),
            "Non-entry should not be entry"
        );
        assert!(
            !result.is_entry(entity_ids[2]),
            "Non-entry should not be entry"
        );
        assert!(
            !result.is_entry(entity_ids[3]),
            "Non-entry should not be entry"
        );
    }

    #[test]
    fn test_dominators_nested_loops() {
        // Scenario: Nested loops CFG
        // Expected: Outer loop header dominates inner loop
        let graph = create_nested_loops();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Entry (0) should dominate all
        for &node in &entity_ids {
            assert!(
                result.dominates(entry, node),
                "Entry should dominate {}",
                node
            );
        }

        // Node 1 (outer loop header) should dominate 1, 2, 3
        assert!(result.dominates(entity_ids[1], entity_ids[1]));
        assert!(result.dominates(entity_ids[1], entity_ids[2]));
        assert!(result.dominates(entity_ids[1], entity_ids[3]));

        // Node 2 (inner loop header) should dominate 2, 3
        assert!(result.dominates(entity_ids[2], entity_ids[2]));
        assert!(result.dominates(entity_ids[2], entity_ids[3]));
    }

    #[test]
    fn test_dominators_multiple_exits() {
        // Scenario: CFG with multiple exits: 0 -> 1 -> 2, 0 -> 1 -> 3
        // Entity IDs: [1, 2, 3, 4] where:
        // - 1 is entry
        // - 2 is common node (on both paths)
        // - 3 and 4 are exits
        // Edges: 1->2, 2->3, 2->4
        let graph = create_multiple_exits();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let result = dominators(&graph, entry).expect("Failed to compute dominators");

        // Entry dominates all nodes (on all paths from entry)
        for &node in &entity_ids {
            assert!(
                result.dominates(entry, node),
                "Entry should dominate {}",
                node
            );
        }

        // Node 2 dominates itself and exits (on paths from entry through 2)
        assert!(
            result.dominates(entity_ids[1], entity_ids[1]),
            "Node 2 should dominate itself"
        );
        assert!(
            result.dominates(entity_ids[1], entity_ids[2]),
            "Node 2 should dominate exit 3"
        );
        assert!(
            result.dominates(entity_ids[1], entity_ids[3]),
            "Node 2 should dominate exit 4"
        );

        // Exits don't dominate each other (different paths)
        assert!(
            !result.dominates(entity_ids[2], entity_ids[3]),
            "Exit 3 should NOT dominate exit 4"
        );
        assert!(
            !result.dominates(entity_ids[3], entity_ids[2]),
            "Exit 4 should NOT dominate exit 3"
        );
    }
}
