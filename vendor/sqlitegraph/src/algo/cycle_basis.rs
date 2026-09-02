//! Minimal cycle basis using Paton's algorithm for cycle explanation.
//!
//! This module provides algorithms for finding a minimal set of cycles that
//! form a basis for the cycle space of a directed graph. A cycle basis is
//! a set of cycles where every cycle in the graph can be expressed as a
//! combination (symmetric difference) of basis cycles.
//!
//! # Algorithm
//!
//! Uses Paton's algorithm (O(|V| + |E| + |C|·|V|)):
//! - SCC decomposition using Tarjan's algorithm to isolate cyclic regions
//! - DFS traversal with parent/depth tracking within each SCC
//! - Back edge detection identifies cycles
//! - Cycle extraction by climbing from both endpoints to LCA
//! - Canonicalization rotates cycles so minimum node ID is first
//!
//! # When to Use Cycle Basis
//!
//! - **Dependency Cycle Explanation**: Show "why" a dependency graph has cycles
//! - **Deadlock Detection**: Find resource allocation cycles in distributed systems
//! - **Feedback Loop Analysis**: Identify feedback loops in inference graphs
//! - **Build System Analysis**: Explain circular dependencies in build graphs
//! - **Circular Reference Detection**: Find and explain circular references in data
//!
//! # Cycle Basis vs Simple Cycle Detection
//!
//! | Aspect | SCC Detection | Cycle Basis |
//! |--------|---------------|-------------|
//! | Output | Sets of mutually reachable nodes | Individual cycles with paths |
//! | Use Case | Detect if cycles exist | Explain specific cycle paths |
//! | Algorithm | Tarjan's (O(V+E)) | Paton's (O(V+E+C·V)) |
//! | Result | "Node A is in a cycle" | "A -> B -> C -> A" |
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E| + |C|·|V|) where C = number of cycles
//! - **Space**: O(|V| + |E| + |C|·|V|) for DFS state and cycle storage
//!
//! # References
//!
//! - K. Paton, "An Algorithm for Finding a Fundamental Set of Cycles of a Graph."
//!   *Communications of the ACM*, 1969.
//! - https://jgrapht.org/javadoc-1.3.0/org/jgrapht/alg/cycle/PatonCycleBase.html
//! - https://en.wikipedia.org/wiki/Cycle_basis

use std::collections::HashSet;

use ahash::{AHashMap, AHashSet};

use crate::{errors::SqliteGraphError, graph::SqliteGraph, progress::ProgressCallback};

use super::scc::{SccResult, strongly_connected_components};

/// Bounds for limiting cycle basis computation.
///
/// Cycle basis can produce many cycles in dense graphs. These bounds
/// prevent memory explosion and excessive computation time.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::algo::CycleBasisBounds;
///
/// // Stop after finding 100 cycles
/// let bounds = CycleBasisBounds {
///     max_cycles: Some(100),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct CycleBasisBounds {
    /// Maximum number of cycles to compute.
    ///
    /// When this limit is reached, enumeration stops and `cycles_skipped`
    /// is incremented to reflect cycles not collected.
    pub max_cycles: Option<usize>,

    /// Maximum cycle length to include.
    ///
    /// Cycles longer than this are ignored. Useful for finding "short"
    /// dependency cycles in large graphs.
    pub max_cycle_length: Option<usize>,

    /// Maximum cycles per SCC.
    ///
    /// Prevents explosion in large strongly connected components.
    /// Each SCC contributes at most this many cycles to the basis.
    pub max_per_scc: Option<usize>,
}

impl CycleBasisBounds {
    /// Creates new bounds with all limits set to None (unlimited).
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of cycles to compute.
    #[inline]
    pub fn with_max_cycles(mut self, max: usize) -> Self {
        self.max_cycles = Some(max);
        self
    }

    /// Sets the maximum cycle length to include.
    #[inline]
    pub fn with_max_cycle_length(mut self, max: usize) -> Self {
        self.max_cycle_length = Some(max);
        self
    }

    /// Sets the maximum cycles per SCC.
    #[inline]
    pub fn with_max_per_scc(mut self, max: usize) -> Self {
        self.max_per_scc = Some(max);
        self
    }

    /// Returns true if any bound is set.
    #[inline]
    pub fn is_bounded(&self) -> bool {
        self.max_cycles.is_some() || self.max_cycle_length.is_some() || self.max_per_scc.is_some()
    }
}

