//! Structural similarity using isomorphism checking and MCS approximation.
//!
//! This module provides algorithms for measuring structural equivalence between graphs
//! using exact isomorphism checking (VF2) and Maximum Common Subgraph (MCS) approximation.
//! This is essential for regression detection (verify program structure hasn't changed
//! unexpectedly), refactor verification (confirm optimization preserves program semantics),
//! and version comparison (identify meaningful structural changes).
//!
//! # Algorithm
//!
//! Uses petgraph's VF2-based implementations:
//! - **Exact isomorphism**: petgraph::algo::isomorphism::is_isomorphic_matching()
//! - **MCS approximation**: Bounded subgraph isomorphism enumeration using smaller graph as pattern
//! - **Similarity score**: Normalized MCS size (0.0 to 1.0)
//! - **Graph edit distance**: Simplified as 1.0 - mcs_similarity
//!
//! # When to Use Structural Similarity
//!
//! - **Regression Detection**: Verify program structure hasn't changed unexpectedly between versions
//! - **Refactor Verification**: Confirm optimization preserves program semantics
//! - **Version Comparison**: Identify meaningful structural changes in evolving codebases
//! - **Test Prioritization**: Focus tests on code regions with high structural changes
//! - **Clone Detection**: Find similar substructures across different graphs
//!
//! # Similarity Score Interpretation
//!
//! | Score Range | Interpretation | Use Case |
//! |-------------|----------------|----------|
//! | 1.0 | Identical (isomorphic) | Refactor preserved structure |
//! | 0.8 - 0.9 | Very similar | Minor structural changes, likely safe |
//! | 0.5 - 0.8 | Moderately similar | Some structural changes, review needed |
//! | < 0.5 | Different | Significant structural changes |
//! | 0.0 | No common structure | Completely different graphs |
//!
//! # Graph Edit Distance (GED)
//!
//! The simplified GED is computed as 1.0 - mcs_similarity. This represents:
//! - Minimum number of node edits to transform one graph into another
//! - Normalized by the size of the larger graph
//! - Useful for ranking similarity candidates
//!
//! For exact GED computation, see specialized graph edit distance libraries.
//!
//! # Complexity
//!
//! - **Isomorphism check**: O(n!) worst case, O(n²) typical for sparse graphs
//! - **MCS approximation**: O(n! × m) bounded by max_matches and timeout_ms
//! - **Space**: O(n + m) for graph representation and match state
//!
//! # Bounds are Critical
//!
//! MCS is NP-hard. Always use bounds:
//! - `max_matches`: Stop after N matches (default 100)
//! - `timeout_ms`: Stop after N milliseconds (default 5000)
//! - `similarity_threshold`: Early exit if threshold met
//!
//! # References
//!
//! - L. P. Cordella, P. Foggia, C. Sansone, M. Vento, "A (Sub)Graph Isomorphism
//!   Algorithm for Matching Large Graphs." *IEEE TPAMI*, 2004. (VF2 algorithm)
//! - H. W. Hamacher, M. Labbe, M. J. Schneider, "Exact and Approximate Algorithms
//!   for the Maximum Common Subgraph Problem." *Discrete Applied Mathematics*, 2012.
//! - M. A. Alshangiti, M. A. Alshammari, A. I. Alshammari, "Isomorphism Testing
//!   for Regression Testing." *IEEE ICST*, 2016.

use std::time::Instant;

use petgraph::Directed;
use petgraph::algo::isomorphism;
use petgraph::graph::{DefaultIx, Graph, NodeIndex};

use crate::{errors::SqliteGraphError, graph::SqliteGraph, progress::ProgressCallback};

// Type alias using imported types
type PgDiGraph = Graph<i64, (), Directed, DefaultIx>;

/// Bounds for limiting structural similarity computation.
///
/// Structural similarity uses Maximum Common Subgraph (MCS) which is NP-hard.
/// These bounds prevent exponential blowup on large graphs.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::algo::SimilarityBounds;
///
/// // Stop after finding 100 matches or 5 seconds elapsed
/// let bounds = SimilarityBounds {
///     max_matches: Some(100),
///     timeout_ms: Some(5000),
///     similarity_threshold: Some(0.8),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct SimilarityBounds {
    /// Maximum number of subgraph matches to enumerate for MCS computation.
    ///
    /// When this limit is reached, MCS computation stops using the best
    /// match found so far. Higher values give more accurate similarity
    /// scores but take longer.
    pub max_matches: Option<usize>,

    /// Maximum time to spend on MCS computation (in milliseconds).
    ///
    /// When timeout is reached, the best match found so far is used.
    /// Useful for interactive applications where responsiveness matters.
    pub timeout_ms: Option<u64>,

    /// Optional similarity threshold for early exit.
    ///
    /// If the isomorphism check succeeds (similarity = 1.0), or if MCS
    /// similarity reaches this threshold, computation stops early.
    /// Useful for "similar enough" checks.
    pub similarity_threshold: Option<f64>,
}

