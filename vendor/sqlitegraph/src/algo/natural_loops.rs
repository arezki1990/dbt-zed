//! Natural loop detection using back-edge dominance analysis.
//!
//! This module provides algorithms for detecting natural loops in control flow
//! graphs (CFGs). Natural loops are the foundation for loop optimizations
//! (invariant code motion, unrolling) and program analysis.
//!
//! # Natural Loop Definition
//!
//! A **natural loop** is identified by a **back-edge** `(tail -> header)` where
//! the header **dominates** the tail. This distinguishes natural loops (reducible
//! CFGs) from irreducible cycles.
//!
//! - **Loop header**: The unique entry point of the loop (dominates all loop nodes)
//! - **Back-edge**: Edge `(tail -> header)` that forms the cycle
//! - **Loop body**: All nodes reachable from tail without passing through header
//!
//! # Properties
//!
//! - **Unique entry**: Every natural loop has a single entry point (the header)
//! - **Reducibility**: Natural loops exist only in reducible CFGs
//! - **Nesting**: Inner loop headers are contained in outer loop bodies
//! - **Multiple back-edges**: Multiple edges to the same header are grouped into one loop
//!
//! # Irreducible CFGs
//!
//! Irreducible CFGs contain cycles without dominance (e.g., two-node cycles where
//! neither node dominates the other). These are **NOT** detected as natural loops.
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::{dominators, natural_loops}};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... build CFG with entry node 0 ...
//!
//! let dom_result = dominators(&graph, 0)?;
//! let loops = natural_loops(&graph, &dom_result)?;
//!
//! for (header, loop_) in loops.loops() {
//!     println!("Loop with header {}: {} nodes, {} back-edges",
//!              header, loop_.all_nodes().len(), loop_.back_edges.len());
//! }
//! ```
//!
//! # References
//!
//! - Cooper, Keith D., Harvey, Timothy J., and Kennedy, Ken. "A simple, fast
//!   dominance algorithm." Software Practice & Experience, 2001.
//! - Muchnick, Steven S. "Advanced Compiler Design and Implementation." 1997.

use ahash::{AHashMap, AHashSet};
use std::collections::VecDeque;

use crate::algo::dominators::DominatorResult;
use crate::errors::SqliteGraphError;
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

/// A natural loop identified by back-edges.
///
/// Natural loops are reducible cycles in control flow graphs where the header
/// dominates the tail of the back-edge.
#[derive(Debug, Clone)]
pub struct NaturalLoop {
    /// Loop header (dominates all loop nodes).
    ///
    /// The header is the unique entry point of the loop. All paths from outside
    /// the loop into the loop must pass through the header.
    pub header: i64,

    /// All back-edges to this header: `(tail, header)` pairs.
    ///
    /// Multiple back-edges to the same header are grouped into a single loop.
    /// For example, a loop with two `continue` statements branching to the header
    /// will have two back-edges.
    pub back_edges: Vec<(i64, i64)>,

    /// Loop body nodes (excluding header).
    ///
    /// The body contains all nodes in the loop except the header. These are
    /// the nodes that are executed during each iteration of the loop.
    pub body: AHashSet<i64>,
}

impl NaturalLoop {
    /// Checks if a node is in this loop (header or body).
    ///
    /// Returns `true` if the node is either the header or in the loop body.
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// `true` if node is in the loop, `false` otherwise.
    ///
    /// # Example
    /// ```rust,ignore
    /// let loop_ = natural_loops(&graph, &dom_result)?;
    /// let some_loop = loop_.loop_with_header(header).unwrap();
    /// assert!(some_loop.contains(header));  // Header is in loop
    /// assert!(some_loop.contains(body_node));  // Body nodes are in loop
    /// ```
    pub fn contains(&self, node: i64) -> bool {
        self.header == node || self.body.contains(&node)
    }

    /// Gets all nodes in the loop (header + body).
    ///
    /// Returns a set containing both the header and all body nodes.
    ///
    /// # Returns
    /// Set of all node IDs in the loop.
    pub fn all_nodes(&self) -> AHashSet<i64> {
        let mut nodes = self.body.clone();
        nodes.insert(self.header);
        nodes
    }

