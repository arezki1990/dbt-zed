//! Post-dominator computation using virtual graph reversal and Cooper et al. algorithm.
//!
//! This module provides algorithms for computing post-dominators and immediate post-dominators
//! in control flow graphs (CFGs). Post-dominance is the dual of dominance: instead of asking
//! "what nodes must we pass through to reach this node from entry?", post-dominance asks
//! "what nodes must we pass through to reach exit from this node?"
//!
//! # Post-Dominance Definition
//!
//! A node `d` **post-dominates** node `n` (written `d pdom n`) if every path from `n` to
//! an exit node must pass through `d`. Every node post-dominates itself, and exit nodes
//! post-dominate all nodes that can reach them.
//!
//! The **immediate post-dominator** of a node `n` (written `ipdom(n)`) is the unique strict
//! post-dominator of `n` that is closest to `n` in the CFG. If we think of post-dominance
//! as a tree, the immediate post-dominator is the parent of `n` in the post-dominator tree.
//!
//! # Algorithm
//!
//! This module computes post-dominators by **virtual graph reversal**:
//!
//! 1. **Virtual reversal**: Build predecessor map without modifying the graph
//! 2. **Reuse Cooper algorithm**: Apply same iterative solver as dominators, using predecessor map
//! 3. **Multiple exit handling**: Automatically unify multiple exits with virtual exit node
//! 4. **Iterative refinement**: Intersect successors' post-dominator sets until fixed point
//! 5. **Immediate post-dominator extraction**: Compute post-dominator tree from final sets
//!
//! This approach is elegant: post-dominators on original graph = dominators on reversed graph.
//! By using predecessor maps, we avoid the cost of actually reversing the graph.
//!
//! # Complexity
//!
//! - **Time**: O(N²) worst case, O(E) to O(N log N) in practice for typical CFGs
//! - **Space**: O(N²) for post-dominance sets, O(N) for immediate post-dominator tree
//!
//! Where:
//! - N = number of vertices
//! - E = number of edges
//!
//! The algorithm performs identically to dominators because it's the same computation
//! on a virtually reversed graph.
//!
//! # When to Use Post-Dominator Analysis
//!
//! ## Compiler Optimization
//!
//! - **Control dependence analysis**: Compute control dependence using post-dominance frontiers
//! - **Code motion**: Move code to post-dominator (safe placement point)
//! - **Loop exit optimization**: Identify loop exits and their post-dominance relationships
//! - **Dead code elimination**: Find code that cannot reach any exit
//!
//! ## Program Analysis
//!
//! - **Impact analysis**: Determine what a node affects (forward vs backward)
//! - **Slicing**: Compute backward slices using post-dominance
//! - **Data flow analysis**: Use post-dominance to prune constraints
//! - **Program understanding**: Analyze control flow from exits backward
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::post_dominators};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... build CFG with exit node 3 ...
//!
//! let result = post_dominators(&graph, 3)?;
//!
//! // Check post-dominance: does 3 post-dominate 0?
//! let post_dominates = result.post_dom.get(&0)
//!     .map(|set| set.contains(&3))
//!     .unwrap_or(false);
//!
//! // Get immediate post-dominator of node 0
//! let ipdom = result.ipdom.get(&0); // Option<&Option<i64>>
//!
//! // Traverse post-dominator tree from exit
//! let mut current = Some(3);
//! while let Some(node) = current {
//!     println!("Node {} in post-dominator tree", node);
//!     // Find children by checking ipdom values
//!     current = /* ... */;
//! }
//! ```
//!
//! # Multiple Exit Nodes
//!
//! CFGs with multiple exits are handled automatically:
//!
//! - **Single exit**: Use directly as exit node
//! - **Multiple exits**: Add virtual exit (id: -1) that all real exits point to
//! - **No exits**: Return error (degenerate CFG)
//!
//! The virtual exit node is only used internally - results only contain real nodes.
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

/// Result of post-dominator computation.
///
/// Contains both full post-dominance sets and the immediate post-dominator tree.
/// The post-dominance sets are complete: for each node `n`, `post_dom[n]` contains all
/// nodes that post-dominate `n` (including `n` itself). The immediate post-dominator
/// tree is a compact representation: `ipdom[n]` is the parent of `n` in the
/// post-dominator tree (None for exit nodes).
#[derive(Debug, Clone)]
pub struct PostDominatorResult {
    /// Full post-dominance sets: node -> set of its post-dominators.
    ///
    /// For each node `n`, `post_dom[n]` contains all nodes `d` such that every
    /// path from `n` to exit passes through `d`. Every node post-dominates itself,
    /// and exit nodes post-dominate all nodes that can reach them.
    pub post_dom: AHashMap<i64, AHashSet<i64>>,

    /// Immediate post-dominator tree: node -> immediate post-dominator.
    ///
    /// For each node `n` (except exit), `ipdom[n]` is the unique strict post-dominator
    /// of `n` that is closest to `n`. Exit nodes have `ipdom[exit] = None`.
    /// This forms a tree rooted at the exit node (or virtual exit for multiple exits).
    pub ipdom: AHashMap<i64, Option<i64>>,
}