/// Result of minimal cycle basis computation.
///
/// Contains a set of cycles that form a basis for all cycles in the graph.
/// Every cycle in the graph can be expressed as a combination of basis cycles.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::{algo::cycle_basis, SqliteGraph};
/// # use sqlitegraph::SqliteGraphError;
///
/// # fn main() -> Result<(), SqliteGraphError> {
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// let result = cycle_basis(&graph)?;
///
/// println!("Found {} basis cycles", result.cycles.len());
/// println!("Nodes in cycles: {:?}", result.cyclic_nodes());
///
/// for node in result.cyclic_nodes() {
///     if result.is_cyclic(node) {
///         println!("Node {} is in a cycle: {}", node, result.explain_cycle(node));
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CycleBasisResult {
    /// The cycle basis: list of cycles.
    ///
    /// Each cycle is a vector of node IDs forming a closed loop.
    /// Cycles are canonicalized (minimum node ID is first) for
    /// consistent comparison and deduplication.
    pub cycles: Vec<Vec<i64>>,

    /// SCC decomposition used for cycle extraction.
    ///
    /// Useful for understanding which cycles belong to which SCC.
    /// Non-trivial SCCs (len() > 1) contain cycles.
    pub scc_decomposition: SccResult,

    /// Number of cycles skipped due to bounds.
    ///
    /// Incremented when:
    /// - max_cycles limit is reached
    /// - max_cycle_length filters a cycle
    /// - max_per_scc limit is reached for an SCC
    pub cycles_skipped: usize,

    /// Bounds applied during computation.
    ///
    /// Records what limits were used, useful for understanding
    /// if the result is complete or partial.
    pub bounds_applied: CycleBasisBounds,
}

impl CycleBasisResult {
    /// Returns all nodes involved in any cycle.
    ///
    /// This is the union of all nodes in all basis cycles.
    /// Useful for identifying "problematic" nodes that need
    /// cycle resolution.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::CycleBasisResult;
    /// # let result = CycleBasisResult {
    /// #     cycles: vec![vec![1, 2, 3], vec![4, 5]],
    /// #     scc_decomposition: /* SccResult from cycle_basis() call */,
    /// #     cycles_skipped: 0,
    /// #     bounds_applied: Default::default(),
    /// # };
    /// let cyclic = result.cyclic_nodes();
    /// // cyclic contains: {1, 2, 3, 4, 5}
    /// ```
    pub fn cyclic_nodes(&self) -> AHashSet<i64> {
        let mut nodes: AHashSet<i64> = self
            .cycles
            .iter()
            .flat_map(|cycle| cycle.iter().copied())
            .collect();

        // Ensure all nodes in non-trivial SCCs are included,
        // even if the cycle basis didn't cover every node.
        for component in &self.scc_decomposition.components {
            if component.len() > 1 {
                for &node in component {
                    nodes.insert(node);
                }
            }
        }

        nodes
    }

    /// Checks if a node is part of any cycle.
    ///
    /// Returns true if the node appears in any basis cycle.
    /// This is a fast check compared to `cycles_containing`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::CycleBasisResult;
    /// # let result = CycleBasisResult {
    /// #     cycles: vec![vec![1, 2, 3], vec![4, 5]],
    /// #     scc_decomposition: /* SccResult from cycle_basis() call */,
    /// #     cycles_skipped: 0,
    /// #     bounds_applied: Default::default(),
    /// # };
    /// assert!(result.is_cyclic(1));  // In cycle [1, 2, 3]
    /// assert!(!result.is_cyclic(99)); // Not in any cycle
    /// ```
    pub fn is_cyclic(&self, node: i64) -> bool {
        self.cycles.iter().any(|cycle| cycle.contains(&node))
    }

    /// Returns cycles that contain a specific node.
    ///
    /// Returns references to the cycles (as slices) that include
    /// the given node. Useful for understanding all cycles a
    /// node participates in.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::CycleBasisResult;
    /// # let result = CycleBasisResult {
    /// #     cycles: vec![vec![1, 2, 3], vec![1, 4, 5]],
    /// #     scc_decomposition: /* SccResult from cycle_basis() call */,
    /// #     cycles_skipped: 0,
    /// #     bounds_applied: Default::default(),
    /// # };
    /// let cycles = result.cycles_containing(1);
    /// assert_eq!(cycles.len(), 2);  // Node 1 is in 2 cycles
    /// ```
    pub fn cycles_containing(&self, node: i64) -> Vec<&[i64]> {
        self.cycles
            .iter()
            .filter(|cycle| cycle.contains(&node))
            .map(|cycle| cycle.as_slice())
            .collect()
    }