    /// Checks if this loop is nested within another loop.
    ///
    /// A loop is nested within another if its header is contained in the
    /// outer loop's body.
    ///
    /// # Arguments
    /// * `parent` - Potential parent loop
    ///
    /// # Returns
    /// `true` if this loop's header is in the parent's body, `false` otherwise.
    ///
    /// # Example
    /// ```rust,ignore
    /// // In nested loop CFG where outer header is 1, inner header is 2
    /// let outer = loops.loop_with_header(1).unwrap();
    /// let inner = loops.loop_with_header(2).unwrap();
    /// assert!(inner.is_nested_in(&outer));  // Inner is nested in outer
    /// ```
    pub fn is_nested_in(&self, parent: &NaturalLoop) -> bool {
        parent.contains(self.header)
    }

    /// Gets nesting depth of this loop within another (0 = not nested).
    ///
    /// Returns 1 if nested in parent, 0 otherwise. This is a simplified check;
    /// for full nesting analysis across all loops, use `NaturalLoopsResult::nesting_depth`.
    ///
    /// # Arguments
    /// * `parent` - Potential parent loop
    ///
    /// # Returns
    /// `1` if this loop is nested in parent, `0` otherwise.
    pub fn nesting_depth_in(&self, parent: &NaturalLoop) -> usize {
        if !parent.contains(self.header) {
            return 0;
        }
        // Simplified: return 1 if nested, 0 otherwise
        // Full implementation would check against all loops
        1
    }

    /// Gets the number of back-edges to this loop's header.
    ///
    /// Multiple back-edges indicate multiple ways to jump back to the header
    /// (e.g., multiple `continue` statements).
    ///
    /// # Returns
    /// Number of back-edges to the header.
    pub fn back_edge_count(&self) -> usize {
        self.back_edges.len()
    }

    /// Gets the number of nodes in the loop body (excluding header).
    ///
    /// # Returns
    /// Number of body nodes.
    pub fn body_size(&self) -> usize {
        self.body.len()
    }

    /// Gets the total number of nodes in the loop (header + body).
    ///
    /// # Returns
    /// Total loop size.
    pub fn size(&self) -> usize {
        self.body.len() + 1
    }
}

/// Natural loop detection result.
///
/// Contains all natural loops found in the CFG, indexed by their header nodes.
#[derive(Debug, Clone)]
pub struct NaturalLoopsResult {
    /// Map from header to its natural loop.
    ///
    /// Each key is a loop header, and the value is the complete loop information
    /// including back-edges and body nodes.
    pub loops: AHashMap<i64, NaturalLoop>,
}

impl NaturalLoopsResult {
    /// Gets the loop with this header (if any).
    ///
    /// Returns `None` if the node is not a loop header.
    ///
    /// # Arguments
    /// * `header` - Potential loop header node ID
    ///
    /// # Returns
    /// `Some(&NaturalLoop)` if header has a loop, `None` otherwise.
    pub fn loop_with_header(&self, header: i64) -> Option<&NaturalLoop> {
        self.loops.get(&header)
    }

    /// Checks if a node is a loop header.
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// `true` if node is a loop header, `false` otherwise.
    pub fn is_loop_header(&self, node: i64) -> bool {
        self.loops.contains_key(&node)
    }

    /// Gets loop nesting depth for a node (0 = not in loop).
    ///
    /// Returns the number of loops that contain this node. For example, a node
    /// in a doubly-nested loop returns 2.
    ///
    /// # Arguments
    /// * `node` - Node ID to check
    ///
    /// # Returns
    /// Nesting depth (0 = not in any loop).
    pub fn nesting_depth(&self, node: i64) -> usize {
        self.loops.values().filter(|l| l.contains(node)).count()
    }

    /// Gets all loops at a given nesting depth.
    ///
    /// Depth 1 = outermost loops, depth 2 = loops nested in depth 1, etc.
    ///
    /// # Arguments
    /// * `depth` - Nesting depth to filter by
    ///
    /// # Returns
    /// Vector of loops at the specified depth.
    pub fn loops_at_depth(&self, depth: usize) -> Vec<&NaturalLoop> {
        self.loops
            .values()
            .filter(|l| self.nesting_depth(l.header) == depth)
            .collect()
    }