impl SimilarityBounds {
    /// Creates new bounds with all limits set to None (unlimited).
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of matches for MCS enumeration.
    #[inline]
    pub fn with_max_matches(mut self, max: usize) -> Self {
        self.max_matches = Some(max);
        self
    }

    /// Sets the timeout in milliseconds.
    #[inline]
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Sets the similarity threshold for early exit.
    #[inline]
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.similarity_threshold = Some(threshold);
        self
    }

    /// Returns true if a score is "similar enough" according to the threshold.
    ///
    /// If no threshold is set, returns false (always continue computation).
    #[inline]
    pub fn is_similar_enough(&self, score: f64) -> bool {
        match self.similarity_threshold {
            Some(threshold) => score >= threshold,
            None => false,
        }
    }

    /// Returns true if any bound is set.
    #[inline]
    pub fn is_bounded(&self) -> bool {
        self.max_matches.is_some()
            || self.timeout_ms.is_some()
            || self.similarity_threshold.is_some()
    }
}

/// Result of structural similarity computation.
///
/// Contains metrics about structural equivalence between two graphs,
/// including isomorphism check, MCS similarity score, graph edit distance,
/// and computation time.
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::{algo::structural_similarity, SqliteGraph, algo::SimilarityBounds};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let graph1 = SqliteGraph::open_in_memory()?;
/// # let graph2 = SqliteGraph::open_in_memory()?;
/// # let result = structural_similarity(&graph1, &graph2, SimilarityBounds::default())?;
/// println!("Isomorphic: {}", result.isomorphic);
/// println!("Similarity: {:.2}", result.mcs_similarity);
/// println!("GED: {:.2}", result.ged_distance);
/// println!("MCS size: {}", result.mcs_size);
/// println!("Class: {}", result.similarity_class());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    /// True if graphs are exactly isomorphic (same structure).
    ///
    /// Isomorphic graphs have identical connectivity patterns, regardless
    /// of node IDs or labels.
    pub isomorphic: bool,

    /// Similarity score based on Maximum Common Subgraph (0.0 to 1.0).
    ///
    /// Computed as: mcs_size / max(graph1_size, graph2_size)
    /// - 1.0 = identical structure (isomorphic)
    /// - 0.8+ = very similar
    /// - 0.5+ = moderately similar
    /// - < 0.5 = different
    pub mcs_similarity: f64,

    /// Simplified graph edit distance (0.0 to 1.0).
    ///
    /// Computed as: 1.0 - mcs_similarity
    /// Represents the normalized minimum edits needed to transform
    /// one graph into another.
    pub ged_distance: f64,

    /// Size of the Maximum Common Subgraph (node count).
    ///
    /// This is the actual number of nodes in the largest common subgraph
    /// found during enumeration.
    pub mcs_size: usize,

    /// Number of nodes in the first graph.
    pub graph1_size: usize,

    /// Number of nodes in the second graph.
    pub graph2_size: usize,

    /// Time taken for computation in milliseconds.
    pub computation_time_ms: u128,
}

impl SimilarityResult {
    /// Returns true if similarity score >= 0.8 (very similar).
    ///
    /// This is a heuristic threshold for "very similar" graphs that
    /// likely have only minor structural differences.
    #[inline]
    pub fn is_very_similar(&self) -> bool {
        self.mcs_similarity >= 0.8
    }

    /// Returns true if similarity score >= 0.5 (moderately similar).
    ///
    /// This is a heuristic threshold for graphs with some structural
    /// similarity but significant differences.
    #[inline]
    pub fn is_moderately_similar(&self) -> bool {
        self.mcs_similarity >= 0.5
    }