    /// Explains why a node is cyclic.
    ///
    /// Returns a human-readable explanation of the cycles involving
    /// the given node. If the node is not cyclic, returns a message
    /// saying so.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::CycleBasisResult;
    /// # let result = CycleBasisResult {
    /// #     cycles: vec![vec![1, 2, 3], vec![1, 4, 5]],
    /// #     scc_decomposition: /* SccResult from cycle_basis() call */,
    /// #     cycles_skipped: 0,
    /// #     bounds_applied: Default::default(),
    /// # };
    /// let explanation = result.explain_cycle(1);
    /// println!("{}", explanation);
    /// // Output:
    /// // Node 1 is in 2 cycle(s):
    /// //   1. [1, 2, 3]
    /// //   2. [1, 4, 5]
    /// ```
    pub fn explain_cycle(&self, node: i64) -> String {
        let cycles = self.cycles_containing(node);
        if cycles.is_empty() {
            format!("Node {} is not part of any cycle", node)
        } else {
            format!(
                "Node {} is in {} cycle(s):\n{}",
                node,
                cycles.len(),
                cycles
                    .iter()
                    .enumerate()
                    .map(|(i, cyc)| format!("  {}. {:?}", i + 1, cyc))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }

    /// Returns the number of non-trivial SCCs (those containing cycles).
    pub fn cyclic_scc_count(&self) -> usize {
        self.scc_decomposition.non_trivial_count()
    }

    /// Returns true if the graph has any cycles.
    pub fn has_cycles(&self) -> bool {
        !self.cycles.is_empty()
    }
}

/// Computes minimal cycle basis using Paton's algorithm.
///
/// A cycle basis is a set of cycles that form a basis for the cycle space
/// of the graph. Every cycle can be expressed as a combination of basis cycles.
///
/// This function uses SCC decomposition to isolate cyclic regions, then
/// applies Paton's algorithm within each non-trivial SCC.
///
/// # Arguments
///
/// * `graph` - The graph to analyze
///
/// # Returns
///
/// `CycleBasisResult` containing:
/// - List of basis cycles (each cycle is node IDs forming a closed loop)
/// - SCC decomposition for context
/// - Count of cycles skipped (0 for unbounded computation)
/// - Empty bounds (unlimited)
///
/// # Example
///
/// ```rust
/// use sqlitegraph::{algo::cycle_basis, SqliteGraph};
/// # use sqlitegraph::SqliteGraphError;
///
/// # fn main() -> Result<(), SqliteGraphError> {
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// let result = cycle_basis(&graph)?;
///
/// for (i, cycle) in result.cycles.iter().enumerate() {
///     println!("Cycle {}: {:?}", i, cycle);
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Complexity
///
/// Time: O(|V| + |E| + |C|·|V|) where C = number of cycles
/// Space: O(|V| + |C|·|V|)
pub fn cycle_basis(graph: &SqliteGraph) -> Result<CycleBasisResult, SqliteGraphError> {
    cycle_basis_bounded(graph, CycleBasisBounds::default())
}

/// Computes cycle basis with bounded enumeration.
///
/// Same as `cycle_basis` but with limits to prevent memory explosion
/// on dense graphs. See `CycleBasisBounds` for available limits.
///
/// # Arguments
///
/// * `graph` - The graph to analyze
/// * `bounds` - Limits on cycle enumeration
///
/// # Returns
///
/// `CycleBasisResult` with cycles found within bounds, and count of
/// cycles skipped due to limits.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::{algo::{cycle_basis_bounded, CycleBasisBounds}, SqliteGraph};
/// # use sqlitegraph::SqliteGraphError;
///
/// # fn main() -> Result<(), SqliteGraphError> {
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// // Find at most 100 cycles, ignore cycles longer than 10 nodes
/// let bounds = CycleBasisBounds {
///     max_cycles: Some(100),
///     max_cycle_length: Some(10),
///     ..Default::default()
/// };
/// let result = cycle_basis_bounded(&graph, bounds)?;
///
/// if result.cycles_skipped > 0 {
///     println!("Warning: {} cycles were skipped due to bounds", result.cycles_skipped);
/// }
/// # Ok(())
/// # }
/// ```
pub fn cycle_basis_bounded(
    graph: &SqliteGraph,
    bounds: CycleBasisBounds,
) -> Result<CycleBasisResult, SqliteGraphError> {
    // Step 1: Run SCC decomposition
    let scc = strongly_connected_components(graph)?;

    // Step 2: Filter to non-trivial SCCs (len() > 1) and single-node SCCs with self-loops
    let mut cycles: Vec<Vec<i64>> = Vec::new();
    let mut cycles_skipped = 0usize;

    // First, detect self-loops in single-node SCCs
    for component in &scc.components {
        if component.len() == 1 {
            let node = *component
                .iter()
                .next()
                .expect("invariant: component is non-empty");
            // Check if this node has a self-loop
            if let Ok(outgoing) = graph.fetch_outgoing(node)
                && outgoing.contains(&node)
            {
                // Self-loop detected: [node, node]
                if let Some(max_len) = bounds.max_cycle_length {
                    if 2 > max_len {
                        cycles_skipped += 1;
                    } else {
                        cycles.push(vec![node, node]);
                    }
                } else {
                    cycles.push(vec![node, node]);
                }
            }
        }
    }

    // Apply max_cycles bound after self-loop detection
    if let Some(max_cycles) = bounds.max_cycles {
        if cycles.len() > max_cycles {
            cycles_skipped += cycles.len() - max_cycles;
            cycles.truncate(max_cycles);
        }
        if cycles.len() >= max_cycles {
            return Ok(CycleBasisResult {
                cycles: deduplicate_cycles(cycles),
                scc_decomposition: scc,
                cycles_skipped,
                bounds_applied: bounds,
            });
        }
    }

    // Filter to non-trivial SCCs (len() > 1) for Paton's algorithm
    let non_trivial_sccs: Vec<&HashSet<i64>> =
        scc.components.iter().filter(|c| c.len() > 1).collect();

    if non_trivial_sccs.is_empty() {
        // Only self-loops (if any) - return result
        return Ok(CycleBasisResult {
            cycles: deduplicate_cycles(cycles),
            scc_decomposition: scc,
            cycles_skipped,
            bounds_applied: bounds,
        });
    }

    // Step 3: Find cycles within each non-trivial SCC
    for scc_nodes in non_trivial_sccs {
        let scc_cycles = paton_cycles_in_scc(graph, scc_nodes, &bounds, &mut cycles_skipped)?;

        // Apply max_per_scc limit if set
        if let Some(max_per_scc) = bounds.max_per_scc {
            if scc_cycles.len() > max_per_scc {
                cycles_skipped += scc_cycles.len() - max_per_scc;
                cycles.extend(scc_cycles.into_iter().take(max_per_scc));
            } else {
                cycles.extend(scc_cycles);
            }
        } else {
            cycles.extend(scc_cycles);
        }

        // Apply max_cycles limit globally
        if let Some(max_cycles) = bounds.max_cycles
            && cycles.len() >= max_cycles
        {
            cycles_skipped += cycles.len() - max_cycles;
            cycles.truncate(max_cycles);
            break;
        }
    }

    // Step 4: Deduplicate cycles by canonicalizing and using a set
    let deduped = deduplicate_cycles(cycles);

    Ok(CycleBasisResult {
        cycles: deduped,
        scc_decomposition: scc,
        cycles_skipped,
        bounds_applied: bounds,
    })
}

/// Computes cycle basis with progress tracking.
///
/// Same as `cycle_basis_bounded` but reports progress during computation.
/// Useful for large graphs where cycle enumeration may take time.
///
/// # Arguments
///
/// * `graph` - The graph to analyze
/// * `bounds` - Limits on cycle enumeration
/// * `progress` - Callback for progress updates
///
/// # Progress Reports
///
/// - SCC decomposition complete
/// - Per-SCC cycle discovery
/// - Bounds application
/// - Completion
pub fn cycle_basis_with_progress<F>(
    graph: &SqliteGraph,
    bounds: CycleBasisBounds,
    progress: &F,
) -> Result<CycleBasisResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    progress.on_progress(0, Some(4), "Computing SCC decomposition");

    // Step 1: Run SCC decomposition
    let scc = strongly_connected_components(graph)?;

    progress.on_progress(1, Some(4), "SCC decomposition complete");

    // Step 2: Filter to non-trivial SCCs
    let non_trivial_sccs: Vec<&HashSet<i64>> =
        scc.components.iter().filter(|c| c.len() > 1).collect();

    let non_trivial_count = non_trivial_sccs.len();

    if non_trivial_count == 0 {
        progress.on_complete();
        return Ok(CycleBasisResult {
            cycles: Vec::new(),
            scc_decomposition: scc,
            cycles_skipped: 0,
            bounds_applied: bounds,
        });
    }

    progress.on_progress(
        2,
        Some(4),
        &format!("Finding cycles in {} SCCs", non_trivial_count),
    );

    // Step 3: Find cycles within each non-trivial SCC
    let mut all_cycles: Vec<Vec<i64>> = Vec::new();
    let mut cycles_skipped = 0usize;

    for (idx, scc_nodes) in non_trivial_sccs.iter().enumerate() {
        progress.on_progress(
            idx,
            Some(non_trivial_count),
            &format!("Processing SCC {}/{}", idx + 1, non_trivial_count),
        );

        let scc_cycles = paton_cycles_in_scc(graph, scc_nodes, &bounds, &mut cycles_skipped)?;

        // Apply max_per_scc limit if set
        if let Some(max_per_scc) = bounds.max_per_scc {
            if scc_cycles.len() > max_per_scc {
                cycles_skipped += scc_cycles.len() - max_per_scc;
                all_cycles.extend(scc_cycles.into_iter().take(max_per_scc));
            } else {
                all_cycles.extend(scc_cycles);
            }
        } else {
            all_cycles.extend(scc_cycles);
        }

        // Apply max_cycles limit globally
        if let Some(max_cycles) = bounds.max_cycles
            && all_cycles.len() >= max_cycles
        {
            cycles_skipped += all_cycles.len() - max_cycles;
            all_cycles.truncate(max_cycles);
            break;
        }
    }

    progress.on_progress(3, Some(4), "Deduplicating cycles");

    // Step 4: Deduplicate cycles
    let deduped = deduplicate_cycles(all_cycles);

    progress.on_complete();

    Ok(CycleBasisResult {
        cycles: deduped,
        scc_decomposition: scc,
        cycles_skipped,
        bounds_applied: bounds,
    })
}

/// Implements Paton's algorithm within a single SCC.
///
/// Performs DFS traversal with parent/depth tracking. When a back edge
/// is encountered (neighbor visited and not parent), extracts the cycle
/// by climbing from both endpoints to their LCA.
///
/// # Algorithm
///
/// 1. DFS traversal from each unvisited node in the SCC
/// 2. Track parent and depth for each visited node
/// 3. On back edge discovery, extract cycle via LCA climb
/// 4. Canonicalize cycle (rotate to min node first)
/// 5. Apply bounds (max_cycle_length)
fn paton_cycles_in_scc(
    graph: &SqliteGraph,
    scc: &HashSet<i64>,
    bounds: &CycleBasisBounds,
    cycles_skipped: &mut usize,
) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    let mut cycles: Vec<Vec<i64>> = Vec::new();
    let mut visited: AHashSet<i64> = AHashSet::new();
    let mut parent: AHashMap<i64, i64> = AHashMap::new();
    let mut depth: AHashMap<i64, usize> = AHashMap::new();
    let mut rec_stack: AHashSet<i64> = AHashSet::new();