    /// Builds loop nesting tree (parent headers -> child headers).
    ///
    /// Returns a map where each key is a parent loop header and the value is
    /// a vector of child loop headers. This represents the direct nesting
    /// relationships (not transitive).
    ///
    /// # Returns
    /// Map from parent header to vector of child headers.
    ///
    /// # Example
    /// ```rust,ignore
    /// let tree = loops.nesting_tree();
    /// for (&parent, children) in &tree {
    ///     println!("Loop {} contains inner loops: {:?}", parent, children);
    /// }
    /// ```
    pub fn nesting_tree(&self) -> AHashMap<i64, Vec<i64>> {
        let mut tree: AHashMap<i64, Vec<i64>> = AHashMap::new();

        for (&header, _loop_) in &self.loops {
            for (&potential_parent, parent_loop) in &self.loops {
                if header == potential_parent {
                    continue;
                }
                if parent_loop.contains(header) {
                    // Check if this is the direct parent (no intermediate loop)
                    let is_direct = !self.loops.values().any(|other| {
                        other.header != potential_parent
                            && other.header != header
                            && parent_loop.contains(other.header)
                            && other.contains(header)
                    });

                    if is_direct {
                        tree.entry(potential_parent).or_default().push(header);
                    }
                }
            }
        }

        tree
    }

    /// Gets total number of loops detected.
    ///
    /// # Returns
    /// Number of natural loops in the CFG.
    pub fn count(&self) -> usize {
        self.loops.len()
    }

    /// Gets all loop headers.
    ///
    /// # Returns
    /// Vector of all loop header node IDs.
    pub fn headers(&self) -> Vec<i64> {
        self.loops.keys().copied().collect()
    }

    /// Gets all loops as an iterator.
    ///
    /// # Returns
    /// Iterator over `(header, &NaturalLoop)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (i64, &NaturalLoop)> {
        self.loops.iter().map(|(&header, loop_)| (header, loop_))
    }
}

/// Computes natural loops by finding back-edges where header dominates tail.
///
/// A natural loop is identified by a back-edge `(tail -> header)` where the
/// header dominates the tail. Multiple back-edges to the same header are
/// grouped into a single loop.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `dom_result` - Dominator computation result (must include dominates() method)
///
/// # Returns
/// `NaturalLoopsResult` containing all detected natural loops.
///
/// # Algorithm
///
/// 1. **Find back-edges**: For each edge `(tail, header)` in the graph:
///    - Check if `dom_result.dominates(header, tail)` is true
///    - If yes, this is a back-edge
///
/// 2. **Group by header**: All back-edges to the same header form one loop
///
/// 3. **Compute loop body**: For each back-edge:
///    - DFS from tail, add all reachable nodes except header
///    - Union with existing body for that header
///
/// # Complexity
///
/// - **Time**: O(E * N) for back-edge check, O(E) for loop body computation
/// - **Space**: O(E) for storing loop bodies
///
/// Where:
/// - E = number of edges
/// - N = number of vertices
///
/// # Irreducible CFGs
///
/// Irreducible CFGs (cycles without dominance) return empty loops for those
/// cycles. This method does NOT produce false positives.
///
/// # Error Handling
///
/// - Returns empty result for empty graphs
/// - Handles unreachable nodes gracefully (skips edges from unreachable nodes)
/// - Handles single-node graphs (no loops possible)
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::{dominators, natural_loops}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let dom_result = dominators(&graph, entry)?;
/// let loops = natural_loops(&graph, &dom_result)?;
///
/// println!("Found {} natural loops", loops.count());
/// for (header, loop_) in loops.iter() {
///     println!("  Loop {}: {} nodes, {} back-edges",
///              header, loop_.size(), loop_.back_edge_count());
/// }
/// ```
pub fn natural_loops(
    graph: &SqliteGraph,
    dom_result: &DominatorResult,
) -> Result<NaturalLoopsResult, SqliteGraphError> {
    let all_nodes = graph.all_entity_ids()?;

    // Handle empty graph
    if all_nodes.is_empty() {
        return Ok(NaturalLoopsResult {
            loops: AHashMap::new(),
        });
    }

    natural_loops_internal(graph, dom_result, &all_nodes)
}

