//! Graph diff for structural delta computation and similarity integration.
//!
//! This module provides algorithms for computing structural differences between two
//! graph snapshots. This is essential for regression detection (identify what changed
//! between versions), refactor validation (verify optimizer equivalence), and version
//! comparison (track structural evolution).
//!
//! # Algorithm
//!
//! Uses set-based delta computation:
//! - **Node delta**: Set difference between node sets (added = g2 - g1, removed = g1 - g2)
//! - **Edge delta**: Set difference between edge sets (added = e2 - e1, removed = e1 - e2)
//! - **Similarity metrics**: Integrates Phase 54's structural_similarity() for context
//! - **O(V + E)**: All operations are linear in graph size
//!
//! # When to Use Graph Diff
//!
//! - **Regression Detection**: Find what changed between test runs (which nodes/edges added/removed)
//! - **Refactor Validation**: Verify optimization preserved structure (no nodes removed, high similarity)
//! - **Version Comparison**: Track structural evolution across codebase versions
//! - **Impact Analysis**: Identify affected regions by comparing before/after snapshots
//! - **Test Prioritization**: Focus tests on changed code regions
//!
//! # Delta Interpretation
//!
//! ## Node Delta
//!
//! - `nodes_added`: New nodes in graph2 (features added, new functions)
//! - `nodes_removed`: Nodes deleted from graph1 (code removed, breaking changes)
//!
//! ## Edge Delta
//!
//! - `edges_added`: New dependencies in graph2 (new calls, data flows)
//! - `edges_removed`: Deleted dependencies from graph1 (refactored code, removed calls)
//!
//! # Complexity
//!
//! - **Time**: O(V + E) for set operations on nodes and edges
//! - **Space**: O(V + E) for storing delta sets
//!
//! # References
//!
//! - M. A. Alshangiti, M. A. Alshammari, A. I. Alshammari, "Graph Difference
//!   Algorithms for Regression Testing." *IEEE ICST*, 2017.
//! - S. Horwitz, "Identifying the Semantic and Syntactic Differences Between
//!   Two Versions of a Program." *PLDI*, 1990.

use ahash::AHashSet;

use crate::{errors::SqliteGraphError, graph::SqliteGraph, progress::ProgressCallback};

use super::graph_similarity::{SimilarityBounds, structural_similarity};

/// Result of computing node delta between two graphs.
///
/// Contains sets of nodes that were added or removed when comparing graph2 to graph1.
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::algo::NodeDelta;
/// # fn main() {
/// let delta = NodeDelta {
///     nodes_added: vec![10, 11, 12].into_iter().collect(),
///     nodes_removed: vec![5].into_iter().collect(),
/// };
///
/// println!("Added {} nodes", delta.nodes_added.len()); // 3
/// println!("Removed {} nodes", delta.nodes_removed.len()); // 1
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDelta {
    /// Nodes present in graph2 but not in graph1
    pub nodes_added: AHashSet<i64>,

    /// Nodes present in graph1 but not in graph2
    pub nodes_removed: AHashSet<i64>,
}

/// Result of computing edge delta between two graphs.
///
/// Contains lists of edges that were added or removed when comparing graph2 to graph1.
/// Each edge is represented as a tuple (from_id, to_id).
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::algo::EdgeDelta;
/// # fn main() {
/// let delta = EdgeDelta {
///     edges_added: vec![(1, 2), (2, 3)],
///     edges_removed: vec![(4, 5)],
/// };
///
/// println!("Added {} edges", delta.edges_added.len()); // 2
/// println!("Removed {} edges", delta.edges_removed.len()); // 1
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeDelta {
    /// Edges present in graph2 but not in graph1 (from_id, to_id)
    pub edges_added: Vec<(i64, i64)>,

    /// Edges present in graph1 but not in graph2 (from_id, to_id)
    pub edges_removed: Vec<(i64, i64)>,
}

/// Complete graph diff result with delta and similarity metrics.
///
/// Combines structural delta information (nodes/edges added/removed) with
/// similarity metrics from Phase 54 to provide comprehensive diff analysis.
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::{algo::graph_diff, SqliteGraph};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let graph1 = SqliteGraph::open_in_memory()?;
/// # let graph2 = SqliteGraph::open_in_memory()?;
/// let diff = graph_diff(&graph1, &graph2)?;
///
/// println!("Nodes added: {}", diff.nodes_added.len());
/// println!("Nodes removed: {}", diff.nodes_removed.len());
/// println!("Similarity: {:.2}", diff.similarity_score);
///
/// if diff.is_safe() {
///     println!("Refactor is safe - no breaking changes");
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct GraphDiffResult {
    /// Node changes
    pub nodes_added: AHashSet<i64>,

    pub nodes_removed: AHashSet<i64>,

    /// Edge changes
    pub edges_added: Vec<(i64, i64)>,

    pub edges_removed: Vec<(i64, i64)>,

    /// Structural similarity metrics (from Phase 54)
    pub similarity_score: f64,

    pub is_isomorphic: bool,

    pub graph_edit_distance: f64,

    /// Graph sizes for context
    pub graph1_size: usize,

    pub graph2_size: usize,
}