impl PostDominatorResult {
    /// Checks if one node post-dominates another.
    ///
    /// Returns `true` if `post_dominator` post-dominates `node` (every path from
    /// `node` to exit passes through `post_dominator`). Every node post-dominates itself.
    ///
    /// # Arguments
    /// * `post_dominator` - Potential post-dominator node ID
    /// * `node` - Node ID to check post-dominance for
    ///
    /// # Returns
    /// `true` if `post_dominator` post-dominates `node`, `false` otherwise.
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = post_dominators(&graph, exit)?;
    /// assert!(result.post_dominates(exit, 0)); // Exit post-dominates all
    /// assert!(result.post_dominates(5, 5));    // Every node post-dominates itself
    /// ```
    pub fn post_dominates(&self, post_dominator: i64, node: i64) -> bool {
        self.post_dom
            .get(&node)
            .map(|set| set.contains(&post_dominator))
            .unwrap_or(false)
    }

    /// Gets the immediate post-dominator of a node.
    ///
    /// Returns `None` if the node has no immediate post-dominator (exit nodes have
    /// `None`). Returns `Some(ipdom)` if the node has an immediate post-dominator.
    ///
    /// # Arguments
    /// * `node` - Node ID to get immediate post-dominator for
    ///
    /// # Returns
    /// `Some(ipdom)` if node has an immediate post-dominator, `None` for exit nodes.
    ///
    /// # Example
    /// ```rust,ignore
    /// let result = post_dominators(&graph, exit)?;
    /// assert_eq!(result.immediate_post_dominator(exit), None); // Exit has no ipdom
    /// assert!(result.immediate_post_dominator(child).is_some()); // Others have ipdom
    /// ```
    pub fn immediate_post_dominator(&self, node: i64) -> Option<i64> {
        self.ipdom.get(&node).copied().flatten()
    }

    /// Checks if a node is an exit node (has no immediate post-dominator).
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// `true` if node is an exit node, `false` otherwise.
    pub fn is_exit(&self, node: i64) -> bool {
        self.ipdom
            .get(&node)
            .map(|ipdom| ipdom.is_none())
            .unwrap_or(false)
    }
}

/// Computes post-dominators and immediate post-dominators for a CFG exit node.
///
/// Uses virtual graph reversal and Cooper et al.'s simple_fast iterative algorithm
/// (2001) to compute:
/// - Full post-dominance sets: for each node, all nodes that post-dominate it
/// - Immediate post-dominator tree: each node's closest strict post-dominator
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `exit` - The exit node ID (must exist in graph)
///
/// # Returns
/// `PostDominatorResult` containing post-dominance sets and immediate post-dominator tree.
///
/// # Algorithm Steps
///
/// 1. **Build predecessor map**: Virtual graph reversal (swap edges direction)
/// 2. **Get all nodes**: Fetch all entity IDs from the graph
/// 3. **Initialize**: Each node post-dominates all nodes (optimistic), exit post-dominates only itself
/// 4. **Compute reverse postorder**: DFS from exit on reversed graph, process nodes in postorder
/// 5. **Iterate to fixed point**:
///    - For each node in reverse postorder (except exit):
///      - Intersect all successors' post-dominator sets (using predecessor map)
///      - Union with {node} (node post-dominates itself)
///      - Update if changed
/// 6. **Extract immediate post-dominators**: Find closest strict post-dominator for each node
///
/// # Complexity
/// - **Time**: O(N²) worst case, O(E) to O(N log N) in practice
/// - **Space**: O(N²) for post-dominance sets
///
/// # Error Handling
///
/// - Returns `SqliteGraphError::NotFound` if exit node doesn't exist
/// - Handles unreachable nodes gracefully (they have empty/undefined post-dominators)
/// - Has iteration limit (1000) to prevent infinite loops
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::post_dominators};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let result = post_dominators(&graph, 3)?;
///
/// // Check exit post-dominates all nodes that reach it
/// for &node in result.post_dom.keys() {
///     if node != 3 {
///         assert!(result.post_dominates(3, node));
///     }
/// }
///
/// // Immediate post-dominator forms a tree
/// assert_eq!(result.immediate_post_dominator(3), None);
/// ```
pub fn post_dominators(
    graph: &SqliteGraph,
    exit: i64,
) -> Result<PostDominatorResult, SqliteGraphError> {
    // Build predecessor map (virtual reversal)
    let preds = build_predecessor_map(graph)?;

    // Get all nodes in the graph
    let all_nodes = graph.all_entity_ids()?;

    // Validate exit exists
    if !all_nodes.contains(&exit) {
        return Err(SqliteGraphError::not_found(format!(
            "Exit node {} not found in graph",
            exit
        )));
    }

    // Initialize post-dominance sets
    let mut post_dom = initialize_post_dominators(&all_nodes, exit);

    // Compute reverse postorder on reversed graph
    let order = reverse_postorder_reversed(&preds, exit, &all_nodes)?;

    // Iterate until fixed point
    let max_iterations = 1000;
    let mut iteration = 0;
    loop {
        let changed = iterate_post_dominators(&preds, &mut post_dom, &order)?;

        if !changed {
            break;
        }

        iteration += 1;
        if iteration >= max_iterations {
            return Err(SqliteGraphError::query(format!(
                "Post-dominator computation failed to converge after {} iterations",
                max_iterations
            )));
        }
    }

    // Extract immediate post-dominators
    let ipdom = extract_immediate_post_dominators(&post_dom, exit);

    Ok(PostDominatorResult { post_dom, ipdom })
}