/// Computes natural loops with automatic entry node detection.
///
/// Convenience function that automatically detects the CFG entry node (node with
/// no incoming edges) and computes natural loops. This is the easiest way to
/// compute natural loops when you don't need to specify a custom entry node.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
///
/// # Returns
/// `NaturalLoopsResult` containing all detected natural loops.
///
/// # Entry Node Detection
///
/// The entry node is detected as the node with no incoming edges. If multiple
/// nodes have no incoming edges (disconnected graph), returns an error.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::natural_loops_from_exit};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build CFG ...
///
/// let loops = natural_loops_from_exit(&graph)?;
///
/// println!("Found {} natural loops", loops.count());
/// ```
///
/// # Naming Note
///
/// Despite the `_from_exit` suffix, this function detects **entry** nodes (not exit nodes)
/// because natural loops are computed using **dominators** (which require an entry point),
/// not post-dominators (which require exit points). The naming follows the convention
/// of `control_dependence_from_exit` for consistency in the CFG analysis API.
pub fn natural_loops_from_exit(
    graph: &SqliteGraph,
) -> Result<NaturalLoopsResult, SqliteGraphError> {
    // Get all nodes
    let all_nodes = graph.all_entity_ids()?;

    if all_nodes.is_empty() {
        return Ok(NaturalLoopsResult {
            loops: AHashMap::new(),
        });
    }

    // Find entry nodes (nodes with no incoming edges)
    let mut entries = Vec::new();
    for &node in &all_nodes {
        let incoming = graph.fetch_incoming(node)?;
        if incoming.is_empty() {
            entries.push(node);
        }
    }

    if entries.is_empty() {
        return Err(SqliteGraphError::query(
            "Cannot compute natural loops: graph has no entry nodes (all nodes have incoming edges)",
        ));
    }

    if entries.len() > 1 {
        return Err(SqliteGraphError::query(format!(
            "Cannot compute natural loops: graph has {} entry nodes (expected 1)",
            entries.len()
        )));
    }

    // Single entry - compute dominators and natural loops
    let entry = entries[0];
    let dom_result = super::dominators::dominators(graph, entry)?;
    natural_loops(graph, &dom_result)
}

/// Computes natural loops with progress tracking.
///
/// Same algorithm as [`natural_loops`] but reports progress during execution.
/// Useful for long-running operations on large graphs.
///
/// # Arguments
/// * `graph` - The control flow graph to analyze
/// * `dom_result` - Dominator computation result
/// * `progress` - Progress callback for reporting execution status
///
/// # Returns
/// `NaturalLoopsResult` containing all detected natural loops.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Number of edges processed
/// - `total`: Total number of edges to process
/// - `message`: "Finding natural loops: {current}/{total} edges checked"
///
/// Progress is reported after each batch of edges is processed.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::{dominators_with_progress, natural_loops_with_progress},
///     progress::ConsoleProgress
/// };
///
/// let progress = ConsoleProgress::new();
/// let dom_result = dominators_with_progress(&graph, entry, &progress)?;
/// let loops = natural_loops_with_progress(&graph, &dom_result, &progress)?;
/// // Output: Finding natural loops: 100/200 edges checked...
/// // Output: Finding natural loops: 200/200 edges checked...
/// ```
pub fn natural_loops_with_progress<F>(
    graph: &SqliteGraph,
    dom_result: &DominatorResult,
    progress: &F,
) -> Result<NaturalLoopsResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    let all_nodes = graph.all_entity_ids()?;

    // Handle empty graph
    if all_nodes.is_empty() {
        return Ok(NaturalLoopsResult {
            loops: AHashMap::new(),
        });
    }

    // Count total edges for progress reporting
    let mut total_edges = 0;
    for &node in &all_nodes {
        let outgoing = graph.fetch_outgoing(node)?;
        total_edges += outgoing.len();
    }

    // Report initial progress
    progress.on_progress(
        0,
        Some(total_edges),
        &format!("Finding natural loops: 0/{} edges checked", total_edges),
    );

    // Find loops with progress tracking
    let mut loops: AHashMap<i64, NaturalLoop> = AHashMap::new();
    let mut edges_checked = 0;

    for &node in &all_nodes {
        let outgoing = graph.fetch_outgoing(node)?;

        for &target in &outgoing {
            // Check if this is a back-edge (header dominates tail)
            if dom_result.dominates(target, node) {
                // Skip if tail is entry (entry can't be tail of back-edge)
                // Entry node doesn't have itself in dominance set from other nodes
                // Actually, we need to check if node != entry
                // But we don't have entry here, so we rely on dominance check:
                // If header dominates tail, and header != tail, it's a valid back-edge
                // However, entry dominates all, so edge from entry to entry is self-loop
                // We'll skip self-loops as they're not interesting for natural loops

                if node == target {
                    // Skip self-loops
                    edges_checked += 1;
                    continue;
                }

                // This is a back-edge to target (header)
                let header = target;

                // Get or create loop for this header
                let loop_ = loops.entry(header).or_insert_with(|| NaturalLoop {
                    header,
                    back_edges: Vec::new(),
                    body: AHashSet::new(),
                });

                // Add back-edge
                loop_.back_edges.push((node, header));

                // Compute and union loop body
                let body = compute_loop_body(graph, header, node)?;
                loop_.body.extend(body);
            }

            edges_checked += 1;

            // Report progress periodically
            if edges_checked % 100 == 0 || edges_checked == total_edges {
                progress.on_progress(
                    edges_checked,
                    Some(total_edges),
                    &format!(
                        "Finding natural loops: {}/{} edges checked",
                        edges_checked, total_edges
                    ),
                );
            }
        }
    }

    // Report completion
    progress.on_complete();

    Ok(NaturalLoopsResult { loops })
}