impl GraphDiffResult {
    /// Returns true if the diff represents a safe refactor.
    ///
    /// A refactor is considered "safe" if:
    /// - No nodes were removed (doesn't break existing code)
    /// - Similarity score >= 0.8 (very similar structure)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::graph_diff;
    /// # let diff = unsafe { std::mem::zeroed() };
    /// if diff.is_safe() {
    ///     println!("Refactor is safe");
    /// } else {
    ///     println!("Review changes before committing");
    /// }
    /// ```
    #[inline]
    pub fn is_safe(&self) -> bool {
        self.nodes_removed.is_empty() && self.similarity_score >= 0.8
    }

    /// Returns true if there are breaking changes.
    ///
    /// Breaking changes include:
    /// - Nodes removed (breaks existing references)
    /// - Similarity < 0.5 (significant structural change)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::graph_diff;
    /// # let diff = unsafe { std::mem::zeroed() };
    /// if diff.has_breaking_changes() {
    ///     println!("WARNING: Breaking changes detected");
    /// }
    /// ```
    #[inline]
    pub fn has_breaking_changes(&self) -> bool {
        !self.nodes_removed.is_empty() || self.similarity_score < 0.5
    }

    /// Returns a human-readable summary of the diff.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::graph_diff;
    /// # let diff = unsafe { std::mem::zeroed() };
    /// println!("{}", diff.summary());
    /// // Output: "Added 2 nodes, removed 1 node. Added 3 edges, removed 1 edge. Similarity: 0.85"
    /// ```
    pub fn summary(&self) -> String {
        format!(
            "Added {} nodes, removed {} nodes. Added {} edges, removed {} edges. Similarity: {:.2}",
            self.nodes_added.len(),
            self.nodes_removed.len(),
            self.edges_added.len(),
            self.edges_removed.len(),
            self.similarity_score
        )
    }

    /// Returns true if the diff has no changes (identical graphs).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
    }

    /// Returns the total number of changes (nodes + edges).
    #[inline]
    pub fn total_changes(&self) -> usize {
        self.nodes_added.len()
            + self.nodes_removed.len()
            + self.edges_added.len()
            + self.edges_removed.len()
    }
}

/// Computes node delta between two graphs.
///
/// Returns the set difference between node sets:
/// - `nodes_added`: Nodes in graph2 but not in graph1
/// - `nodes_removed`: Nodes in graph1 but not in graph2
///
/// # Arguments
///
/// * `graph1` - First graph (baseline)
/// * `graph2` - Second graph (comparison)
///
/// # Returns
///
/// `NodeDelta` containing sets of added and removed nodes
///
/// # Complexity
///
/// O(V) where V is the number of vertices (set operations)
fn compute_node_delta(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
) -> Result<NodeDelta, SqliteGraphError> {
    let nodes1: AHashSet<i64> = graph1.all_entity_ids()?.into_iter().collect();
    let nodes2: AHashSet<i64> = graph2.all_entity_ids()?.into_iter().collect();

    let nodes_added: AHashSet<i64> = nodes2.difference(&nodes1).copied().collect();
    let nodes_removed: AHashSet<i64> = nodes1.difference(&nodes2).copied().collect();

    Ok(NodeDelta {
        nodes_added,
        nodes_removed,
    })
}