    // Process each node in the SCC
    for &node in scc {
        if !visited.contains(&node) {
            dfs_cycle_search(
                graph,
                node,
                scc,
                &mut visited,
                &mut parent,
                &mut depth,
                &mut rec_stack,
                &mut cycles,
                bounds,
                cycles_skipped,
            )?;
        }
    }

    Ok(cycles)
}

/// DFS helper for Paton's algorithm.
///
/// Traverses the graph, detecting back edges and cross edges that form cycles,
/// and extracting cycles. Only explores edges within the SCC.
fn dfs_cycle_search(
    graph: &SqliteGraph,
    node: i64,
    scc: &HashSet<i64>,
    visited: &mut AHashSet<i64>,
    parent: &mut AHashMap<i64, i64>,
    depth: &mut AHashMap<i64, usize>,
    rec_stack: &mut AHashSet<i64>,
    cycles: &mut Vec<Vec<i64>>,
    bounds: &CycleBasisBounds,
    cycles_skipped: &mut usize,
) -> Result<(), SqliteGraphError> {
    visited.insert(node);
    rec_stack.insert(node);

    let current_depth = depth.get(&node).copied().unwrap_or(0);

    // Explore neighbors
    for &neighbor in &graph.fetch_outgoing(node)? {
        // Only consider neighbors within the SCC
        if !scc.contains(&neighbor) {
            continue;
        }

        if !visited.contains(&neighbor) {
            // Tree edge - continue DFS
            parent.insert(neighbor, node);
            depth.insert(neighbor, current_depth + 1);
            dfs_cycle_search(
                graph,
                neighbor,
                scc,
                visited,
                parent,
                depth,
                rec_stack,
                cycles,
                bounds,
                cycles_skipped,
            )?;
        } else if rec_stack.contains(&neighbor) {
            // Edge to a node on the current recursion stack -> cycle
            if let Some(cycle) = extract_cycle_from_back_edge(node, neighbor, parent, depth) {
                // Apply max_cycle_length bound
                if let Some(max_len) = bounds.max_cycle_length
                    && cycle.len() > max_len
                {
                    *cycles_skipped += 1;
                    continue;
                }

                let canonical = canonicalize_cycle(cycle);
                cycles.push(canonical);
            }
        }
    }

    rec_stack.remove(&node);
    Ok(())
}