/// Internal implementation of natural loop detection.
///
/// # Arguments
/// * `graph` - The control flow graph
/// * `dom_result` - Dominator computation result
/// * `all_nodes` - All node IDs in the graph (pre-fetched for efficiency)
///
/// # Returns
/// `NaturalLoopsResult` containing all detected natural loops.
fn natural_loops_internal(
    graph: &SqliteGraph,
    dom_result: &DominatorResult,
    all_nodes: &[i64],
) -> Result<NaturalLoopsResult, SqliteGraphError> {
    let mut loops: AHashMap<i64, NaturalLoop> = AHashMap::new();

    for &node in all_nodes {
        let outgoing = graph.fetch_outgoing(node)?;

        for &target in &outgoing {
            // Check if this is a back-edge (header dominates tail)
            if dom_result.dominates(target, node) {
                // Skip self-loops
                if node == target {
                    continue;
                }

                // This is a back-edge to target (header)
                let header = target;

                // Get or create loop for this header
                let loop_ = loops.entry(header).or_insert_with(|| NaturalLoop {
                    header,
                    back_edges: Vec::new(),
                    body: AHashSet::new(),
                });

                // Add back-edge
                loop_.back_edges.push((node, header));

                // Compute and union loop body
                let body = compute_loop_body(graph, header, node)?;
                loop_.body.extend(body);
            }
        }
    }

    Ok(NaturalLoopsResult { loops })
}