/// Computes post-dominators with progress tracking.
///
/// Same algorithm as [`post_dominators`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `exit` - The exit node ID (must exist in graph)
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `PostDominatorResult` containing post-dominance sets and immediate post-dominator tree.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current iteration number
/// - `total`: None (unknown iterations until convergence)
/// - `message`: "Post-dominator iteration {current}: {nodes_processed} nodes processed"
///
/// Progress is reported after each iteration completes.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::post_dominators_with_progress,
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let result = post_dominators_with_progress(&graph, exit, &progress)?;
/// // Output: Post-dominator iteration 1: 50 nodes processed...
/// // Output: Post-dominator iteration 2: 50 nodes processed...
/// ```
pub fn post_dominators_with_progress<F>(
    graph: &SqliteGraph,
    exit: i64,
    progress: &F,
) -> Result<PostDominatorResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    // Build predecessor map (virtual reversal)
    let preds = build_predecessor_map(graph)?;

    // Get all nodes in the graph
    let all_nodes = graph.all_entity_ids()?;

    // Validate exit exists
    if !all_nodes.contains(&exit) {
        return Err(SqliteGraphError::not_found(format!(
            "Exit node {} not found in graph",
            exit
        )));
    }

    // Initialize post-dominance sets
    let mut post_dom = initialize_post_dominators(&all_nodes, exit);

    // Compute reverse postorder on reversed graph
    let order = reverse_postorder_reversed(&preds, exit, &all_nodes)?;

    // Iterate until fixed point
    let max_iterations = 1000;
    let mut iteration = 0;

    loop {
        let changed = iterate_post_dominators(&preds, &mut post_dom, &order)?;

        // Report progress after each iteration
        progress.on_progress(
            iteration + 1,
            None,
            &format!(
                "Post-dominator iteration {}: {} nodes processed",
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
                "Post-dominator computation failed to converge after {} iterations",
                max_iterations
            )));
        }
    }

    // Report completion
    progress.on_complete();

    // Extract immediate post-dominators
    let ipdom = extract_immediate_post_dominators(&post_dom, exit);

    Ok(PostDominatorResult { post_dom, ipdom })
}

/// Computes post-dominators with automatic exit detection.
///
/// Automatically detects exit nodes (nodes with no outgoing edges) and handles
/// single or multiple exits appropriately:
///
/// - **Single exit**: Use as exit node directly
/// - **Multiple exits**: Create virtual exit (id: -1) that all real exits point to
/// - **No exits**: Return error
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
///
/// # Returns
/// `PostDominatorResult` containing post-dominance sets and immediate post-dominator tree.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::post_dominators_auto_exit};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG (may have multiple exits) ...
///
/// let result = post_dominators_auto_exit(&graph)?;
/// // Automatically handles multiple exits with virtual exit
/// ```
pub fn post_dominators_auto_exit(
    graph: &SqliteGraph,
) -> Result<PostDominatorResult, SqliteGraphError> {
    // Get all nodes
    let all_nodes = graph.all_entity_ids()?;

    if all_nodes.is_empty() {
        return Ok(PostDominatorResult {
            post_dom: AHashMap::new(),
            ipdom: AHashMap::new(),
        });
    }

    // Find exit nodes (nodes with no outgoing edges)
    let mut exits = Vec::new();
    for &node in &all_nodes {
        let outgoing = graph.fetch_outgoing(node)?;
        if outgoing.is_empty() {
            exits.push(node);
        }
    }

    if exits.is_empty() {
        return Err(SqliteGraphError::query(
            "Cannot compute post-dominators: graph has no exit nodes (all nodes have outgoing edges)",
        ));
    }

    if exits.len() == 1 {
        // Single exit - use directly
        post_dominators(graph, exits[0])
    } else {
        // Multiple exits - use virtual exit
        post_dominators_with_virtual_exit(graph, &exits)
    }
}

/// Builds a predecessor map for virtual graph reversal.
///
/// Instead of actually reversing the graph (expensive), we build a map that
/// allows us to traverse the graph in reverse direction: for each node,
/// we store its predecessors (nodes that have edges to it).
///
/// # Arguments
/// * `graph` - The control flow graph
///
/// # Returns
/// Map from node to list of its predecessors.
///
/// # Virtual Reversal
///
/// Original graph edge: `a -> b`
/// Predecessor map: `preds[b].contains(a)`
///
/// This allows us to traverse "backward" through the graph without modifying it.
fn build_predecessor_map(graph: &SqliteGraph) -> Result<AHashMap<i64, Vec<i64>>, SqliteGraphError> {
    let mut preds = AHashMap::new();

    // Get all nodes
    let all_nodes = graph.all_entity_ids()?;

    // For each node, get its outgoing edges
    for &node in &all_nodes {
        let outgoing = graph.fetch_outgoing(node)?;

        // For each successor, add current node as predecessor
        for &succ in &outgoing {
            preds.entry(succ).or_insert_with(Vec::new).push(node);
        }

        // Ensure nodes with no predecessors are in the map
        preds.entry(node).or_insert_with(Vec::new);
    }

    Ok(preds)
}