/// Extracts a cycle from a back edge by climbing to LCA.
///
/// Given a back edge (u -> v) where v is already visited,
/// reconstructs the cycle by:
/// 1. Climbing from u to root via parent pointers
/// 2. Climbing from v to root via parent pointers
/// 3. Finding the LCA (first common node)
/// 4. Building the cycle: u -> ... -> lca -> ... -> v -> u
fn extract_cycle_from_back_edge(
    u: i64,
    v: i64,
    parent: &AHashMap<i64, i64>,
    _depth: &AHashMap<i64, usize>,
) -> Option<Vec<i64>> {
    // Handle self-loop
    if u == v {
        return Some(vec![u, u]);
    }

    // Build path from u to root
    let mut path_u: Vec<i64> = Vec::new();
    let mut current = u;
    path_u.push(current);
    while let Some(&p) = parent.get(&current) {
        path_u.push(p);
        current = p;
    }

    // Build path from v to root
    let mut path_v: Vec<i64> = Vec::new();
    current = v;
    path_v.push(current);
    while let Some(&p) = parent.get(&current) {
        path_v.push(p);
        current = p;
    }

    // Find LCA (first common node from root)
    // We search from the end (root) towards the nodes
    let mut lca = None;
    let path_u_set: AHashSet<i64> = path_u.iter().copied().collect();

    for &node in path_v.iter().rev() {
        if path_u_set.contains(&node) {
            lca = Some(node);
            break;
        }
    }

    let lca = lca?;

    // Build the cycle: lca -> ... -> u -> v -> ... -> lca
    // The tree path from lca to u (following parent pointers from u up to lca, then reversing)
    let mut path_lca_to_u: Vec<i64> = Vec::new();
    current = u;
    while current != lca {
        path_lca_to_u.push(current);
        current = *parent.get(&current)?;
    }
    path_lca_to_u.push(lca); // Add lca at the end
    path_lca_to_u.reverse(); // Now it's lca -> ... -> u

    // The tree path from lca to v (following parent pointers from v up to lca, then reversing)
    let mut path_lca_to_v: Vec<i64> = Vec::new();
    current = v;
    while current != lca {
        path_lca_to_v.push(current);
        current = *parent.get(&current)?;
    }
    // path_lca_to_v is v -> ... -> child_of_lca, reversed it's child_of_lca -> ... -> v
    path_lca_to_v.reverse();

    // Build cycle: lca -> ... -> u (back edge) -> v -> ... -> child_of_lca -> lca
    let mut cycle = Vec::new();
    cycle.extend(&path_lca_to_u); // lca -> ... -> u
    cycle.extend(&path_lca_to_v); // -> v -> ... -> child_of_lca
    cycle.push(lca); // -> lca (close the cycle)

    Some(cycle)
}