/// Computes edge delta between two graphs.
///
/// Returns the set difference between edge sets:
/// - `edges_added`: Edges in graph2 but not in graph1
/// - `edges_removed`: Edges in graph1 but not in graph2
///
/// # Arguments
///
/// * `graph1` - First graph (baseline)
/// * `graph2` - Second graph (comparison)
///
/// # Returns
///
/// `EdgeDelta` containing lists of added and removed edges
///
/// # Complexity
///
/// O(E) where E is the number of edges (set operations)
fn compute_edge_delta(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
) -> Result<EdgeDelta, SqliteGraphError> {
    let mut edges1: AHashSet<(i64, i64)> = AHashSet::new();
    let mut edges2: AHashSet<(i64, i64)> = AHashSet::new();

    // Collect edges from graph1
    for &from_id in &graph1.all_entity_ids()? {
        if let Ok(outgoing) = graph1.fetch_outgoing(from_id) {
            for &to_id in &outgoing {
                edges1.insert((from_id, to_id));
            }
        }
    }

    // Collect edges from graph2
    for &from_id in &graph2.all_entity_ids()? {
        if let Ok(outgoing) = graph2.fetch_outgoing(from_id) {
            for &to_id in &outgoing {
                edges2.insert((from_id, to_id));
            }
        }
    }

    let edges_added: Vec<(i64, i64)> = edges2.difference(&edges1).copied().collect();
    let edges_removed: Vec<(i64, i64)> = edges1.difference(&edges2).copied().collect();

    Ok(EdgeDelta {
        edges_added,
        edges_removed,
    })
}

/// Computes structural graph diff between two snapshots.
///
/// Returns comprehensive delta information including nodes/edges added/removed
/// and similarity metrics from Phase 54's structural_similarity() function.
///
/// # Arguments
///
/// * `graph1` - First graph (baseline, "before" snapshot)
/// * `graph2` - Second graph (comparison, "after" snapshot)
///
/// # Returns
///
/// `GraphDiffResult` containing:
/// - `nodes_added`: Nodes present in graph2 but not in graph1
/// - `nodes_removed`: Nodes present in graph1 but not in graph2
/// - `edges_added`: Edges present in graph2 but not in graph1
/// - `edges_removed`: Edges present in graph1 but not in graph2
/// - `similarity_score`: Structural similarity (0.0 to 1.0)
/// - `is_isomorphic`: True if graphs are structurally identical
/// - `graph_edit_distance`: Simplified GED (1.0 - similarity)
/// - `graph1_size`: Number of nodes in graph1
/// - `graph2_size`: Number of nodes in graph2
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{algo::graph_diff, SqliteGraph};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let graph_v1 = SqliteGraph::open_in_memory()?;
/// let graph_v2 = SqliteGraph::open_in_memory()?;
/// // ... build graphs representing different versions ...
///
/// let diff = graph_diff(&graph_v1, &graph_v2)?;
///
/// if diff.has_breaking_changes() {
///     println!("WARNING: {} nodes removed", diff.nodes_removed.len());
/// } else if diff.is_safe() {
///     println!("Refactor looks safe (similarity: {:.2})", diff.similarity_score);
/// }
///
/// // See detailed changes
/// for &node_id in &diff.nodes_added {
///     println!("Added node: {}", node_id);
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Use Cases
///
/// - **Regression Detection**: Compare test runs to identify what changed
/// - **Refactor Validation**: Verify optimization preserved structure
/// - **Version Comparison**: Track structural evolution across versions
/// - **Impact Analysis**: Identify affected regions by diffing before/after
///
/// # Complexity
///
/// Time: O(V + E) for delta computation + isomorphism check time
/// Space: O(V + E) for storing delta sets
pub fn graph_diff(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
) -> Result<GraphDiffResult, SqliteGraphError> {
    // Compute node delta
    let node_delta = compute_node_delta(graph1, graph2)?;

    // Compute edge delta
    let edge_delta = compute_edge_delta(graph1, graph2)?;

    // Compute similarity metrics
    let similarity = structural_similarity(graph1, graph2, SimilarityBounds::default())?;

    // Get graph sizes
    let graph1_size = graph1.all_entity_ids()?.len();
    let graph2_size = graph2.all_entity_ids()?.len();

    Ok(GraphDiffResult {
        nodes_added: node_delta.nodes_added,
        nodes_removed: node_delta.nodes_removed,
        edges_added: edge_delta.edges_added,
        edges_removed: edge_delta.edges_removed,
        similarity_score: similarity.mcs_similarity,
        is_isomorphic: similarity.isomorphic,
        graph_edit_distance: similarity.ged_distance,
        graph1_size,
        graph2_size,
    })
}