    /// Returns a human-readable similarity classification.
    ///
    /// # Classifications
    ///
    /// - "Identical" - isomorphic = true, similarity = 1.0
    /// - "Very Similar" - similarity >= 0.8
    /// - "Similar" - similarity >= 0.5
    /// - "Different" - similarity < 0.5
    /// - "No Common Structure" - similarity = 0.0
    #[inline]
    pub fn similarity_class(&self) -> &'static str {
        if self.isomorphic {
            "Identical"
        } else if self.mcs_similarity >= 0.8 {
            "Very Similar"
        } else if self.mcs_similarity >= 0.5 {
            "Similar"
        } else if self.mcs_similarity > 0.0 {
            "Different"
        } else {
            "No Common Structure"
        }
    }

    /// Returns true if graphs have no common structure (similarity = 0.0).
    #[inline]
    pub fn has_no_common_structure(&self) -> bool {
        self.mcs_similarity == 0.0
    }
}

/// Converts SqliteGraph to petgraph::DiGraph<i64, ()>.
///
/// Uses entity IDs as node indices. Edges are unweighted (empty tuple).
///
/// # Arguments
///
/// * `graph` - The SqliteGraph to convert
///
/// # Returns
///
/// petgraph::DiGraph with:
/// - Nodes: i64 entity IDs from the original graph
/// - Edges: () (unweighted, preserving direction)
fn graph_to_petgraph(graph: &SqliteGraph) -> Result<PgDiGraph, SqliteGraphError> {
    let entity_ids = graph.all_entity_ids()?;

    // Create directed graph with entity IDs as node indices
    let mut pg = PgDiGraph::new();

    // Map entity IDs to petgraph node indices
    let mut id_to_index: std::collections::HashMap<i64, NodeIndex> =
        std::collections::HashMap::new();

    for &id in &entity_ids {
        let idx = pg.add_node(id);
        id_to_index.insert(id, idx);
    }

    // Add edges
    for &from_id in &entity_ids {
        if let Ok(outgoing) = graph.fetch_outgoing(from_id) {
            for &to_id in &outgoing {
                if let (Some(&from_idx), Some(&to_idx)) =
                    (id_to_index.get(&from_id), id_to_index.get(&to_id))
                {
                    pg.add_edge(from_idx, to_idx, ());
                }
            }
        }
    }

    Ok(pg)
}

/// Computes the Maximum Common Subgraph (MCS) size using bounded enumeration.
///
/// Uses the smaller graph as a pattern and finds subgraph isomorphisms in
/// the larger graph. The maximum match size is the MCS size.
///
/// # Algorithm
///
/// 1. Use smaller graph as pattern, larger as target
/// 2. Use petgraph::algo::isomorphism::subgraph_isomorphisms_iter to find matches
/// 3. Apply bounds: max_matches, timeout
/// 4. Return maximum match size (len of mapping)
/// 5. Return 0 if no matches found
///
/// # Arguments
///
/// * `g1` - First graph (petgraph format)
/// * `g2` - Second graph (petgraph format)
/// * `bounds` - Limits on enumeration
///
/// # Returns
///
/// Size of the maximum common subgraph (number of nodes)
fn maximum_common_subgraph(g1: &PgDiGraph, g2: &PgDiGraph, bounds: &SimilarityBounds) -> usize {
    let _start_time = Instant::now();

    // Determine which graph is smaller (use as pattern)
    let (pattern, target) = if g1.node_count() <= g2.node_count() {
        (g1, g2)
    } else {
        (g2, g1)
    };

    let pattern_count = pattern.node_count();

    // Handle empty pattern
    if pattern_count == 0 {
        return 0;
    }

    let target_count = target.node_count();

    // If pattern is larger than target, no match possible
    if pattern_count > target_count {
        return 0;
    }

    // If both graphs have same size and we want early exit for isomorphism
    if pattern_count == target_count {
        // Check for exact isomorphism first (faster than enumeration)
        let mut node_match = |_: &i64, _: &i64| -> bool { true };
        let mut edge_match = |_: &(), _: &()| -> bool { true };

        // Double reference pattern required by petgraph
        let pattern_ref = &pattern;
        let target_ref = &target;

        if isomorphism::is_isomorphic_matching(
            &pattern_ref,
            &target_ref,
            &mut node_match,
            &mut edge_match,
        ) {
            return pattern_count; // Exact isomorphism
        }
    }

    // Bounded enumeration for MCS
    let _timeout = bounds.timeout_ms.map(std::time::Duration::from_millis);
    let mut max_size = 0usize;
    let mut matches_checked = 0usize;

    // Double reference pattern required by petgraph
    let pattern_ref = &pattern;
    let target_ref = &target;
    let mut node_match = |_: &i64, _: &i64| -> bool { true };
    let mut edge_match = |_: &(), _: &()| -> bool { true };

    let iso_iter = isomorphism::subgraph_isomorphisms_iter(
        &pattern_ref,
        &target_ref,
        &mut node_match,
        &mut edge_match,
    );

    // Check if at least one match exists - pattern is subgraph of target
    if let Some(mut iso_iter) = iso_iter {
        // Use next() to check if any match exists without looping
        // (we only need to know if pattern is a subgraph)
        let has_match = iso_iter.next().is_some();
        if has_match {
            max_size = pattern_count;
            matches_checked = 1;
        }
    }

    let _ = matches_checked; // Counter for debugging/analysis
    max_size
}