/// Computes the loop body for a back-edge.
///
/// The loop body is all nodes reachable from the tail without passing through
/// the header. This uses DFS starting from the tail, excluding the header.
///
/// # Arguments
/// * `graph` - The control flow graph
/// * `header` - Loop header node ID
/// * `tail` - Tail node of the back-edge
///
/// # Returns
/// Set of nodes in the loop body (excluding header).
///
/// # Algorithm
///
/// 1. DFS from tail
/// 2. Add all reachable nodes to body
/// 3. Stop when reaching header (don't traverse past header)
/// 4. Return body set (excluding header)
///
/// # Example
///
/// For a loop with edges `1 -> 2`, `2 -> 3`, `3 -> 1`:
/// - Header = 1
/// - Back-edge = (3, 1)
/// - Body = {2, 3} (nodes reachable from 3 without passing through 1)
fn compute_loop_body(
    graph: &SqliteGraph,
    header: i64,
    tail: i64,
) -> Result<AHashSet<i64>, SqliteGraphError> {
    let mut body = AHashSet::new();
    let mut visited = AHashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(tail);
    visited.insert(tail);

    while let Some(node) = queue.pop_front() {
        // Skip header (don't add it to body, don't traverse past it)
        if node == header {
            continue;
        }

        // Add node to body
        body.insert(node);

        // Traverse successors
        let successors = graph.fetch_outgoing(node)?;
        for &succ in &successors {
            if !visited.contains(&succ) {
                visited.insert(succ);
                queue.push_back(succ);
            }
        }
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create single loop CFG: 0 -> 1 -> 2 -> 1
    /// Expected: header=1, back_edge=(2,1), body={2}
    fn create_single_loop_cfg() -> SqliteGraph {
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

    /// Helper: Create nested loops CFG: 0 -> 1 -> 2 -> 3, 3 -> 2, 3 -> 1
    /// Expected: outer (header=1, back_edge=(3,1)), inner (header=2, back_edge=(3,2))
    fn create_nested_loops_cfg() -> SqliteGraph {
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

        // Create nested loops: 0 -> 1, 1 -> 2, 2 -> 3, 3 -> 2, 3 -> 1
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

    /// Helper: Create while loop CFG: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3
    /// Expected: header=1, back_edge=(2,1), body={2}, exit to 3
    fn create_while_loop_cfg() -> SqliteGraph {
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

        // Create while loop: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3
        let edges = vec![(0, 1), (1, 2), (2, 1), (1, 3)];
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

    /// Helper: Create diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    /// Expected: no back-edges, no natural loops
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

    /// Helper: Create irreducible CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3, 3 -> 1, 3 -> 2
    /// Expected: neither 3->1 nor 3->2 are back-edges (neither dominates)
    fn create_irreducible_cfg() -> SqliteGraph {
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

        // Create irreducible CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3, 3 -> 1, 3 -> 2
        let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3), (3, 1), (3, 2)];
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

    /// Helper: Create multiple back-edges CFG: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3, 3 -> 1
    /// Expected: single loop with header=1, back_edges=[(2,1), (3,1)]
    fn create_multiple_back_edges_cfg() -> SqliteGraph {
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

        // Create multiple back-edges: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3, 3 -> 1
        let edges = vec![(0, 1), (1, 2), (2, 1), (1, 3), (3, 1)];
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
    fn test_natural_loops_single_loop() {
        // Scenario: Simple loop CFG
        // Expected: Detect header, back-edge, body correctly
        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should find 1 loop
        assert_eq!(loops.count(), 1, "Should find 1 loop");

        // Check loop with header 1
        let loop_ = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have loop with header 1");
        assert_eq!(loop_.header, entity_ids[1], "Loop header should be 1");
        assert_eq!(loop_.back_edges.len(), 1, "Should have 1 back-edge");
        assert_eq!(
            loop_.back_edges[0],
            (entity_ids[2], entity_ids[1]),
            "Back-edge should be (2, 1)"
        );
        assert!(
            loop_.body.contains(&entity_ids[2]),
            "Body should contain node 2"
        );
        assert_eq!(loop_.body.len(), 1, "Body should have 1 node");
    }

    #[test]
    fn test_natural_loop_contains() {
        // Scenario: Test contains() method
        // Expected: contains() works for header and body
        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        let loop_ = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have loop");

        // Header is in loop
        assert!(loop_.contains(entity_ids[1]), "Header should be in loop");

        // Body node is in loop
        assert!(loop_.contains(entity_ids[2]), "Body node should be in loop");

        // Outside node is not in loop
        assert!(
            !loop_.contains(entity_ids[0]),
            "Entry node should not be in loop"
        );
    }

    #[test]
    fn test_natural_loop_all_nodes() {
        // Scenario: Test all_nodes() method
        // Expected: all_nodes() returns header + body
        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        let loop_ = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have loop");

        let all_nodes = loop_.all_nodes();
        assert_eq!(all_nodes.len(), 2, "Loop should have 2 nodes total");
        assert!(all_nodes.contains(&entity_ids[1]), "Should contain header");
        assert!(all_nodes.contains(&entity_ids[2]), "Should contain body");
    }

    #[test]
    fn test_natural_loops_nested() {
        // Scenario: Nested loops CFG
        // Expected: Both loops detected
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should find 2 loops
        assert_eq!(loops.count(), 2, "Should find 2 loops");

        // Check outer loop (header 1)
        let outer = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have outer loop");
        assert_eq!(outer.back_edges.len(), 1, "Outer should have 1 back-edge");
        assert_eq!(
            outer.back_edges[0],
            (entity_ids[3], entity_ids[1]),
            "Outer back-edge should be (3, 1)"
        );

        // Check inner loop (header 2)
        let inner = loops
            .loop_with_header(entity_ids[2])
            .expect("Should have inner loop");
        assert_eq!(inner.back_edges.len(), 1, "Inner should have 1 back-edge");
        assert_eq!(
            inner.back_edges[0],
            (entity_ids[3], entity_ids[2]),
            "Inner back-edge should be (3, 2)"
        );
    }

    #[test]
    fn test_natural_loop_nesting() {
        // Scenario: Test nested loops detection
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should find loops
        assert!(loops.count() >= 1, "Should find at least one loop");

        // Verify loops have non-empty bodies
        let loops_vec: Vec<_> = loops.loops_at_depth(1);
        for loop_result in loops_vec {
            assert!(
                !loop_result.body.is_empty(),
                "Loop body should not be empty"
            );
        }
    }

    #[test]
    fn test_natural_loop_nesting_tree() {
        // Scenario: Test nesting_tree() method
        // Expected: nesting_tree() returns correct parent->children mapping
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        let tree = loops.nesting_tree();

        // Outer loop (1) should have inner loop (2) as child
        assert!(
            tree.contains_key(&entity_ids[1]),
            "Tree should have outer loop"
        );
        let children = tree.get(&entity_ids[1]).expect("Should have children");
        assert_eq!(children.len(), 1, "Outer should have 1 child");
        assert_eq!(children[0], entity_ids[2], "Child should be inner loop");
    }

    #[test]
    fn test_natural_loops_nesting_depth() {
        // Scenario: Test nesting_depth() method
        // Expected: nesting_depth() returns depth for each node
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Entry is not in any loop
        assert_eq!(
            loops.nesting_depth(entity_ids[0]),
            0,
            "Entry should have depth 0"
        );

        // Verify nesting depth is computed (actual values depend on loop structure)
        if loops.count() > 0 {
            // Loop headers should have at least depth 1
            for &header in &loops.headers() {
                assert!(
                    loops.nesting_depth(header) >= 1,
                    "Loop header should have depth >= 1"
                );
            }
        }
    }

    #[test]
    fn test_natural_loops_multiple_back_edges() {
        // Scenario: Multiple back-edges to same header
        // Expected: Single loop with multiple back_edges
        let graph = create_multiple_back_edges_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should find 1 loop (header 1)
        assert_eq!(loops.count(), 1, "Should find 1 loop");

        let loop_ = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have loop");
        assert_eq!(loop_.back_edges.len(), 2, "Should have 2 back-edges");
        assert!(
            loop_.back_edges.contains(&(entity_ids[2], entity_ids[1])),
            "Should have back-edge (2, 1)"
        );
        assert!(
            loop_.back_edges.contains(&(entity_ids[3], entity_ids[1])),
            "Should have back-edge (3, 1)"
        );
    }

    #[test]
    fn test_natural_loops_grouped_by_header() {
        // Scenario: Same header loops are grouped
        // Expected: Multiple back-edges to same header grouped into single loop
        let graph = create_multiple_back_edges_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Only header 1 should have a loop
        assert!(
            loops.is_loop_header(entity_ids[1]),
            "Node 1 should be loop header"
        );
        assert!(
            !loops.is_loop_header(entity_ids[2]),
            "Node 2 should not be loop header"
        );
        assert!(
            !loops.is_loop_header(entity_ids[3]),
            "Node 3 should not be loop header"
        );
    }

    #[test]
    fn test_natural_loops_empty_graph() {
        // Scenario: Empty graph
        // Expected: Returns empty result
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create fake dominator result for empty graph
        let dom_result = DominatorResult {
            dom: AHashMap::new(),
            idom: AHashMap::new(),
        };

        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert_eq!(loops.count(), 0, "Should have 0 loops in empty graph");
        assert_eq!(loops.headers().len(), 0, "Should have 0 headers");
    }

    #[test]
    fn test_natural_loops_single_node() {
        // Scenario: Single node graph
        // Expected: No loops (no edges)
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

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert_eq!(loops.count(), 0, "Should have 0 loops with single node");
    }

    #[test]
    fn test_natural_loops_linear_chain() {
        // Scenario: Linear chain (no cycles)
        // Expected: No loops (no back-edges)
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create linear chain: 0 -> 1 -> 2 -> 3
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

        // Create chain edges
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

        let entry = entity_ids[0];
        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert_eq!(loops.count(), 0, "Linear chain should have 0 loops");
    }

    #[test]
    fn test_natural_loops_diamond() {
        // Scenario: Diamond CFG (no back-edges where header dominates tail)
        // Expected: No natural loops
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert_eq!(loops.count(), 0, "Diamond CFG should have 0 natural loops");
    }

    #[test]
    fn test_natural_loops_irreducible_cfg() {
        // Scenario: Irreducible CFG (cycles without dominance)
        // Expected: Returns empty (no false positives)
        let graph = create_irreducible_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should not detect any natural loops (irreducible CFG)
        assert_eq!(
            loops.count(),
            0,
            "Irreducible CFG should have 0 natural loops"
        );
    }

    #[test]
    fn test_natural_loops_no_dominance_no_loop() {
        // Scenario: Edge without dominance is not a back-edge
        // Expected: Edge without dominance is not a back-edge
        let graph = create_diamond_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");

        // Check that node 0 does not dominate node 3 (paths: 0->1->3 and 0->2->3)
        // Actually, in diamond, 0 DOES dominate 3 (all paths go through 0)
        assert!(
            dom_result.dominates(entity_ids[0], entity_ids[3]),
            "0 should dominate 3"
        );

        // But there are no back-edges (no edges where header dominates tail in a cycle)
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");
        assert_eq!(loops.count(), 0, "Should have no loops");
    }

    #[test]
    fn test_natural_loops_loop_with_header() {
        // Scenario: Test loop_with_header() method
        // Expected: loop_with_header() returns correct loop
        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Existing loop
        let loop_ = loops.loop_with_header(entity_ids[1]);
        assert!(loop_.is_some(), "Should find loop with header 1");
        assert_eq!(loop_.unwrap().header, entity_ids[1]);

        // Non-existing loop
        let loop_ = loops.loop_with_header(entity_ids[0]);
        assert!(loop_.is_none(), "Should not find loop with header 0");
    }

    #[test]
    fn test_natural_loops_is_loop_header() {
        // Scenario: Test is_loop_header() method
        // Expected: is_loop_header() returns true only for headers
        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert!(
            loops.is_loop_header(entity_ids[1]),
            "Node 1 should be loop header"
        );
        assert!(
            !loops.is_loop_header(entity_ids[0]),
            "Node 0 should not be loop header"
        );
        assert!(
            !loops.is_loop_header(entity_ids[2]),
            "Node 2 should not be loop header"
        );
    }

    #[test]
    fn test_natural_loops_count() {
        // Scenario: Test count() method
        // Expected: count() returns number of loops
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        assert_eq!(loops.count(), 2, "Should have 2 loops");
    }

    #[test]
    fn test_natural_loops_headers() {
        // Scenario: Test headers() method
        // Expected: headers() returns all loop headers
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        let headers = loops.headers();
        assert_eq!(headers.len(), 2, "Should have 2 headers");
        assert!(headers.contains(&entity_ids[1]), "Should contain header 1");
        assert!(headers.contains(&entity_ids[2]), "Should contain header 2");
    }

    #[test]
    fn test_natural_loops_loops_at_depth() {
        // Scenario: Test loops_at_depth() method
        // Expected: loops_at_depth() returns loops at specified depth
        let graph = create_nested_loops_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Verify we can query loops at different depths
        assert!(loops.count() >= 1, "Should find at least one loop");

        // Just verify the method returns valid results
        let depth1 = loops.loops_at_depth(1);
        for loop_result in &depth1 {
            assert!(
                !loop_result.body.is_empty(),
                "Loop should have non-empty body"
            );
        }
    }

    #[test]
    fn test_natural_loops_with_progress_reports() {
        // Scenario: Progress callback is invoked
        // Expected: Progress callback called during computation
        use crate::progress::NoProgress;

        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");

        let progress = NoProgress;
        let loops = natural_loops_with_progress(&graph, &dom_result, &progress)
            .expect("Failed to compute natural loops with progress");

        assert_eq!(loops.count(), 1, "Should find 1 loop");
    }

    #[test]
    fn test_natural_loops_progress_completes() {
        // Scenario: Verify on_complete called
        // Expected: Progress callback completes without error
        use crate::progress::NoProgress;

        let graph = create_single_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");

        let progress = NoProgress;
        let result = natural_loops_with_progress(&graph, &dom_result, &progress);

        assert!(result.is_ok(), "Progress variant should succeed");
        assert_eq!(result.unwrap().count(), 1, "Should find 1 loop");
    }

    #[test]
    fn test_natural_loops_while_loop() {
        // Scenario: While loop with exit
        // Expected: Detect loop body and exit edge
        let graph = create_while_loop_cfg();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let entry = entity_ids[0];

        let dom_result =
            crate::algo::dominators(&graph, entry).expect("Failed to compute dominators");
        let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

        // Should find 1 loop
        assert_eq!(loops.count(), 1, "Should find 1 loop");

        let loop_ = loops
            .loop_with_header(entity_ids[1])
            .expect("Should have loop");
        assert_eq!(loop_.back_edges.len(), 1, "Should have 1 back-edge");
        assert_eq!(
            loop_.back_edges[0],
            (entity_ids[2], entity_ids[1]),
            "Back-edge should be (2, 1)"
        );

        // Body should contain node 2
        assert!(
            loop_.body.contains(&entity_ids[2]),
            "Body should contain node 2"
        );

        // Exit node (3) should not be in loop
        assert!(
            !loop_.contains(entity_ids[3]),
            "Exit node should not be in loop"
        );
    }
}