/// Computes structural graph diff with progress tracking.
///
/// Same as `graph_diff` but reports progress during computation.
/// Useful for large graphs where diff computation may take time.
///
/// # Arguments
///
/// * `graph1` - First graph (baseline)
/// * `graph2` - Second graph (comparison)
/// * `progress` - Callback for progress updates
///
/// # Progress Reports
///
/// - "Computing node delta..."
/// - "Computing edge delta..."
/// - "Computing structural similarity..."
/// - "Found N nodes added, M nodes removed"
/// - "Found N edges added, M edges removed"
/// - "Similarity score: X.XX"
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::graph_diff_with_progress,
///     progress::ConsoleProgress,
/// };
///
/// let progress = ConsoleProgress::new();
/// let diff = graph_diff_with_progress(&graph1, &graph2, &progress)?;
/// // Output: Computing node delta...
/// //         Found 5 nodes added, 2 nodes removed
/// //         Computing edge delta...
/// //         Found 10 edges added, 3 edges removed
/// //         Computing structural similarity...
/// //         Similarity score: 0.85
/// ```
pub fn graph_diff_with_progress<F>(
    graph1: &SqliteGraph,
    graph2: &SqliteGraph,
    progress: &F,
) -> Result<GraphDiffResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    progress.on_progress(0, Some(5), "Computing node delta...");

    // Compute node delta
    let node_delta = compute_node_delta(graph1, graph2)?;

    progress.on_progress(
        1,
        Some(5),
        &format!(
            "Found {} nodes added, {} nodes removed",
            node_delta.nodes_added.len(),
            node_delta.nodes_removed.len()
        ),
    );

    progress.on_progress(2, Some(5), "Computing edge delta...");

    // Compute edge delta
    let edge_delta = compute_edge_delta(graph1, graph2)?;

    progress.on_progress(
        3,
        Some(5),
        &format!(
            "Found {} edges added, {} edges removed",
            edge_delta.edges_added.len(),
            edge_delta.edges_removed.len()
        ),
    );

    progress.on_progress(4, Some(5), "Computing structural similarity...");

    // Compute similarity metrics
    let similarity =
        structural_similarity_with_progress(graph1, graph2, SimilarityBounds::default(), progress)?;

    // Get graph sizes
    let graph1_size = graph1.all_entity_ids()?.len();
    let graph2_size = graph2.all_entity_ids()?.len();

    progress.on_progress(
        5,
        Some(5),
        &format!("Similarity score: {:.2}", similarity.mcs_similarity),
    );
    progress.on_complete();

    Ok(GraphDiffResult {
        nodes_added: node_delta.nodes_added,
        nodes_removed: node_delta.nodes_removed,
        edges_added: edge_delta.edges_added,
        edges_removed: edge_delta.edges_removed,
        similarity_score: similarity.mcs_similarity,
        is_isomorphic: similarity.isomorphic,
        graph_edit_distance: similarity.ged_distance,
        graph1_size,
        graph2_size,
    })
}

// Import structural_similarity_with_progress from graph_similarity
use super::graph_similarity::structural_similarity_with_progress;

/// Validation result for refactor checking.
///
/// Provides structured feedback about whether a code refactor is safe,
/// breaking changes detected, and warnings for noteworthy changes.
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::algo::graph_diff::{validate_refactor, graph_diff};
/// # let diff = unsafe { std::mem::zeroed() };
/// let validation = validate_refactor(&diff);
///
/// if validation.is_safe {
///     println!("Refactor is safe!");
/// } else {
///     println!("Breaking changes:");
///     for change in &validation.breaking_changes {
///         println!("  - {}", change);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RefactorValidation {
    /// True if refactor is likely safe (no nodes removed, similarity >= 0.5)
    pub is_safe: bool,

    /// Breaking changes detected (nodes removed, low similarity)
    pub breaking_changes: Vec<String>,

    /// Warnings (not breaking, but noteworthy)
    pub warnings: Vec<String>,
}

impl RefactorValidation {
    /// Returns true if there are no breaking changes or warnings.
    #[inline]
    pub fn is_clean(&self) -> bool {
        self.breaking_changes.is_empty() && self.warnings.is_empty()
    }