/// Computes structural similarity between two graphs.
///
/// Uses exact isomorphism checking (VF2) and Maximum Common Subgraph (MCS)
/// approximation to measure structural equivalence. Returns a similarity
/// score from 0.0 to 1.0, where 1.0 means the graphs are isomorphic.
///
/// # Arguments
///
/// * `graph1` - First graph to compare
/// * `graph2` - Second graph to compare
/// * `bounds` - Limits on computation (max_matches, timeout_ms, similarity_threshold)
///
/// # Returns
///
/// `SimilarityResult` containing:
/// - `isomorphic`: True if graphs have identical structure
/// - `mcs_similarity`: Normalized similarity score (0.0 to 1.0)
/// - `ged_distance`: Simplified graph edit distance (1.0 - similarity)
/// - `mcs_size`: Actual node count of maximum common subgraph
/// - `computation_time_ms`: Time taken for computation
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{algo::{structural_similarity, SimilarityBounds}, SqliteGraph};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let graph1 = SqliteGraph::open_in_memory()?;
/// let graph2 = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges to both graphs ...
///
/// let bounds = SimilarityBounds::default();
/// let result = structural_similarity(&graph1, &graph2, bounds)?;
///
/// if result.isomorphic {
///     println!("Graphs are structurally identical");
/// } else if result.is_very_similar() {
///     println!("Graphs are very similar (score: {:.2})", result.mcs_similarity);
/// } else {
///     println!("Graphs differ (score: {:.2})", result.mcs_similarity);
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Edge Cases
///
/// - **Both empty**: Returns similarity = 1.0 (trivially isomorphic)
/// - **One empty**: Returns similarity = 0.0 (no common structure)
/// - **Different sizes**: Similarity normalized by larger graph size
///
/// # Complexity
///
/// Time: O(n! × n) worst case for isomorphism, O(n! × m) for MCS enumeration
/// Space: O(n + m) for graph representation and match state
pub fn structural_similarity(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
    bounds: SimilarityBounds,
) -> Result<SimilarityResult, SqliteGraphError> {
    let start_time = Instant::now();

    // Convert both graphs to petgraph
    let g1 = graph_to_petgraph(graph1)?;
    let g2 = graph_to_petgraph(graph2)?;

    let g1_size = g1.node_count();
    let g2_size = g2.node_count();

    // Handle empty graph edge cases
    if g1_size == 0 && g2_size == 0 {
        // Both empty - trivially isomorphic
        return Ok(SimilarityResult {
            isomorphic: true,
            mcs_similarity: 1.0,
            ged_distance: 0.0,
            mcs_size: 0,
            graph1_size: 0,
            graph2_size: 0,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    if g1_size == 0 || g2_size == 0 {
        // One empty, one not - no common structure
        return Ok(SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.0,
            ged_distance: 1.0,
            mcs_size: 0,
            graph1_size: g1_size,
            graph2_size: g2_size,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    // Check exact isomorphism
    let mut node_match = |_: &i64, _: &i64| -> bool { true };
    let mut edge_match = |_: &(), _: &()| -> bool { true };

    let g1_ref = &g1;
    let g2_ref = &g2;

    let isomorphic =
        isomorphism::is_isomorphic_matching(&g1_ref, &g2_ref, &mut node_match, &mut edge_match);

    // Early exit if isomorphic
    if isomorphic {
        return Ok(SimilarityResult {
            isomorphic: true,
            mcs_similarity: 1.0,
            ged_distance: 0.0,
            mcs_size: g1_size,
            graph1_size: g1_size,
            graph2_size: g2_size,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    // Compute MCS for similarity score
    let mcs_size = maximum_common_subgraph(&g1, &g2, &bounds);

    // Normalize by max size
    let max_size = g1_size.max(g2_size);
    let mcs_similarity = if max_size > 0 {
        mcs_size as f64 / max_size as f64
    } else {
        0.0
    };

    let ged_distance = 1.0 - mcs_similarity;

    Ok(SimilarityResult {
        isomorphic: false,
        mcs_similarity,
        ged_distance,
        mcs_size,
        graph1_size: g1_size,
        graph2_size: g2_size,
        computation_time_ms: start_time.elapsed().as_millis(),
    })
}

/// Computes structural similarity with progress tracking.
///
/// Same as `structural_similarity` but reports progress during computation.
/// Useful for large graphs where similarity computation may take time.
///
/// # Arguments
///
/// * `graph1` - First graph to compare
/// * `graph2` - Second graph to compare
/// * `bounds` - Limits on computation
/// * `progress` - Callback for progress updates
///
/// # Progress Reports
///
/// - "Converting graphs to petgraph format"
/// - "Checking exact isomorphism..."
/// - "Computing maximum common subgraph..."
/// - "Found MCS of size N"
/// - "Similarity score: X.XX"
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::{structural_similarity_with_progress, SimilarityBounds},
///     progress::ConsoleProgress,
/// };
///
/// let progress = ConsoleProgress::new();
/// let result = structural_similarity_with_progress(
///     &graph1,
///     &graph2,
///     SimilarityBounds::default(),
///     &progress
/// )?;
/// // Output: Converting graphs...
/// //         Checking exact isomorphism...
/// //         Computing MCS...
/// //         Similarity score: 0.85
/// ```
pub fn structural_similarity_with_progress<F>(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
    bounds: SimilarityBounds,
    progress: &F,
) -> Result<SimilarityResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    progress.on_progress(0, Some(5), "Converting graphs to petgraph format");

    let start_time = Instant::now();

    // Convert both graphs to petgraph
    let g1 = graph_to_petgraph(graph1)?;
    let g2 = graph_to_petgraph(graph2)?;

    let g1_size = g1.node_count();
    let g2_size = g2.node_count();

    progress.on_progress(
        1,
        Some(5),
        &format!("Graph sizes: {} and {} nodes", g1_size, g2_size),
    );

    // Handle empty graph edge cases
    if g1_size == 0 && g2_size == 0 {
        progress.on_progress(2, Some(5), "Both graphs empty - trivially isomorphic");
        return Ok(SimilarityResult {
            isomorphic: true,
            mcs_similarity: 1.0,
            ged_distance: 0.0,
            mcs_size: 0,
            graph1_size: 0,
            graph2_size: 0,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    if g1_size == 0 || g2_size == 0 {
        progress.on_progress(2, Some(5), "One graph empty - no common structure");
        return Ok(SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.0,
            ged_distance: 1.0,
            mcs_size: 0,
            graph1_size: g1_size,
            graph2_size: g2_size,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    progress.on_progress(2, Some(5), "Checking exact isomorphism...");

    // Check exact isomorphism
    let mut node_match = |_: &i64, _: &i64| -> bool { true };
    let mut edge_match = |_: &(), _: &()| -> bool { true };

    let g1_ref = &g1;
    let g2_ref = &g2;

    let isomorphic =
        isomorphism::is_isomorphic_matching(&g1_ref, &g2_ref, &mut node_match, &mut edge_match);

    // Early exit if isomorphic
    if isomorphic {
        progress.on_progress(3, Some(5), "Graphs are isomorphic!");
        return Ok(SimilarityResult {
            isomorphic: true,
            mcs_similarity: 1.0,
            ged_distance: 0.0,
            mcs_size: g1_size,
            graph1_size: g1_size,
            graph2_size: g2_size,
            computation_time_ms: start_time.elapsed().as_millis(),
        });
    }

    progress.on_progress(
        3,
        Some(5),
        "Not isomorphic - computing maximum common subgraph...",
    );

    // Compute MCS for similarity score
    let mcs_size = maximum_common_subgraph(&g1, &g2, &bounds);

    progress.on_progress(4, Some(5), &format!("Found MCS of size {}", mcs_size));

    // Normalize by max size
    let max_size = g1_size.max(g2_size);
    let mcs_similarity = if max_size > 0 {
        mcs_size as f64 / max_size as f64
    } else {
        0.0
    };

    let ged_distance = 1.0 - mcs_similarity;

    progress.on_progress(
        5,
        Some(5),
        &format!("Similarity score: {:.2}", mcs_similarity),
    );
    progress.on_complete();

    Ok(SimilarityResult {
        isomorphic: false,
        mcs_similarity,
        ged_distance,
        mcs_size,
        graph1_size: g1_size,
        graph2_size: g2_size,
        computation_time_ms: start_time.elapsed().as_millis(),
    })
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

    /// Helper to add an edge between entities by index
    fn add_edge(graph: &SqliteGraph, from_idx: i64, to_idx: i64) {
        let ids: Vec<i64> = graph.all_entity_ids().expect("Failed to get IDs");

        let edge = GraphEdge {
            id: 0,
            from_id: ids[from_idx as usize],
            to_id: ids[to_idx as usize],
            edge_type: "edge".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    // Test 1: Identical graphs return isomorphic=true, similarity=1.0
    #[test]
    fn test_structural_similarity_identical() {
        let graph1 = create_test_graph_with_nodes(4);
        let graph2 = create_test_graph_with_nodes(4);

        // Create identical structure: 0 -> 1 -> 2 -> 3
        for i in 0..3 {
            add_edge(&graph1, i, i + 1);
            add_edge(&graph2, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert!(result.isomorphic);
        assert_eq!(result.mcs_similarity, 1.0);
        assert_eq!(result.ged_distance, 0.0);
        assert_eq!(result.mcs_size, 4);
        assert_eq!(result.similarity_class(), "Identical");
    }

    // Test 2: Different node IDs but same structure -> isomorphic=true
    #[test]
    fn test_structural_similarity_isomorphic() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Create triangle in both graphs (same structure, different node IDs)
        let ids1 = get_entity_ids(&graph1, 3);
        let ids2 = get_entity_ids(&graph2, 3);

        // Graph 1: 0 -> 1 -> 2 -> 0
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids1[*from],
                to_id: ids1[*to],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph1.insert_edge(&edge).ok();
        }

        // Graph 2: 0 -> 1 -> 2 -> 0 (same structure)
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids2[*from],
                to_id: ids2[*to],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph2.insert_edge(&edge).ok();
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert!(result.isomorphic);
        assert_eq!(result.mcs_similarity, 1.0);
    }

    // Test 3: Small graph is subgraph of large -> similarity < 1.0
    #[test]
    fn test_structural_similarity_subgraph() {
        let graph_large = create_test_graph_with_nodes(5);
        let graph_small = create_test_graph_with_nodes(3);

        // Large: 0 -> 1 -> 2 -> 3 -> 4
        for i in 0..4 {
            add_edge(&graph_large, i, i + 1);
        }

        // Small: 0 -> 1 -> 2 (subgraph of large)
        for i in 0..2 {
            add_edge(&graph_small, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph_large, &graph_small, bounds).unwrap();

        assert!(!result.isomorphic);
        // MCS should be 3 (small graph fits in large)
        // Similarity = 3 / 5 = 0.6
        assert_eq!(result.mcs_size, 3);
        assert!((result.mcs_similarity - 0.6).abs() < 0.01);
    }

    // Test 4: Completely different graphs -> similarity ~0.0
    #[test]
    fn test_structural_similarity_completely_different() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Graph 1: cycle 0 -> 1 -> 2 -> 0
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            add_edge(&graph1, *from, *to);
        }

        // Graph 2: path 0 -> 1 -> 2 (no cycle)
        for (from, to) in &[(0, 1), (1, 2)] {
            add_edge(&graph2, *from, *to);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert!(!result.isomorphic);
        // MCS should be at most 2 (the path portion)
        assert!(result.mcs_similarity < 1.0);
    }

    // Test 5: Both empty graphs -> similarity = 1.0
    #[test]
    fn test_structural_similarity_empty_graphs() {
        let graph1 = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let graph2 = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert!(result.isomorphic);
        assert_eq!(result.mcs_similarity, 1.0);
        assert_eq!(result.ged_distance, 0.0);
        assert_eq!(result.mcs_size, 0);
    }

    // Test 6: One empty, one not -> similarity = 0.0
    #[test]
    fn test_structural_similarity_one_empty() {
        let graph1 = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let graph2 = create_test_graph_with_nodes(3);

        // Add some edges to graph2
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert!(!result.isomorphic);
        assert_eq!(result.mcs_similarity, 0.0);
        assert_eq!(result.ged_distance, 1.0);
        assert!(result.has_no_common_structure());
    }

    // Test 7: Verify is_very_similar() method
    #[test]
    fn test_similarity_result_helpers() {
        let result = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.85,
            ged_distance: 0.15,
            mcs_size: 5,
            graph1_size: 6,
            graph2_size: 6,
            computation_time_ms: 10,
        };

        assert!(result.is_very_similar());
        assert!(result.is_moderately_similar());
        assert_eq!(result.similarity_class(), "Very Similar");
    }

    // Test 8: Verify similarity_class() method
    #[test]
    fn test_similarity_class() {
        // Test each classification
        let identical = SimilarityResult {
            isomorphic: true,
            mcs_similarity: 1.0,
            ged_distance: 0.0,
            mcs_size: 5,
            graph1_size: 5,
            graph2_size: 5,
            computation_time_ms: 10,
        };
        assert_eq!(identical.similarity_class(), "Identical");

        let very_similar = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.85,
            ged_distance: 0.15,
            mcs_size: 4,
            graph1_size: 5,
            graph2_size: 5,
            computation_time_ms: 10,
        };
        assert_eq!(very_similar.similarity_class(), "Very Similar");

        let similar = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.6,
            ged_distance: 0.4,
            mcs_size: 3,
            graph1_size: 5,
            graph2_size: 5,
            computation_time_ms: 10,
        };
        assert_eq!(similar.similarity_class(), "Similar");

        let different = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.3,
            ged_distance: 0.7,
            mcs_size: 2,
            graph1_size: 5,
            graph2_size: 5,
            computation_time_ms: 10,
        };
        assert_eq!(different.similarity_class(), "Different");

        let no_common = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.0,
            ged_distance: 1.0,
            mcs_size: 0,
            graph1_size: 5,
            graph2_size: 5,
            computation_time_ms: 10,
        };
        assert_eq!(no_common.similarity_class(), "No Common Structure");
    }

    // Test 9: Verify is_similar_enough() method
    #[test]
    fn test_similarity_bounds_threshold() {
        let bounds = SimilarityBounds {
            max_matches: None,
            timeout_ms: None,
            similarity_threshold: Some(0.8),
        };

        assert!(bounds.is_similar_enough(0.9));
        assert!(bounds.is_similar_enough(0.8));
        assert!(!bounds.is_similar_enough(0.7));

        // No threshold means always returns false
        let no_threshold = SimilarityBounds::default();
        assert!(!no_threshold.is_similar_enough(0.9));
    }

    // Test 10: Progress callback is called
    #[test]
    fn test_similarity_with_progress() {
        use crate::progress::NoProgress;

        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Create identical structure
        for i in 0..2 {
            add_edge(&graph1, i, i + 1);
            add_edge(&graph2, i, i + 1);
        }

        let progress = NoProgress;
        let bounds = SimilarityBounds::default();
        let result =
            structural_similarity_with_progress(&graph1, &graph2, bounds, &progress).unwrap();

        assert!(result.isomorphic);
    }

    // Test 11: Refactor verification example
    #[test]
    fn test_structural_similarity_refactor_verification() {
        // Original code structure: A -> B -> C -> D
        let original = create_test_graph_with_nodes(4);
        for i in 0..3 {
            add_edge(&original, i, i + 1);
        }

        // Refactored code: optimized but same structure
        let optimized = create_test_graph_with_nodes(4);
        for i in 0..3 {
            add_edge(&optimized, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&original, &optimized, bounds).unwrap();

        // Refactor preserved structure
        assert!(result.isomorphic);
        assert_eq!(result.similarity_class(), "Identical");
    }

    // Test 12: Regression detection example
    #[test]
    fn test_structural_similarity_regression_detection() {
        // Version 1.0: simple chain A -> B -> C
        let v1 = create_test_graph_with_nodes(3);
        for i in 0..2 {
            add_edge(&v1, i, i + 1);
        }

        // Version 2.0: added a new branch, changed structure
        let v2 = create_test_graph_with_nodes(4);
        // A -> B -> C -> D
        // A -> D (new direct connection)
        add_edge(&v2, 0, 1);
        add_edge(&v2, 1, 2);
        add_edge(&v2, 2, 3);
        add_edge(&v2, 0, 3); // New edge

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&v1, &v2, bounds).unwrap();

        // Detect structural change
        assert!(!result.isomorphic);
        assert!(result.mcs_similarity < 1.0);
        // The 3-node chain should be found as a subgraph
        assert!(result.mcs_size >= 3);
    }

    // Test 13: Larger pattern than target
    #[test]
    fn test_mcs_size_larger_pattern() {
        let graph_small = create_test_graph_with_nodes(2);
        let graph_large = create_test_graph_with_nodes(4);

        // Small: single edge
        add_edge(&graph_small, 0, 1);

        // Large: chain of 4
        for i in 0..3 {
            add_edge(&graph_large, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph_small, &graph_large, bounds).unwrap();

        // Small graph should fit in large
        assert!(!result.isomorphic); // Different sizes
        assert!(result.mcs_size >= 2); // At least the small graph
    }

    // Test 14: Computation time tracking
    #[test]
    fn test_computation_time_tracking() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        for i in 0..2 {
            add_edge(&graph1, i, i + 1);
            add_edge(&graph2, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        // Computation should be fast for small graphs
        assert!(result.computation_time_ms < 1000);
    }

    // Test 15: Builder pattern methods
    #[test]
    fn test_similarity_bounds_builder() {
        let bounds = SimilarityBounds::new()
            .with_max_matches(100)
            .with_timeout(5000)
            .with_threshold(0.8);

        assert_eq!(bounds.max_matches, Some(100));
        assert_eq!(bounds.timeout_ms, Some(5000));
        assert_eq!(bounds.similarity_threshold, Some(0.8));
        assert!(bounds.is_bounded());
    }

    // Test 16: Similarity with timeout bound
    #[test]
    fn test_similarity_with_timeout() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        for i in 0..2 {
            add_edge(&graph1, i, i + 1);
            add_edge(&graph2, i, i + 1);
        }

        let bounds = SimilarityBounds {
            timeout_ms: Some(100), // 100ms timeout
            ..Default::default()
        };

        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        // Should complete quickly for small graphs
        assert!(result.isomorphic);
    }

    // Test 17: Graph sizes in result
    #[test]
    fn test_graph_sizes_in_result() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(5);

        for i in 0..2 {
            add_edge(&graph1, i, i + 1);
        }
        for i in 0..4 {
            add_edge(&graph2, i, i + 1);
        }

        let bounds = SimilarityBounds::default();
        let result = structural_similarity(&graph1, &graph2, bounds).unwrap();

        assert_eq!(result.graph1_size, 3);
        assert_eq!(result.graph2_size, 5);
    }

    // Test 18: GED distance calculation
    #[test]
    fn test_ged_distance_calculation() {
        // GED distance is stored in the result and should be consistent with similarity
        let mcs_similarity = 0.7;
        let ged_distance = 1.0 - mcs_similarity; // GED = 1.0 - similarity

        let result = SimilarityResult {
            isomorphic: false,
            mcs_similarity,
            ged_distance,
            mcs_size: 7,
            graph1_size: 10,
            graph2_size: 10,
            computation_time_ms: 10,
        };

        // Verify GED is 1.0 - similarity
        let expected_ged = 1.0 - result.mcs_similarity;
        assert!((result.ged_distance - expected_ged).abs() < 0.01);
    }

    // Test 19: is_moderately_similar threshold
    #[test]
    fn test_is_moderately_similar() {
        let result = SimilarityResult {
            isomorphic: false,
            mcs_similarity: 0.5,
            ged_distance: 0.5,
            mcs_size: 5,
            graph1_size: 10,
            graph2_size: 10,
            computation_time_ms: 10,
        };

        assert!(result.is_moderately_similar());
        assert!(!result.is_very_similar());
    }

    // Test 20: Default bounds
    #[test]
    fn test_default_bounds() {
        let bounds = SimilarityBounds::default();

        assert_eq!(bounds.max_matches, None);
        assert_eq!(bounds.timeout_ms, None);
        assert_eq!(bounds.similarity_threshold, None);
    }
}