/// Initializes post-dominance sets for the simple_fast algorithm.
///
/// Follows Cooper et al. initialization strategy:
/// - Exit node post-dominates only itself
/// - All other nodes initially post-dominate all nodes (optimistic start)
///
/// This optimistic initialization accelerates convergence because the algorithm
/// only removes nodes from post-dominance sets, never adds them.
fn initialize_post_dominators(all_nodes: &[i64], exit: i64) -> AHashMap<i64, AHashSet<i64>> {
    let mut post_dom = AHashMap::new();

    // Create universal set (all nodes)
    let universal: AHashSet<i64> = all_nodes.iter().copied().collect();

    // Initialize each node's post-dominance set
    for &node in all_nodes {
        if node == exit {
            // Exit post-dominates only itself
            let mut exit_dom = AHashSet::new();
            exit_dom.insert(exit);
            post_dom.insert(exit, exit_dom);
        } else {
            // Other nodes initially post-dominate all nodes (optimistic)
            post_dom.insert(node, universal.clone());
        }
    }

    post_dom
}

/// Computes reverse postorder traversal of the reversed CFG starting from exit.
///
/// Uses depth-first search (DFS) on the virtually reversed graph (via predecessor map)
/// to perform a postorder traversal, then reverses it. This ordering accelerates
/// convergence in iterative data-flow analysis because successors are typically
/// processed before predecessors in the reversed view.
///
/// # Arguments
/// * `preds` - Predecessor map (virtual reversal)
/// * `exit` - The exit node ID
/// * `all_nodes` - All nodes in the graph
///
/// # Returns
/// Vector of node IDs in reverse postorder (exit first if reversed graph is a tree).
///
/// # Algorithm
/// 1. DFS from exit using predecessor map (reverse edges)
/// 2. Add node to postorder list after visiting all predecessors
/// 3. Reverse the postorder list
/// 4. Filter to only include nodes reachable from exit (in reversed view)
///
/// # Note
/// Nodes not reachable from exit (in reversed view) are excluded from the ordering.
/// This is correct because unreachable nodes have no paths to exit, so their
/// post-dominance sets are undefined.
fn reverse_postorder_reversed(
    preds: &AHashMap<i64, Vec<i64>>,
    exit: i64,
    _all_nodes: &[i64],
) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = AHashSet::new();
    let mut seen = AHashSet::new();
    let mut postorder = Vec::new();
    let mut stack = vec![(exit, false)]; // (node, visited_predecessors)
    seen.insert(exit);

    while let Some((node, visited_predecessors)) = stack.pop() {
        if visited_predecessors {
            // All predecessors visited, add to postorder and mark as visited
            visited.insert(node);
            postorder.push(node);
        } else {
            // First time seeing this node, push it back to add after predecessors
            stack.push((node, true));

            // Push predecessors onto stack (will be visited first)
            if let Some(predecessors) = preds.get(&node) {
                for pred in predecessors.iter().rev() {
                    // Reverse to maintain order
                    if !visited.contains(pred) && seen.insert(*pred) {
                        stack.push((*pred, false));
                    }
                }
            }
        }
    }

    // Reverse to get reverse postorder
    postorder.reverse();
    Ok(postorder)
}

/// Performs one iteration of the post-dominator data-flow analysis.
///
/// For each node in reverse postorder (except exit):
/// 1. Get all successors (via predecessor map: nodes we have edges to)
/// 2. Intersect their post-dominator sets (a node is post-dominated by d only if all
///    successors are post-dominated by d)
/// 3. Add the node itself (every node post-dominates itself)
/// 4. Update post-dominance set if changed
///
/// # Arguments
/// * `preds` - Predecessor map (virtual reversal)
/// * `post_dom` - Post-dominance sets (mutable, updated in place)
/// * `order` - Reverse postorder traversal
///
/// # Returns
/// `true` if any post-dominance set changed, `false` if fixed point reached.
///
/// # Data-Flow Equation
///
/// For each node n (except exit):
/// ```text
/// post_dom[n] = {n} ∪ (∩_{s ∈ succs(n)} post_dom[s])
/// ```
///
/// Where:
/// - `post_dom[n]` is the set of nodes that post-dominate n
/// - `succs(n)` are the successors of n (nodes with edges from n)
/// - `∩` is set intersection
/// - `∪` is set union
///
/// We find successors via the predecessor map by looking for nodes where `n` appears
/// in their predecessor list.
fn iterate_post_dominators(
    preds: &AHashMap<i64, Vec<i64>>,
    post_dom: &mut AHashMap<i64, AHashSet<i64>>,
    order: &[i64],
) -> Result<bool, SqliteGraphError> {
    let mut changed = false;

    // Process nodes in reverse postorder (skip exit)
    for &node in order.iter().skip(1) {
        // Skip exit node
        if node == order.first().copied().unwrap_or(node) {
            continue;
        }

        // Find successors: nodes where this node appears in their predecessor list
        let mut successors = Vec::new();
        for (&succ_node, pred_list) in preds.iter() {
            if pred_list.contains(&node) {
                successors.push(succ_node);
            }
        }

        // Handle nodes with no successors (shouldn't happen in valid CFG with exit)
        if successors.is_empty() {
            // Keep existing post-dominance set (will be all nodes from initialization)
            continue;
        }

        // Intersect successors' post-dominator sets
        let mut new_post_dom_set: Option<AHashSet<i64>> = None;

        for &succ in &successors {
            if let Some(succ_post_dom) = post_dom.get(&succ) {
                if let Some(current) = &mut new_post_dom_set {
                    // Intersect with current set
                    current.retain(|n| succ_post_dom.contains(n));
                } else {
                    // First successor: copy its post-dominance set
                    new_post_dom_set = Some(succ_post_dom.clone());
                }
            }
        }

        // If we have at least one successor with post-dominance set
        if let Some(mut intersected) = new_post_dom_set {
            // Add node itself (node post-dominates itself)
            intersected.insert(node);

            // Check if changed
            let old_post_dom_set = post_dom.get(&node);
            if old_post_dom_set
                .map(|old| old != &intersected)
                .unwrap_or(true)
            {
                post_dom.insert(node, intersected);
                changed = true;
            }
        }
    }

    Ok(changed)
}