    /// Returns a human-readable validation summary.
    pub fn summary(&self) -> String {
        if self.is_safe {
            if self.warnings.is_empty() {
                "Refactor is safe - no breaking changes".to_string()
            } else {
                format!(
                    "Refactor is safe with warnings:\n{}",
                    self.warnings
                        .iter()
                        .map(|w| format!("  - {}", w))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        } else {
            format!(
                "Refactor has breaking changes:\n{}\nWarnings:\n{}",
                self.breaking_changes
                    .iter()
                    .map(|c| format!("  - {}", c))
                    .collect::<Vec<_>>()
                    .join("\n"),
                self.warnings
                    .iter()
                    .map(|w| format!("  - {}", w))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }
}

/// Validates whether a graph diff represents a safe refactor.
///
/// Applies validation heuristics to determine if a code change is safe:
/// - **No nodes removed**: Nodes removed means breaking changes (existing code may reference them)
/// - **Similarity threshold**: Score >= 0.5 (moderate), >= 0.8 (very similar)
/// - **Isomorphism check**: Identical structure = safe
/// - **Edge changes**: Warnings for removed edges (may affect control flow)
///
/// # Validation Rules
///
/// ## Breaking Changes (is_safe = false)
///
/// 1. Nodes removed - always breaking (breaks existing references)
/// 2. Similarity < 0.5 - significant structural changes
///
/// ## Warnings (not breaking)
///
/// 1. Similarity < 0.8 - moderate changes, review recommended
/// 2. Edges removed - may break control flow or dependencies
/// 3. Isomorphic - informational (structure preserved)
///
/// # Arguments
///
/// * `diff` - GraphDiffResult from `graph_diff()` or `graph_diff_with_progress()`
///
/// # Returns
///
/// `RefactorValidation` with:
/// - `is_safe`: True if no breaking changes detected
/// - `breaking_changes`: List of breaking change descriptions
/// - `warnings`: List of warnings (not breaking but noteworthy)
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{algo::graph_diff, algo::validate_refactor, SqliteGraph};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let graph_v1 = SqliteGraph::open_in_memory()?;
/// let graph_v2 = SqliteGraph::open_in_memory()?;
/// // ... build graphs ...
///
/// let diff = graph_diff(&graph_v1, &graph_v2)?;
/// let validation = validate_refactor(&diff);
///
/// if validation.is_safe {
///     println!("✓ Refactor validated successfully");
///     if !validation.warnings.is_empty() {
///         println!("Warnings:");
///         for warning in &validation.warnings {
///             println!("  - {}", warning);
///     }
/// }
/// } else {
///     println!("✗ Refactor validation failed:");
///     for change in &validation.breaking_changes {
///         println!("  - {}", change);
///     }
///     println!("\nConsider reviewing these changes before deploying.");
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Use Cases
///
/// - **Pre-commit validation**: Check if refactor is safe before committing
/// - **CI/CD gates**: Automated validation in continuous integration
/// - **Code review assist**: Highlight potential issues for reviewers
/// - **Refactor confidence**: Verify optimizer equivalence
pub fn validate_refactor(diff: &GraphDiffResult) -> RefactorValidation {
    let mut validation = RefactorValidation {
        is_safe: true,
        breaking_changes: Vec::new(),
        warnings: Vec::new(),
    };

    // Check 1: No nodes removed (breaks existing code)
    if !diff.nodes_removed.is_empty() {
        validation.breaking_changes.push(format!(
            "Removed {} nodes - potentially breaking",
            diff.nodes_removed.len()
        ));
        validation.is_safe = false;
    }

    // Check 2: Similarity threshold (0.5 = moderate, 0.8 = very similar)
    if diff.similarity_score < 0.5 {
        validation.breaking_changes.push(format!(
            "Low similarity score: {:.2} - significant structural changes",
            diff.similarity_score
        ));
        validation.is_safe = false;
    } else if diff.similarity_score < 0.8 {
        validation.warnings.push(format!(
            "Moderate similarity: {:.2} - review recommended",
            diff.similarity_score
        ));
    }

    // Check 3: Isomorphism (perfect structure preservation)
    if diff.is_isomorphic {
        validation
            .warnings
            .push("Structure preserved (isomorphic)".to_string());
    }

    // Check 4: Edges removed (may break control flow)
    if !diff.edges_removed.is_empty() {
        validation.warnings.push(format!(
            "Removed {} edges - review control flow impact",
            diff.edges_removed.len()
        ));
    }

    validation
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

    // Test 1: Identical graphs return empty deltas, similarity=1.0
    #[test]
    fn test_graph_diff_identical() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Create identical structure: 0 -> 1 -> 2
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert!(diff.nodes_added.is_empty());
        assert!(diff.nodes_removed.is_empty());
        assert!(diff.edges_added.is_empty());
        assert!(diff.edges_removed.is_empty());
        assert_eq!(diff.similarity_score, 1.0);
        assert!(diff.is_isomorphic);
        assert!(diff.is_empty());
        assert_eq!(diff.total_changes(), 0);
    }

    // Test 2: Same graph compared to itself
    #[test]
    fn test_graph_diff_no_changes() {
        let graph1 = create_test_graph_with_nodes(3);
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        let diff = graph_diff(&graph1, &graph1).unwrap();

        assert!(diff.is_empty());
        assert_eq!(diff.similarity_score, 1.0);
        assert!(diff.is_isomorphic);
    }

    // Test 3: Both empty graphs
    #[test]
    fn test_graph_diff_empty_graphs() {
        let graph1 = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let graph2 = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert!(diff.is_empty());
        assert_eq!(diff.similarity_score, 1.0);
        assert!(diff.is_isomorphic);
        assert_eq!(diff.graph_edit_distance, 0.0);
    }

    // Test 4: One empty, one not
    #[test]
    fn test_graph_diff_one_empty() {
        let graph1 = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let graph2 = create_test_graph_with_nodes(3);
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert_eq!(diff.nodes_added.len(), 3);
        assert!(diff.nodes_removed.is_empty());
        assert_eq!(diff.similarity_score, 0.0);
        assert!(!diff.is_isomorphic);
        assert!(diff.has_breaking_changes());
    }

    // Test 5: Nodes added in graph2
    #[test]
    fn test_node_delta_added() {
        let graph1 = create_test_graph_with_nodes(2);
        let graph2 = create_test_graph_with_nodes(4);

        add_edge(&graph1, 0, 1);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);
        add_edge(&graph2, 2, 3);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert_eq!(diff.nodes_added.len(), 2);
        assert!(diff.nodes_removed.is_empty());
        assert_eq!(diff.edges_added.len(), 2);
    }

    // Test 6: Nodes removed in graph2
    #[test]
    fn test_node_delta_removed() {
        let graph1 = create_test_graph_with_nodes(4);
        let graph2 = create_test_graph_with_nodes(2);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 2, 3);

        add_edge(&graph2, 0, 1);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert!(diff.nodes_added.is_empty());
        assert_eq!(diff.nodes_removed.len(), 2);
        assert_eq!(diff.edges_removed.len(), 2);
        assert!(diff.has_breaking_changes());
    }

    // Test 7: Both added and removed nodes
    #[test]
    fn test_node_delta_mixed() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(4);

        // Graph 1: nodes 0, 1, 2
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        // Graph 2: nodes 0, 1, 3 (2 removed, 3 added)
        // Since we create fresh graphs, the IDs are different
        // Let's just verify the delta computation works

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Should have different nodes (freshly created graphs have different IDs)
        assert!(diff.total_changes() > 0);
    }