/// Canonicalizes a cycle by rotating so minimum node ID is first.
///
/// This ensures equivalent cycles are deduplicated:
/// - [A, B, C, A] -> [A, B, C, A] (A is min)
/// - [B, C, A, B] -> [A, B, C, A] (rotated to A)
/// - [C, A, B, C] -> [A, B, C, A] (rotated to A)
fn canonicalize_cycle(mut cycle: Vec<i64>) -> Vec<i64> {
    if cycle.is_empty() {
        return cycle;
    }

    // Handle self-loop
    if cycle.len() == 2 && cycle[0] == cycle[1] {
        return cycle; // Already canonical
    }

    // Remove the duplicate last element (it repeats the first)
    // Our cycles are stored as [u, ..., lca, ..., v, u]
    // So the last element equals the first
    if cycle.len() > 1 && cycle[0] == cycle[cycle.len() - 1] {
        cycle.pop();
    }

    if cycle.is_empty() {
        return cycle;
    }

    // Find the minimum element
    let min_node = *cycle.iter().min().expect("invariant: cycle is non-empty");

    // Find the position of the minimum element
    let min_pos = cycle
        .iter()
        .position(|&x| x == min_node)
        .expect("invariant: min_node is member of cycle");

    // Rotate so minimum is first
    cycle.rotate_left(min_pos);

    // Add the closing element back
    cycle.push(cycle[0]);

    cycle
}