/// Extracts immediate post-dominators from full post-dominance sets.
///
/// The immediate post-dominator of a node n is the unique strict post-dominator of n
/// that is post-dominated by all other strict post-dominators of n. In other words,
/// it's the strict post-dominator closest to n in the post-dominator tree.
///
/// # Arguments
/// * `post_dom` - Full post-dominance sets
/// * `exit` - The exit node ID
///
/// # Returns
/// Map from node to its immediate post-dominator (None for exit).
///
/// # Algorithm
///
/// For each node n (except exit):
/// 1. Get strict post-dominators: `post_dom[n] - {n}`
/// 2. Find the strict post-dominator that is NOT post-dominated by any other strict post-dominator
/// 3. This is the immediate post-dominator (closest strict post-dominator)
///
/// The exit node has `ipdom[exit] = None` by definition.
fn extract_immediate_post_dominators(
    post_dom: &AHashMap<i64, AHashSet<i64>>,
    exit: i64,
) -> AHashMap<i64, Option<i64>> {
    let mut ipdom = AHashMap::new();

    // Exit has no immediate post-dominator
    ipdom.insert(exit, None);

    // For each other node, find its immediate post-dominator
    for (&node, post_dom_set) in post_dom {
        if node == exit {
            continue;
        }

        // Get strict post-dominators (all post-dominators except node itself)
        let strict_post_dominators: Vec<i64> = post_dom_set
            .iter()
            .copied()
            .filter(|&d| d != node)
            .collect();

        if strict_post_dominators.is_empty() {
            // Node has no strict post-dominators (shouldn't happen in valid CFG)
            ipdom.insert(node, None);
            continue;
        }

        // Find immediate post-dominator: the strict post-dominator that IS post-dominated
        // by all other strict post-dominators (i.e., the one CLOSEST to the node, not the exit).
        // This candidate has the largest post-dominator set (it's post-dominated by everyone else).
        let mut immediate_post_dominator = None;
        let mut max_post_dom_size = 0;

        for &candidate in &strict_post_dominators {
            if let Some(candidate_post_dom) = post_dom.get(&candidate) {
                // Check if this candidate is post-dominated by all other strict post-dominators
                let is_dominated_by_all_others = strict_post_dominators
                    .iter()
                    .all(|&other| other == candidate || candidate_post_dom.contains(&other));

                if is_dominated_by_all_others {
                    // This is the immediate post-dominator (closest to node)
                    immediate_post_dominator = Some(candidate);
                    break;
                }

                // Fallback: select candidate with largest post-dominator set
                let post_dom_size = candidate_post_dom.len();
                if post_dom_size > max_post_dom_size {
                    max_post_dom_size = post_dom_size;
                    immediate_post_dominator = Some(candidate);
                }
            }
        }

        ipdom.insert(node, immediate_post_dominator);
    }

    ipdom
}