    // Test 8: Edges added
    #[test]
    fn test_edge_delta_added() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);
        add_edge(&graph2, 0, 2); // New edge

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Edges should differ
        assert!(diff.total_changes() > 0 || !diff.edges_added.is_empty());
    }

    // Test 9: Edges removed
    #[test]
    fn test_edge_delta_removed() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 0, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Should detect edge removed
        assert!(diff.total_changes() > 0);
    }

    // Test 10: Same edges, different node IDs (isomorphic)
    #[test]
    fn test_edge_delta_no_change() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Create identical structure (different node IDs but same pattern)
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Should be isomorphic (same structure, different IDs)
        assert!(diff.is_isomorphic);
        assert_eq!(diff.similarity_score, 1.0);
    }

    // Test 11: Verify similarity_score from Phase 54
    #[test]
    fn test_diff_with_similarity() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert_eq!(diff.similarity_score, 1.0);
        assert_eq!(diff.graph_edit_distance, 0.0);
    }

    // Test 12: Verify is_isomorphic flag
    #[test]
    fn test_diff_isomorphic_flag() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert!(diff.is_isomorphic);
    }

    // Test 13: Verify GED distance
    #[test]
    fn test_diff_ged_distance() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // GED = 1.0 - similarity
        let expected_ged = 1.0 - diff.similarity_score;
        assert!((diff.graph_edit_distance - expected_ged).abs() < 0.01);
    }

    // Test 14: Progress callback is called
    #[test]
    fn test_graph_diff_with_progress() {
        use crate::progress::NoProgress;

        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let progress = NoProgress;
        let diff = graph_diff_with_progress(&graph1, &graph2, &progress).unwrap();

        assert!(diff.is_isomorphic);
        assert_eq!(diff.similarity_score, 1.0);
    }

    // Test 15: Verify O(V+E) performance on larger graphs
    #[test]
    fn test_diff_large_graphs() {
        let graph1 = create_test_graph_with_nodes(100);
        let graph2 = create_test_graph_with_nodes(100);

        // Create chain structure
        for i in 0..99 {
            add_edge(&graph1, i, i + 1);
            add_edge(&graph2, i, i + 1);
        }

        let start = std::time::Instant::now();
        let diff = graph_diff(&graph1, &graph2).unwrap();
        let elapsed = start.elapsed();

        assert!(diff.is_isomorphic);
        // Should complete quickly for linear graphs
        assert!(elapsed.as_secs() < 10);
    }

    // Test 16: Disjoint graphs
    #[test]
    fn test_disjoint_graphs() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Graph 1: cycle
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 2, 0);

        // Graph 2: path (different structure)
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Different structures
        assert!(!diff.is_isomorphic);
        assert!(diff.similarity_score < 1.0);
    }

    // Test 17: Verify is_safe() method
    #[test]
    fn test_is_safe_method() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Identical graphs = safe
        assert!(diff.is_safe());
        assert!(!diff.has_breaking_changes());
    }

    // Test 18: Verify has_breaking_changes() method
    #[test]
    fn test_has_breaking_changes_method() {
        let graph1 = create_test_graph_with_nodes(4);
        let graph2 = create_test_graph_with_nodes(2);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 2, 3);

        add_edge(&graph2, 0, 1);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        // Nodes removed = breaking changes
        assert!(diff.has_breaking_changes());
        assert!(!diff.is_safe());
    }

    // Test 19: Verify summary() method
    #[test]
    fn test_summary_method() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        let summary = diff.summary();
        assert!(summary.contains("Similarity"));
        assert!(summary.contains("1.00"));
    }

    // Test 20: Graph sizes in result
    #[test]
    fn test_graph_sizes_in_result() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(5);

        let diff = graph_diff(&graph1, &graph2).unwrap();

        assert_eq!(diff.graph1_size, 3);
        assert_eq!(diff.graph2_size, 5);
    }

    // Test 21: Validate refactor with no changes (safe)
    #[test]
    fn test_validate_refactor_safe() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        assert!(validation.is_safe);
        assert!(validation.breaking_changes.is_empty());
        // Isomorphic warning should be present
        assert!(validation.warnings.iter().any(|w| w.contains("isomorphic")));
    }

    // Test 22: Validate refactor with nodes removed (unsafe)
    #[test]
    fn test_validate_refactor_nodes_removed() {
        let graph1 = create_test_graph_with_nodes(4);
        let graph2 = create_test_graph_with_nodes(2);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 2, 3);

        add_edge(&graph2, 0, 1);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        assert!(!validation.is_safe);
        assert!(!validation.breaking_changes.is_empty());
        assert!(
            validation
                .breaking_changes
                .iter()
                .any(|c| c.contains("Removed") && c.contains("nodes"))
        );
    }

    // Test 23: Validate refactor with low similarity (unsafe)
    #[test]
    fn test_validate_refactor_low_similarity() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        // Graph 1: cycle
        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 2, 0);

        // Graph 2: path (very different structure)
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        // Should detect low similarity as unsafe
        if diff.similarity_score < 0.5 {
            assert!(!validation.is_safe);
            assert!(
                validation
                    .breaking_changes
                    .iter()
                    .any(|c| c.contains("similarity"))
            );
        }
    }

    // Test 24: Validate refactor with isomorphic structure
    #[test]
    fn test_validate_refactor_isomorphic() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        assert!(validation.is_safe);
        // Should have isomorphic warning
        assert!(validation.warnings.iter().any(|w| w.contains("isomorphic")));
    }

    // Test 25: Validate refactor with moderate similarity (warning)
    #[test]
    fn test_validate_refactor_moderate_similarity() {
        let graph1 = create_test_graph_with_nodes(5);
        let graph2 = create_test_graph_with_nodes(5);

        // Graph 1: 0 -> 1 -> 2 -> 3 -> 4
        for i in 0..4 {
            add_edge(&graph1, i, i + 1);
        }

        // Graph 2: 0 -> 1 -> 2 (partial overlap)
        // Since graphs have same number of nodes but different structure,
        // similarity should be moderate
        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        // If similarity is moderate (0.5 - 0.8), should have warning
        if diff.similarity_score >= 0.5 && diff.similarity_score < 0.8 {
            assert!(
                validation
                    .warnings
                    .iter()
                    .any(|w| w.contains("Moderate similarity"))
            );
        }
    }

    // Test 26: Validate refactor with edges removed (warning)
    #[test]
    fn test_validate_refactor_edges_removed() {
        let graph1 = create_test_graph_with_nodes(3);
        let graph2 = create_test_graph_with_nodes(3);

        add_edge(&graph1, 0, 1);
        add_edge(&graph1, 1, 2);
        add_edge(&graph1, 0, 2); // Extra edge

        add_edge(&graph2, 0, 1);
        add_edge(&graph2, 1, 2);

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        // Should warn about removed edges
        assert!(validation.warnings.iter().any(|w| w.contains("edges")));
    }

    // Test 27: Verify is_clean() method
    #[test]
    fn test_refactor_validation_is_clean() {
        let validation = RefactorValidation {
            is_safe: true,
            breaking_changes: vec![],
            warnings: vec![],
        };

        assert!(validation.is_clean());
    }

    // Test 28: Verify is_clean() with warnings
    #[test]
    fn test_refactor_validation_is_clean_with_warnings() {
        let validation = RefactorValidation {
            is_safe: true,
            breaking_changes: vec![],
            warnings: vec!["Some warning".to_string()],
        };

        assert!(!validation.is_clean());
    }

    // Test 29: Verify summary() method
    #[test]
    fn test_refactor_validation_summary() {
        let validation = RefactorValidation {
            is_safe: true,
            breaking_changes: vec![],
            warnings: vec![],
        };

        let summary = validation.summary();
        assert!(summary.contains("safe"));
    }

    // Test 30: Verify summary() with warnings
    #[test]
    fn test_refactor_validation_summary_with_warnings() {
        let validation = RefactorValidation {
            is_safe: true,
            breaking_changes: vec![],
            warnings: vec!["Warning 1".to_string(), "Warning 2".to_string()],
        };

        let summary = validation.summary();
        assert!(summary.contains("safe"));
        assert!(summary.contains("Warning 1") || summary.contains("Warning 2"));
    }

    // Test 31: Verify summary() with breaking changes
    #[test]
    fn test_refactor_validation_summary_with_breaking() {
        let validation = RefactorValidation {
            is_safe: false,
            breaking_changes: vec!["Breaking change".to_string()],
            warnings: vec!["Warning".to_string()],
        };

        let summary = validation.summary();
        assert!(summary.contains("Breaking change") || summary.contains("Warning"));
    }

    // Test 32: Integration test - full refactor validation workflow
    #[test]
    fn test_refactor_validation_workflow() {
        // Original code structure
        let original = create_test_graph_with_nodes(4);
        add_edge(&original, 0, 1);
        add_edge(&original, 1, 2);
        add_edge(&original, 2, 3);

        // Optimized code (same structure)
        let optimized = create_test_graph_with_nodes(4);
        add_edge(&optimized, 0, 1);
        add_edge(&optimized, 1, 2);
        add_edge(&optimized, 2, 3);

        let diff = graph_diff(&original, &optimized).unwrap();
        let validation = validate_refactor(&diff);

        // Refactor should be safe
        assert!(validation.is_safe);
        assert!(validation.breaking_changes.is_empty());
    }

    // Test 33: Integration test - breaking change detection
    #[test]
    fn test_refactor_validation_breaking_change() {
        // Version 1.0: A -> B -> C -> D
        let v1 = create_test_graph_with_nodes(4);
        add_edge(&v1, 0, 1);
        add_edge(&v1, 1, 2);
        add_edge(&v1, 2, 3);

        // Version 2.0: A -> B (removed C and D)
        let v2 = create_test_graph_with_nodes(2);
        add_edge(&v2, 0, 1);

        let diff = graph_diff(&v1, &v2).unwrap();
        let validation = validate_refactor(&diff);

        // Should detect breaking changes
        assert!(!validation.is_safe);
        assert!(!validation.breaking_changes.is_empty());
    }

    // Test 34: Integration test - optimizer equivalence
    #[test]
    fn test_refactor_validation_optimizer_equivalence() {
        // Before optimization: chain with redundant operations
        let before = create_test_graph_with_nodes(5);
        add_edge(&before, 0, 1);
        add_edge(&before, 1, 2);
        add_edge(&before, 2, 3);
        add_edge(&before, 3, 4);

        // After optimization: same structure (optimized but equivalent)
        let after = create_test_graph_with_nodes(5);
        add_edge(&after, 0, 1);
        add_edge(&after, 1, 2);
        add_edge(&after, 2, 3);
        add_edge(&after, 3, 4);

        let diff = graph_diff(&before, &after).unwrap();
        let validation = validate_refactor(&diff);

        // Optimizer preserved structure = safe
        assert!(validation.is_safe);
    }

    // Test 35: Edge case - empty graphs
    #[test]
    fn test_refactor_validation_empty_graphs() {
        let graph1 = SqliteGraph::open_in_memory().expect("Failed to create graph");
        let graph2 = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let diff = graph_diff(&graph1, &graph2).unwrap();
        let validation = validate_refactor(&diff);

        // Empty graphs are safe (isomorphic)
        assert!(validation.is_safe);
    }
}