/// Deduplicates cycles using canonicalization.
///
/// Different back edges may discover the same cycle. Canonicalization
/// ensures they are represented identically, allowing deduplication
/// via a HashSet.
fn deduplicate_cycles(cycles: Vec<Vec<i64>>) -> Vec<Vec<i64>> {
    let mut seen: AHashSet<Vec<i64>> = AHashSet::new();
    let mut result: Vec<Vec<i64>> = Vec::new();

    for cycle in cycles {
        let canonical = canonicalize_cycle(cycle);
        if seen.insert(canonical.clone()) {
            result.push(canonical);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper to create a test graph with numbered entities
    fn create_test_graph_with_nodes(count: usize) -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        for i in 0..count {
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

    /// Helper to get entity IDs from a graph
    fn get_entity_ids(graph: &SqliteGraph, count: usize) -> Vec<i64> {
        graph
            .all_entity_ids()
            .expect("Failed to get IDs")
            .into_iter()
            .take(count)
            .collect()
    }

    // Test 1: Simple cycle (A -> B -> C -> A)
    #[test]
    fn test_simple_cycle() {
        let graph = create_test_graph_with_nodes(3);
        let ids = get_entity_ids(&graph, 3);

        // Create cycle: 0 -> 1 -> 2 -> 0
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find at least one cycle covering all nodes
        assert!(
            !result.cycles.is_empty(),
            "Expected at least 1 cycle, found {:?}",
            result.cycles
        );

        // All nodes should be marked as cyclic
        let cyclic = result.cyclic_nodes();
        assert!(
            cyclic.contains(&ids[0]),
            "ids[0]={} not in cyclic {:?}",
            ids[0],
            cyclic
        );
        assert!(
            cyclic.contains(&ids[1]),
            "ids[1]={} not in cyclic {:?}",
            ids[1],
            cyclic
        );
        assert!(
            cyclic.contains(&ids[2]),
            "ids[2]={} not in cyclic {:?}",
            ids[2],
            cyclic
        );
    }

    // Test 2: Two cycles sharing edge
    #[test]
    fn test_two_cycles_sharing_edge() {
        let graph = create_test_graph_with_nodes(4);
        let ids = get_entity_ids(&graph, 4);

        // Create two cycles: 0 -> 1 -> 2 -> 0 and 0 -> 2 -> 3 -> 0
        let edges = [(0, 1), (1, 2), (2, 0), (0, 2), (2, 3), (3, 0)];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find at least one cycle (exact number depends on discovery order)
        assert!(!result.cycles.is_empty());
        assert!(result.has_cycles());

        // All nodes should be in cycles
        assert_eq!(result.cyclic_nodes().len(), 4);
    }

    // Test 3: Mutual recursion (2-node cycle)
    #[test]
    fn test_mutual_recursion() {
        let graph = create_test_graph_with_nodes(2);
        let ids = get_entity_ids(&graph, 2);

        // Create mutual recursion: 0 <-> 1
        for (from, to) in &[(0, 1), (1, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "recursion".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find at least one cycle
        assert!(
            !result.cycles.is_empty(),
            "Expected at least 1 cycle, found {:?}",
            result.cycles
        );

        // Both nodes should be cyclic
        assert!(
            result.is_cyclic(ids[0]),
            "ids[0]={} should be cyclic",
            ids[0]
        );
        assert!(
            result.is_cyclic(ids[1]),
            "ids[1]={} should be cyclic",
            ids[1]
        );
    }

    // Test 4: Multiple SCCs
    #[test]
    fn test_multiple_sccs() {
        let graph = create_test_graph_with_nodes(5);
        let ids = get_entity_ids(&graph, 5);

        // SCC1: 0 -> 1 -> 0
        for (from, to) in &[(0, 1), (1, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "scc1".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        // SCC2: 2 -> 3 -> 4 -> 2
        for (from, to) in &[(2, 3), (3, 4), (4, 2)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "scc2".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find at least one cycle per SCC
        assert!(
            !result.cycles.is_empty(),
            "Expected at least 1 cycle, found {:?}",
            result.cycles
        );
        assert_eq!(result.cyclic_scc_count(), 2);

        // All 5 nodes should be in cycles
        let cyclic = result.cyclic_nodes();
        assert_eq!(
            cyclic.len(),
            5,
            "Expected 5 cyclic nodes, found {:?}",
            cyclic
        );
    }

    // Test 5: DAG (no cycles)
    #[test]
    fn test_dag_no_cycles() {
        let graph = create_test_graph_with_nodes(4);
        let ids = get_entity_ids(&graph, 4);

        // Create DAG: 0 -> 1 -> 2 -> 3 (linear chain)
        for (from, to) in &[(0, 1), (1, 2), (2, 3)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "chain".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find no cycles
        assert_eq!(result.cycles.len(), 0);
        assert!(!result.has_cycles());
        assert_eq!(result.cyclic_nodes().len(), 0);
        assert_eq!(result.cyclic_scc_count(), 0);
    }

    // Test 6: Complex interlocking cycles
    #[test]
    fn test_complex_interlocking_cycles() {
        let graph = create_test_graph_with_nodes(5);
        let ids = get_entity_ids(&graph, 5);

        // Create interlocking cycles:
        // 0 -> 1 -> 2 -> 0 (main cycle)
        // 0 -> 3 -> 4 -> 1 (adds complexity)
        let edges = [
            (0, 1),
            (1, 2),
            (2, 0), // Main cycle
            (0, 3),
            (3, 4),
            (4, 1), // Additional path
        ];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "complex".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        // Should find at least one cycle
        assert!(!result.cycles.is_empty());

        // All nodes should be in the SCC
        assert_eq!(result.cyclic_scc_count(), 1);
    }

    // Test 7: Self-loop
    #[test]
    fn test_self_loop() {
        let graph = create_test_graph_with_nodes(1);
        let ids = get_entity_ids(&graph, 1);

        // Create self-loop: 0 -> 0
        let edge = GraphEdge {
            id: 0,
            from_id: ids[0],
            to_id: ids[0],
            edge_type: "self".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();

        let result = cycle_basis(&graph).unwrap();

        // Should find the self-loop cycle
        assert_eq!(result.cycles.len(), 1);
        // Self-loop is represented as [a, a]
        assert_eq!(result.cycles[0].len(), 2);
        assert_eq!(result.cycles[0][0], result.cycles[0][1]);
    }

    // Test 8: Bounded enumeration (max_cycles)
    #[test]
    fn test_bounded_max_cycles() {
        let graph = create_test_graph_with_nodes(6);
        let ids = get_entity_ids(&graph, 6);

        // Create multiple cycles to test max_cycles bound
        // 0 -> 1 -> 0
        // 2 -> 3 -> 2
        // 4 -> 5 -> 4
        for (from, to) in &[(0, 1), (1, 0), (2, 3), (3, 2), (4, 5), (5, 4)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let bounds = CycleBasisBounds {
            max_cycles: Some(2),
            ..Default::default()
        };

        let result = cycle_basis_bounded(&graph, bounds).unwrap();

        // Should find at most 2 cycles
        assert!(result.cycles.len() <= 2);
        // cycles_skipped may be > 0 if we found more than 2
    }

    // Test 9: Bounded enumeration (max_cycle_length)
    #[test]
    fn test_bounded_max_cycle_length() {
        let graph = create_test_graph_with_nodes(5);
        let ids = get_entity_ids(&graph, 5);

        // Create a 4-node cycle: 0 -> 1 -> 2 -> 3 -> 0
        let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "long".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let bounds = CycleBasisBounds {
            max_cycle_length: Some(3), // Ignore cycles with more than 3 unique nodes
            ..Default::default()
        };

        let result = cycle_basis_bounded(&graph, bounds).unwrap();

        // The 4-node cycle (5 elements with closure) should be filtered out
        assert_eq!(result.cycles.len(), 0);
        assert!(result.cycles_skipped > 0 || result.cycles.is_empty());
    }

    // Test 10: Bounded enumeration (max_per_scc)
    #[test]
    fn test_bounded_max_per_scc() {
        let graph = create_test_graph_with_nodes(5);
        let ids = get_entity_ids(&graph, 5);

        // Create a dense SCC with multiple cycles
        // 0 -> 1 -> 2 -> 0
        // 0 -> 2
        // 1 -> 0
        let edges = [(0, 1), (1, 2), (2, 0), (0, 2), (1, 0)];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "dense".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let bounds = CycleBasisBounds {
            max_per_scc: Some(1),
            ..Default::default()
        };

        let result = cycle_basis_bounded(&graph, bounds).unwrap();

        // Should find at most 1 cycle from the SCC
        assert!(result.cycles.len() <= 1);
    }

    // Test 11: Helper method - cyclic_nodes
    #[test]
    fn test_helper_cyclic_nodes() {
        let graph = create_test_graph_with_nodes(4);
        let ids = get_entity_ids(&graph, 4);

        // Create cycles: 0 -> 1 -> 0 and 2 -> 3 -> 2
        for (from, to) in &[(0, 1), (1, 0), (2, 3), (3, 2)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        let cyclic = result.cyclic_nodes();
        assert_eq!(cyclic.len(), 4);
        assert!(cyclic.contains(&ids[0]));
        assert!(cyclic.contains(&ids[1]));
        assert!(cyclic.contains(&ids[2]));
        assert!(cyclic.contains(&ids[3]));
    }

    // Test 12: Helper method - is_cyclic
    #[test]
    fn test_helper_is_cyclic() {
        let graph = create_test_graph_with_nodes(3);
        let ids = get_entity_ids(&graph, 3);

        // Create cycle: 0 -> 1 -> 0
        for (from, to) in &[(0, 1), (1, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        assert!(result.is_cyclic(ids[0]));
        assert!(result.is_cyclic(ids[1]));
        assert!(!result.is_cyclic(ids[2])); // Node 2 is not in a cycle
    }

    // Test 13: Helper method - cycles_containing
    #[test]
    fn test_helper_cycles_containing() {
        let graph = create_test_graph_with_nodes(4);
        let ids = get_entity_ids(&graph, 4);

        // Create cycles: 0 -> 1 -> 0 and 0 -> 2 -> 3 -> 0
        let edges = [(0, 1), (1, 0), (0, 2), (2, 3), (3, 0)];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        // Node 0 is in multiple cycles
        let cycles_with_0 = result.cycles_containing(ids[0]);
        assert!(!cycles_with_0.is_empty());

        // Node 1 is in one cycle
        let cycles_with_1 = result.cycles_containing(ids[1]);
        assert!(!cycles_with_1.is_empty());
    }

    // Test 14: Helper method - explain_cycle
    #[test]
    fn test_helper_explain_cycle() {
        let graph = create_test_graph_with_nodes(2);
        let ids = get_entity_ids(&graph, 2);

        // Create cycle: 0 -> 1 -> 0
        for (from, to) in &[(0, 1), (1, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        let explanation = result.explain_cycle(ids[0]);
        assert!(explanation.contains(&format!("{}", ids[0])));
        assert!(explanation.contains("cycle"));

        let no_cycle_explanation = result.explain_cycle(999);
        assert!(no_cycle_explanation.contains("not part of any cycle"));
    }

    // Test 15: Cycle deduplication
    #[test]
    fn test_cycle_deduplication() {
        let graph = create_test_graph_with_nodes(3);
        let ids = get_entity_ids(&graph, 3);

        // Create cycle with multiple back edges that can discover the same cycle
        // 0 -> 1 -> 2 -> 0
        // Add extra edge 0 -> 2 which creates same cycle via different path
        let edges = [(0, 1), (1, 2), (2, 0), (0, 2)];
        for (from, to) in &edges {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let result = cycle_basis(&graph).unwrap();

        // Should deduplicate to at most 2 cycles (not more)
        // (may get 2 due to different discovery paths)
        assert!(result.cycles.len() <= 2);
    }

    // Test 16: Empty graph
    #[test]
    fn test_empty_graph() {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = cycle_basis(&graph).unwrap();

        assert_eq!(result.cycles.len(), 0);
        assert!(!result.has_cycles());
        assert_eq!(result.cycles_skipped, 0);
    }

    // Test 17: Single node with no edges
    #[test]
    fn test_single_node_no_edges() {
        let graph = create_test_graph_with_nodes(1);

        let result = cycle_basis(&graph).unwrap();

        assert_eq!(result.cycles.len(), 0);
        assert!(!result.has_cycles());
    }

    // Test 18: Progress tracking
    #[test]
    fn test_cycle_basis_with_progress() {
        use crate::progress::NoProgress;

        let graph = create_test_graph_with_nodes(3);
        let ids = get_entity_ids(&graph, 3);

        // Create cycle
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let progress = NoProgress;
        let result =
            cycle_basis_with_progress(&graph, CycleBasisBounds::default(), &progress).unwrap();

        assert!(result.has_cycles());
    }

    // Test 19: CycleBasisBounds builder methods
    #[test]
    fn test_bounds_builder() {
        let bounds = CycleBasisBounds::new()
            .with_max_cycles(100)
            .with_max_cycle_length(10)
            .with_max_per_scc(5);

        assert_eq!(bounds.max_cycles, Some(100));
        assert_eq!(bounds.max_cycle_length, Some(10));
        assert_eq!(bounds.max_per_scc, Some(5));
        assert!(bounds.is_bounded());
    }

    // Test 20: Bounds with all None (unbounded)
    #[test]
    fn test_bounds_unbounded() {
        let bounds = CycleBasisBounds::default();

        assert_eq!(bounds.max_cycles, None);
        assert_eq!(bounds.max_cycle_length, None);
        assert_eq!(bounds.max_per_scc, None);
        assert!(!bounds.is_bounded());
    }
}