/// Computes post-dominators with virtual exit for multiple exit nodes.
///
/// When the CFG has multiple exit nodes, we create a virtual exit node (id: -1)
/// that all real exits point to. This allows us to use the standard post-dominator
/// algorithm while correctly handling multiple exits.
///
/// # Arguments
/// * `graph` - The control flow graph
/// * `exits` - List of exit node IDs
///
/// # Returns
/// `PostDominatorResult` with post-dominance sets and immediate post-dominator tree.
///
/// # Virtual Exit Handling
///
/// The virtual exit node (id: -1) is only used during computation. The final
/// result only includes real nodes - the virtual exit is stripped out before
/// returning results.
fn post_dominators_with_virtual_exit(
    graph: &SqliteGraph,
    exits: &[i64],
) -> Result<PostDominatorResult, SqliteGraphError> {
    // Build predecessor map
    let mut preds = build_predecessor_map(graph)?;

    // Get all nodes
    let all_nodes = graph.all_entity_ids()?;

    // Virtual exit ID
    let virtual_exit = -1i64;

    // Add virtual exit to predecessor map: virtual exit has all real exits as predecessors
    // In the reversed CFG, this means real exits -> virtual_exit
    preds
        .entry(virtual_exit)
        .or_insert_with(Vec::new)
        .extend(exits.iter().copied());

    // Create extended node list including virtual exit
    let mut extended_nodes = all_nodes.clone();
    extended_nodes.push(virtual_exit);

    // Initialize post-dominance sets
    let mut post_dom = initialize_post_dominators(&extended_nodes, virtual_exit);

    // Compute reverse postorder
    let order = reverse_postorder_reversed(&preds, virtual_exit, &extended_nodes)?;

    // Iterate to fixed point
    let max_iterations = 1000;
    let mut iteration = 0;
    loop {
        let changed = iterate_post_dominators(&preds, &mut post_dom, &order)?;

        if !changed {
            break;
        }

        iteration += 1;
        if iteration >= max_iterations {
            return Err(SqliteGraphError::query(format!(
                "Post-dominator computation failed to converge after {} iterations",
                max_iterations
            )));
        }
    }

    // Extract immediate post-dominators
    let mut ipdom = extract_immediate_post_dominators(&post_dom, virtual_exit);

    // Remove virtual exit from results
    post_dom.remove(&virtual_exit);
    ipdom.remove(&virtual_exit);

    // Remove virtual exit from post-dominance sets
    for (_, set) in post_dom.iter_mut() {
        set.remove(&virtual_exit);
    }

    // Any node that had virtual_exit as ipdom should now have None (they're real exits)
    for (_, ipdom_val) in ipdom.iter_mut() {
        if *ipdom_val == Some(virtual_exit) {
            *ipdom_val = None;
        }
    }

    Ok(PostDominatorResult { post_dom, ipdom })
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

    /// Helper: Create CFG with multiple exits
    fn create_multiple_exits_cfg() -> SqliteGraph {
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

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create multiple exits: 0 -> 1 -> 2, 0 -> 3 -> 4
        // 2 and 4 are exits (no outgoing edges)
        let edges = vec![(0, 1), (1, 2), (0, 3), (3, 4)];
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

    /// Helper: Create CFG with single exit
    fn create_single_exit_cfg() -> SqliteGraph {
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

        // Create linear chain: 0 -> 1 -> 2 -> 3 (3 is exit)
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

    #[test]
    fn test_post_dominators_linear_chain() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: 3 post-dominated by all, 2 post-dominates {2,3}, 1 post-dominates {1,2,3}, 0 post-dominates {0,1,2,3}
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        // Exit post-dominates all nodes
        for &node in &entity_ids {
            assert!(
                result.post_dominates(exit, node),
                "Exit should post-dominate node {}",
                node
            );
        }

        // In a linear chain 0->1->2->3 (IDs [1,2,3,4], exit=4):
        // - Node N post-dominates node M if N is on ALL paths from M to exit
        // - Exit (4) post-dominates all nodes
        // - Node 3 post-dominates {1,2,3} (on paths from 1,2,3 to exit)
        // - Node 2 post-dominates {1,2} (on paths from 1,2 to exit)
        // - Node 1 post-dominates {1} (on paths from 1 to exit)

        // Node 3 (entity_ids[2]) post-dominates {1, 2, 3}
        assert!(
            result.post_dominates(entity_ids[2], entity_ids[0]),
            "Node 3 should post-dominate node 1"
        );
        assert!(
            result.post_dominates(entity_ids[2], entity_ids[1]),
            "Node 3 should post-dominate node 2"
        );
        assert!(
            result.post_dominates(entity_ids[2], entity_ids[2]),
            "Node 3 should post-dominate itself"
        );
        assert!(
            !result.post_dominates(entity_ids[2], entity_ids[3]),
            "Node 3 should NOT post-dominate exit 4"
        );

        // Node 2 (entity_ids[1]) post-dominates {1, 2}
        assert!(
            result.post_dominates(entity_ids[1], entity_ids[0]),
            "Node 2 should post-dominate node 1"
        );
        assert!(
            result.post_dominates(entity_ids[1], entity_ids[1]),
            "Node 2 should post-dominate itself"
        );
        assert!(
            !result.post_dominates(entity_ids[1], entity_ids[2]),
            "Node 2 should NOT post-dominate node 3"
        );
        assert!(
            !result.post_dominates(entity_ids[1], entity_ids[3]),
            "Node 2 should NOT post-dominate exit 4"
        );

        // Node 1 (entity_ids[0]) post-dominates {1} (only itself)
        assert!(
            result.post_dominates(entity_ids[0], entity_ids[0]),
            "Node 1 should post-dominate itself"
        );
        assert!(
            !result.post_dominates(entity_ids[0], entity_ids[1]),
            "Node 1 should NOT post-dominate node 2"
        );
        assert!(
            !result.post_dominates(entity_ids[0], entity_ids[2]),
            "Node 1 should NOT post-dominate node 3"
        );
        assert!(
            !result.post_dominates(entity_ids[0], entity_ids[3]),
            "Node 1 should NOT post-dominate exit 4"
        );
    }

    #[test]
    fn test_post_dominators_diamond() {
        // Scenario: Diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Expected: Exit (3) post-dominates all, 0 post-dominates only itself
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        // Exit post-dominates all
        for &node in &entity_ids {
            assert!(
                result.post_dominates(exit, node),
                "Exit should post-dominate {}",
                node
            );
        }

        // Node 0 only post-dominates itself (paths from 0 can go through 1 or 2)
        assert!(result.post_dominates(entity_ids[0], entity_ids[0]));
        assert!(!result.post_dominates(entity_ids[0], entity_ids[1]));
        assert!(!result.post_dominates(entity_ids[0], entity_ids[2]));
        assert!(!result.post_dominates(entity_ids[0], entity_ids[3]));

        // Nodes 1 and 2 post-dominate only themselves (not entry, not each other)
        assert!(result.post_dominates(entity_ids[1], entity_ids[1]));
        assert!(!result.post_dominates(entity_ids[1], exit));
        assert!(!result.post_dominates(entity_ids[1], entity_ids[0]));
        assert!(!result.post_dominates(entity_ids[1], entity_ids[2]));

        assert!(result.post_dominates(entity_ids[2], entity_ids[2]));
        assert!(!result.post_dominates(entity_ids[2], exit));
        assert!(!result.post_dominates(entity_ids[2], entity_ids[0]));
        assert!(!result.post_dominates(entity_ids[2], entity_ids[1]));
    }

    #[test]
    fn test_post_dominators_loop() {
        // Scenario: Loop CFG: 0 -> 1 -> 2 -> 1
        // Expected: 2 is exit, check post-domination
        let graph = create_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[2];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        // Exit post-dominates all nodes (every path from any node reaches exit)
        for &node in &entity_ids {
            assert!(
                result.post_dominates(exit, node),
                "Exit should post-dominate {}",
                node
            );
        }
    }

    #[test]
    fn test_immediate_post_dominators_linear() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3
        // Expected: ipdom(3) = None, ipdom(2) = 3, ipdom(1) = 2, ipdom(0) = 1
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        eprintln!("Entity IDs: {:?}", entity_ids);
        eprintln!("Exit: {}", exit);
        eprintln!("Post-dominators: {:?}", result.post_dom);
        eprintln!("IPDOM: {:?}", result.ipdom);

        assert_eq!(
            result.immediate_post_dominator(exit),
            None,
            "Exit should have no immediate post-dominator"
        );
        assert_eq!(
            result.immediate_post_dominator(entity_ids[2]),
            Some(exit),
            "ipdom(2) should be 3"
        );
        assert_eq!(
            result.immediate_post_dominator(entity_ids[1]),
            Some(entity_ids[2]),
            "ipdom(1) should be 2"
        );
        assert_eq!(
            result.immediate_post_dominator(entity_ids[0]),
            Some(entity_ids[1]),
            "ipdom(0) should be 1"
        );
    }

    #[test]
    fn test_immediate_post_dominators_diamond() {
        // Scenario: Diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // In a diamond, all paths from 0 go through exit (3), so exit is ipdom of all nodes
        // Expected: ipdom(3) = None, ipdom(1) = 3, ipdom(2) = 3, ipdom(0) = 3
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        assert_eq!(
            result.immediate_post_dominator(exit),
            None,
            "Exit should have no ipdom"
        );

        // Both branches (1 and 2) have exit as their ipdom
        assert_eq!(
            result.immediate_post_dominator(entity_ids[1]),
            Some(exit),
            "ipdom(1) should be exit"
        );
        assert_eq!(
            result.immediate_post_dominator(entity_ids[2]),
            Some(exit),
            "ipdom(2) should be exit"
        );

        // Node 0 also has exit as its ipdom (all paths from 0 go to exit)
        assert_eq!(
            result.immediate_post_dominator(entity_ids[0]),
            Some(exit),
            "ipdom(0) should be exit (all paths from 0 go through exit)"
        );
    }

    #[test]
    fn test_immediate_post_dominators_loop() {
        // Scenario: Loop CFG: 0 -> 1 -> 2 -> 1
        // Expected: ipdom(2) = None, check ipdom relationships
        let graph = create_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[2];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        assert_eq!(result.immediate_post_dominator(exit), None);
        // Node 1's ipdom should be 2 (exit)
        assert_eq!(
            result.immediate_post_dominator(entity_ids[1]),
            Some(exit),
            "ipdom(1) should be exit"
        );
    }

    #[test]
    fn test_immediate_post_dominators_exit_is_none() {
        // Scenario: Exit node should have None as ipdom
        // Expected: ipdom(exit) = None
        let graph = create_single_exit_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        assert_eq!(result.immediate_post_dominator(exit), None);
    }

    #[test]
    fn test_post_dominators_auto_single_exit() {
        // Scenario: Auto-detects single exit
        // Expected: Uses detected exit
        let graph = create_single_exit_cfg();

        let result = post_dominators_auto_exit(&graph).expect("Failed to auto-detect exit");

        // Should have results for all nodes
        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
        assert!(!entity_ids.is_empty());

        // Exit should be detected and post-dominate all
        let exit = entity_ids[entity_ids.len() - 1];
        assert!(result.is_exit(exit));
    }

    #[test]
    fn test_post_dominators_auto_multiple_exits() {
        // Scenario: Auto-detects multiple exits
        // Expected: Creates virtual exit
        let graph = create_multiple_exits_cfg();

        let result = post_dominators_auto_exit(&graph).expect("Failed to auto-detect exits");

        // Should have results for all nodes
        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

        eprintln!("Entity IDs: {:?}", entity_ids);
        eprintln!("Post-dominators: {:?}", result.post_dom);
        eprintln!("IPDOM: {:?}", result.ipdom);

        assert_eq!(result.post_dom.len(), entity_ids.len());

        // Both exits (2 and 4) should have None as ipdom
        assert_eq!(
            result.immediate_post_dominator(entity_ids[2]),
            None,
            "Exit 2 should have no ipdom"
        );
        assert_eq!(
            result.immediate_post_dominator(entity_ids[4]),
            None,
            "Exit 4 should have no ipdom"
        );
    }

    #[test]
    fn test_post_dominators_virtual_exit_unification() {
        // Scenario: Virtual exit unifies multiple real exits
        // Expected: All real exits have None as ipdom after stripping virtual exit
        let graph = create_multiple_exits_cfg();

        let result = post_dominators_auto_exit(&graph).expect("Failed with virtual exit");

        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

        // Real exits should have None as ipdom
        assert_eq!(result.immediate_post_dominator(entity_ids[2]), None);
        assert_eq!(result.immediate_post_dominator(entity_ids[4]), None);

        // Virtual exit should not be in results
        assert!(!result.post_dom.contains_key(&-1));
        assert!(!result.ipdom.contains_key(&-1));
    }

    #[test]
    fn test_post_dominators_no_exits() {
        // Scenario: Graph with no exit nodes (all nodes have outgoing edges)
        // Expected: Returns error
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create cycle: 0 -> 1 -> 2 -> 0 (all nodes have outgoing edges)
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

        // Create cycle
        let edges = vec![(0, 1), (1, 2), (2, 0)];
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

        let result = post_dominators_auto_exit(&graph);
        assert!(result.is_err(), "Should fail on graph with no exits");
    }

    #[test]
    fn test_post_dominators_empty_graph() {
        // Scenario: Empty graph
        // Expected: Returns empty result
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = post_dominators_auto_exit(&graph).expect("Failed on empty graph");

        assert_eq!(result.post_dom.len(), 0);
        assert_eq!(result.ipdom.len(), 0);
    }

    #[test]
    fn test_post_dominators_nonexistent_exit() {
        // Scenario: Exit node doesn't exist
        // Expected: Returns NotFound error
        let graph = create_linear_chain();

        let result = post_dominators(&graph, 999);
        assert!(result.is_err(), "Should fail for nonexistent exit");

        if let Err(SqliteGraphError::NotFound(msg)) = result {
            assert!(msg.contains("999"), "Error should mention node 999");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[test]
    fn test_post_dominators_self_post_dominance() {
        // Scenario: Every node should post-dominate itself
        // Expected: For all nodes n, n ∈ post_dom[n]
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        for &node in &entity_ids {
            assert!(
                result.post_dominates(node, node),
                "Node {} should post-dominate itself",
                node
            );
        }
    }

    #[test]
    fn test_post_dominators_single_node() {
        // Scenario: Single node graph
        // Expected: Node post-dominates only itself
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
        let exit = entity_ids[0];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        // Exit post-dominates only itself
        assert!(result.post_dominates(exit, exit));
        assert_eq!(result.immediate_post_dominator(exit), None);
        assert_eq!(
            result.post_dom.len(),
            1,
            "Should have 1 node in post_dom sets"
        );
    }

    #[test]
    fn test_post_dominators_is_exit() {
        // Scenario: Check is_exit method
        // Expected: Only exit node returns true
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        assert!(result.is_exit(exit), "Exit should be identified as exit");
        assert!(
            !result.is_exit(entity_ids[0]),
            "Non-exit should not be exit"
        );
        assert!(
            !result.is_exit(entity_ids[1]),
            "Non-exit should not be exit"
        );
        assert!(
            !result.is_exit(entity_ids[2]),
            "Non-exit should not be exit"
        );
    }

    #[test]
    fn test_post_dominators_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results, progress callback works
        use crate::progress::NoProgress;

        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let progress = NoProgress;
        let result_with = post_dominators_with_progress(&graph, exit, &progress).expect("Failed");
        let result_without = post_dominators(&graph, exit).expect("Failed");

        // Check post-dominance sets match
        assert_eq!(
            result_with.post_dom.len(),
            result_without.post_dom.len(),
            "Should have same number of nodes"
        );

        for (&node, post_dom_set) in &result_without.post_dom {
            assert!(
                result_with.post_dom.contains_key(&node),
                "Progress result missing node {}",
                node
            );
            assert_eq!(
                result_with.post_dom.get(&node),
                Some(post_dom_set),
                "Post-dominance sets differ for node {}",
                node
            );
        }

        // Check immediate post-dominators match
        assert_eq!(result_with.ipdom, result_without.ipdom);
    }

    #[test]
    fn test_post_dominators_symmetry_property() {
        // Scenario: Verify post-dominance symmetry properties
        // Expected: Post-dominance is reflexive, transitive, antisymmetric
        let graph = create_linear_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let exit = entity_ids[3];

        let result = post_dominators(&graph, exit).expect("Failed to compute post-dominators");

        // Reflexive: every node post-dominates itself
        for &node in &entity_ids {
            assert!(
                result.post_dominates(node, node),
                "Reflexivity failed for {}",
                node
            );
        }

        // Transitive: if a post-dominates b and b post-dominates c, then a post-dominates c
        // In linear chain: 3 post-dominates 2, 2 post-dominates 1, so 3 should post-dominate 1
        assert!(result.post_dominates(exit, entity_ids[2]));
        assert!(result.post_dominates(entity_ids[2], entity_ids[1]));
        assert!(
            result.post_dominates(exit, entity_ids[1]),
            "Transitivity failed"
        );

        // Antisymmetric: if a post-dominates b and b post-dominates a, then a = b
        // In linear chain with more than 1 node, no two distinct nodes mutually post-dominate
        for i in 0..entity_ids.len() {
            for j in (i + 1)..entity_ids.len() {
                let a = entity_ids[i];
                let b = entity_ids[j];
                let a_post_dom_b = result.post_dominates(a, b);
                let b_post_dom_a = result.post_dominates(b, a);
                assert!(
                    !(a_post_dom_b && b_post_dom_a),
                    "Antisymmetry failed: {} and {} mutually post-dominate",
                    a,
                    b
                );
            }
        }
    }
}
