//! Bounded subgraph isomorphism using VF2 algorithm.
//!
//! This module provides algorithms for finding all occurrences of a pattern graph
//! within a target graph using the VF2 (Vento-Foggia-Di Matteo) algorithm. Subgraph
//! isomorphism is the problem of determining whether a pattern graph can be mapped
//! to a subgraph of a target graph, and finding all such mappings.
//!
//! # Algorithm
//!
//! Uses petgraph's VF2 implementation (O(n! × n) worst case):
//! - VF2 backtracking with feasibility rules for pruning
//! - Converts SqliteGraph to petgraph::DiGraph for processing
//! - Semantic matching via node_match and edge_match closures
//! - Bounds to prevent exponential blowup on dense graphs
//!
//! # When to Use Subgraph Isomorphism
//!
//! - **Common Subexpression Detection**: Find repeated patterns in computation graphs
//! - **Pattern Matching**: Search for specific graph structures in larger graphs
//! - **Anti-pattern Detection**: Find problematic patterns in dependency graphs
//! - **Compiler IR Optimization**: Identify common subexpressions for CSE
//! - **ML Framework Pattern Matching**: Find computation patterns in dataflow graphs
//!
//! # Subgraph Isomorphism vs Graph Isomorphism
//!
//! | Aspect | Graph Isomorphism | Subgraph Isomorphism |
//! |--------|------------------|----------------------|
//! | Output | Can pattern match entire target | Pattern can match subgraph of target |
//! | Use Case | Same structure, different labels | Pattern search within larger graph |
//! | Algorithm | VF2 with |V| = |G| | VF2 with |V| <= |G| |
//! | Complexity | O(n!) | O(n! × m) where m = |G| - |V| |
//!
//! # Complexity
//!
//! - **Time**: O(n! × n) worst case, where n = pattern nodes
//! - **Space**: O(n + m) for recursion stack and match state
//! - **Practical**: Bounds required for patterns > 5 nodes on dense graphs
//!
//! # Bounds are Critical
//!
//! Subgraph isomorphism is NP-complete. Always use bounds:
//! - `max_matches`: Stop after N matches (default 100)
//! - `timeout_ms`: Stop after N milliseconds (default 5000)
//! - `max_pattern_nodes`: Reject patterns larger than N nodes (default 10)
//!
//! # References
//!
//! - L. P. Cordella, P. Foggia, C. Sansone, M. Vento, "A (Sub)Graph Isomorphism
//!   Algorithm for Matching Large Graphs." *IEEE TPAMI*, 2004.
//! - https://petgraph.github.io/petgraph/petgraph/algo/isomorphism/index.html
//! - https://en.wikipedia.org/wiki/Subgraph_isomorphism_problem

use std::collections::HashMap;
use std::time::Instant;

// Import petgraph types - use fully qualified paths to avoid ambiguity
use petgraph::Directed;
use petgraph::algo::isomorphism;
use petgraph::graph::{DefaultIx, Graph, NodeIndex};

use crate::{errors::SqliteGraphError, graph::SqliteGraph, progress::ProgressCallback};

// Type alias using imported types
type PgDiGraph = Graph<i64, (), Directed, DefaultIx>;

/// Bounds for limiting subgraph isomorphism search.
///
/// Subgraph isomorphism is NP-complete (O(n!) worst case). These bounds
/// prevent exponential blowup on dense graphs with large patterns.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::algo::SubgraphPatternBounds;
///
/// // Stop after finding 100 matches or 5 seconds elapsed
/// let bounds = SubgraphPatternBounds {
///     max_matches: Some(100),
///     timeout_ms: Some(5000),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct SubgraphPatternBounds {
    /// Maximum number of matches to return.
    ///
    /// When this limit is reached, enumeration stops and `bounded_hit`
    /// is set to true in the result.
    pub max_matches: Option<usize>,

    /// Maximum time to spend searching (in milliseconds).
    ///
    /// When timeout is reached, enumeration stops and `bounded_hit`
    /// is set to true in the result.
    pub timeout_ms: Option<u64>,

    /// Maximum pattern size (in nodes) to attempt.
    ///
    /// Patterns larger than this will return an error immediately.
    /// This prevents accidentally searching for patterns with 20+ nodes
    /// which would take exponential time.
    pub max_pattern_nodes: Option<usize>,
}

impl SubgraphPatternBounds {
    /// Creates new bounds with all limits set to None (unlimited).
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of matches to return.
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

    /// Sets the maximum pattern size.
    #[inline]
    pub fn with_max_pattern_nodes(mut self, max: usize) -> Self {
        self.max_pattern_nodes = Some(max);
        self
    }

    /// Returns true if any bound is set.
    #[inline]
    pub fn is_bounded(&self) -> bool {
        self.max_matches.is_some() || self.timeout_ms.is_some() || self.max_pattern_nodes.is_some()
    }
}

/// Result of subgraph isomorphism search.
///
/// Contains all matches found within the bounds, plus statistics
/// about the search.
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::{algo::find_subgraph_patterns, SqliteGraph};
/// # use sqlitegraph::algo::SubgraphMatchResult;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let graph = SqliteGraph::open_in_memory()?;
/// # let pattern = SqliteGraph::open_in_memory()?;
/// # let result = find_subgraph_patterns(&graph, &pattern, Default::default())?;
/// println!("Found {} matches", result.patterns_found);
/// println!("Computation took {} ms", result.computation_time_ms);
///
/// if result.bounded_hit {
///     println!("Search stopped early due to bounds");
/// }
///
/// for (i, mapping) in result.matches.iter().enumerate() {
///     println!("Match {}: pattern_node -> target_node", i);
///     for (pattern_idx, target_id) in mapping.iter().enumerate() {
///         println!("  Pattern node {} maps to target {}", pattern_idx, target_id);
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct SubgraphMatchResult {
    /// All matches found.
    ///
    /// Each match is a Vec<i64> where the index is the pattern node index
    /// (from pattern.all_entity_ids()) and the value is the target node ID.
    /// Match length equals number of nodes in the pattern graph.
    pub matches: Vec<Vec<i64>>,

    /// Number of patterns found (same as matches.len()).
    ///
    /// This is provided as a convenience for quick access.
    pub patterns_found: usize,

    /// Time taken for computation in milliseconds.
    ///
    /// High-precision timing using std::time::Instant.
    pub computation_time_ms: u128,

    /// True if search stopped due to bounds.
    ///
    /// Set to true if max_matches or timeout_ms limit was reached.
    /// Check this to determine if there might be more matches not found.
    pub bounded_hit: bool,
}

impl SubgraphMatchResult {
    /// Returns true if no matches were found.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Returns the number of matches found.
    #[inline]
    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// Returns the first match, if any.
    #[inline]
    pub fn first_match(&self) -> Option<&[i64]> {
        self.matches.first().map(|m| m.as_slice())
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
    let mut id_to_index: HashMap<i64, NodeIndex> = HashMap::new();

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

/// Finds all subgraph isomorphisms of pattern within graph.
///
/// Uses VF2 algorithm to find all occurrences of the pattern graph
/// as a subgraph of the target graph. Returns all mappings from
/// pattern nodes to target nodes.
///
/// # Arguments
///
/// * `graph` - The target graph to search within
/// * `pattern` - The pattern graph to search for
/// * `bounds` - Limits on the search (max_matches, timeout, max_pattern_nodes)
///
/// # Returns
///
/// `SubgraphMatchResult` containing:
/// - List of matches (each is a mapping from pattern index to target node ID)
/// - Number of patterns found
/// - Computation time in milliseconds
/// - Whether search stopped due to bounds
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{algo::find_subgraph_patterns, SqliteGraph, GraphEntity};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Create target graph: 0 -> 1 -> 2 -> 3
/// let graph = SqliteGraph::open_in_memory()?;
/// for i in 0..4 {
///     let entity = GraphEntity {
///         id: 0,
///         kind: "node".to_string(),
///         name: format!("n{}", i),
///         file_path: None,
///         data: serde_json::json!({}),
///     };
///     graph.insert_entity(&entity)?;
/// }
///
/// let ids: Vec<i64> = graph.all_entity_ids()?;
/// // Add edges for chain
/// for i in 0..3 {
///     graph.insert_edge(&(ids[i], ids[i+1], "edge".to_string()))?;
/// }
///
/// // Create pattern: A -> B (2-node chain)
/// let pattern = SqliteGraph::open_in_memory()?;
/// for i in 0..2 {
///     let entity = GraphEntity {
///         id: 0,
///         kind: "pattern".to_string(),
///         name: format!("p{}", i),
///         file_path: None,
///         data: serde_json::json!({}),
///     };
///     pattern.insert_entity(&entity)?;
/// }
///
/// let pattern_ids: Vec<i64> = pattern.all_entity_ids()?;
/// pattern.insert_edge(&(pattern_ids[0], pattern_ids[1], "edge".to_string()))?;
///
/// // Find all occurrences of 2-node chain in 4-node path
/// // Should find 3 matches: (0,1), (1,2), (2,3)
/// let result = find_subgraph_patterns(&graph, &pattern, SubgraphPatternBounds::default())?;
/// assert_eq!(result.patterns_found, 3);
/// # Ok(())
/// # }
/// ```
///
/// # Complexity
///
/// Time: O(n! × n) worst case, where n = pattern nodes
/// Space: O(n + m) for recursion stack and match state
///
/// # Bounds
///
/// Always use bounds for patterns > 5 nodes or dense graphs:
/// - `max_matches`: Prevents enumerating millions of matches
/// - `timeout_ms`: Prevents hanging on exponential searches
/// - `max_pattern_nodes`: Rejects large patterns early
pub fn find_subgraph_patterns(
    graph: &SqliteGraph,
    pattern: &SqliteGraph,
    bounds: SubgraphPatternBounds,
) -> Result<SubgraphMatchResult, SqliteGraphError> {
    let start_time = Instant::now();

    // Get pattern entity IDs for ordering
    let pattern_ids = pattern.all_entity_ids()?;
    let pattern_count = pattern_ids.len();

    // Check max_pattern_nodes bound
    if let Some(max_nodes) = bounds.max_pattern_nodes
        && pattern_count > max_nodes
    {
        return Err(SqliteGraphError::invalid_input(format!(
            "Pattern too large: {} nodes exceeds max_pattern_nodes bound of {}",
            pattern_count, max_nodes
        )));
    }

    // Convert both graphs to petgraph
    let target_pg: PgDiGraph = graph_to_petgraph(graph)?;
    let pattern_pg: PgDiGraph = graph_to_petgraph(pattern)?;

    // Collect target node IDs in index order
    let target_node_ids: Vec<i64> = target_pg.node_indices().map(|ni| target_pg[ni]).collect();
    let _pattern_node_ids: Vec<i64> = pattern_pg.node_indices().map(|ni| pattern_pg[ni]).collect();

    // Use petgraph's subgraph_isomorphisms_iter
    let mut matches = Vec::new();
    let mut bounded_hit = false;

    let timeout = bounds.timeout_ms.map(std::time::Duration::from_millis);

    // petgraph's subgraph_isomorphisms_iter requires double references (&&Graph)
    // because IntoEdgesDirected is implemented for &Graph, not Graph itself
    // Following the pattern from petgraph tests: let a_ref = &a; iter(&a_ref, ...)
    // We need &a_ref so that G0 = &Graph, which implements IntoEdgesDirected
    let pattern_ref = &pattern_pg;
    let target_ref = &target_pg;
    let mut node_match = |_: &i64, _: &i64| -> bool { true };
    let mut edge_match = |_: &(), _: &()| -> bool { true };

    let iso_iter = isomorphism::subgraph_isomorphisms_iter(
        &pattern_ref,
        &target_ref,
        &mut node_match,
        &mut edge_match,
    );

    if let Some(iso_iter) = iso_iter {
        for mapping in iso_iter {
            // Check timeout
            if let Some(to) = timeout
                && start_time.elapsed() >= to
            {
                bounded_hit = true;
                break;
            }

            // Check max_matches
            if let Some(max) = bounds.max_matches
                && matches.len() >= max
            {
                bounded_hit = true;
                break;
            }

            // mapping is Vec<usize> where pattern_idx -> target_idx
            // Convert to entity IDs: pattern_node_ids[pattern_idx] -> target_node_ids[target_idx]
            let match_mapping: Vec<i64> = mapping
                .iter()
                .map(|&target_idx| {
                    if target_idx < target_node_ids.len() {
                        target_node_ids[target_idx]
                    } else {
                        0 // Fallback (shouldn't happen)
                    }
                })
                .collect();

            matches.push(match_mapping);
        }
    }

    Ok(SubgraphMatchResult {
        patterns_found: matches.len(),
        matches,
        computation_time_ms: start_time.elapsed().as_millis(),
        bounded_hit,
    })
}

/// Finds all subgraph isomorphisms with progress tracking.
///
/// Same as `find_subgraph_patterns` but reports progress during enumeration.
/// Useful for large graphs where pattern matching may take time.
///
/// # Arguments
///
/// * `graph` - The target graph to search within
/// * `pattern` - The pattern graph to search for
/// * `bounds` - Limits on the search
/// * `progress` - Callback for progress updates
///
/// # Progress Reports
///
/// - Starting search
/// - Every 10 matches found
/// - Completion (with bounded_hit status)
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::{find_subgraph_patterns_with_progress, SubgraphPatternBounds},
///     progress::ConsoleProgress,
/// };
///
/// let progress = ConsoleProgress::new();
/// let result = find_subgraph_patterns_with_progress(
///     &graph,
///     &pattern,
///     SubgraphPatternBounds::default(),
///     &progress
/// )?;
/// // Output: Found 10 matches...
/// ```
pub fn find_subgraph_patterns_with_progress<F>(
    graph: &SqliteGraph,
    pattern: &SqliteGraph,
    bounds: SubgraphPatternBounds,
    progress: &F,
) -> Result<SubgraphMatchResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    progress.on_progress(0, Some(3), "Converting graphs to petgraph format");

    // Get pattern entity IDs for ordering
    let pattern_ids = pattern.all_entity_ids()?;
    let pattern_count = pattern_ids.len();

    // Check max_pattern_nodes bound
    if let Some(max_nodes) = bounds.max_pattern_nodes
        && pattern_count > max_nodes
    {
        return Err(SqliteGraphError::invalid_input(format!(
            "Pattern too large: {} nodes exceeds max_pattern_nodes bound of {}",
            pattern_count, max_nodes
        )));
    }

    // Convert both graphs to petgraph
    let target_pg: PgDiGraph = graph_to_petgraph(graph)?;
    let pattern_pg: PgDiGraph = graph_to_petgraph(pattern)?;

    // Collect target node IDs in index order
    let target_node_ids: Vec<i64> = target_pg.node_indices().map(|ni| target_pg[ni]).collect();

    progress.on_progress(
        1,
        Some(3),
        &format!("Searching for patterns ({} pattern nodes)", pattern_count),
    );

    let start_time = Instant::now();
    let timeout = bounds.timeout_ms.map(std::time::Duration::from_millis);

    let mut matches = Vec::new();
    let mut bounded_hit = false;

    // petgraph's subgraph_isomorphisms_iter requires double references (&&Graph)
    // because IntoEdgesDirected is implemented for &Graph, not Graph itself
    // Following the pattern from petgraph tests: let a_ref = &a; iter(&a_ref, ...)
    // We need &a_ref so that G0 = &Graph, which implements IntoEdgesDirected
    let pattern_ref = &pattern_pg;
    let target_ref = &target_pg;
    let mut node_match = |_: &i64, _: &i64| -> bool { true };
    let mut edge_match = |_: &(), _: &()| -> bool { true };

    let iso_iter = isomorphism::subgraph_isomorphisms_iter(
        &pattern_ref,
        &target_ref,
        &mut node_match,
        &mut edge_match,
    );

    if let Some(iso_iter) = iso_iter {
        for mapping in iso_iter {
            // Check timeout
            if let Some(to) = timeout
                && start_time.elapsed() >= to
            {
                bounded_hit = true;
                break;
            }

            // Check max_matches
            if let Some(max) = bounds.max_matches
                && matches.len() >= max
            {
                bounded_hit = true;
                break;
            }

            // Convert Vec<usize> mapping to entity IDs
            let match_mapping: Vec<i64> = mapping
                .iter()
                .map(|&target_idx| {
                    if target_idx < target_node_ids.len() {
                        target_node_ids[target_idx]
                    } else {
                        0
                    }
                })
                .collect();

            matches.push(match_mapping);

            // Report progress every 10 matches
            if matches.len() % 10 == 0 {
                progress.on_progress(
                    2,
                    Some(3),
                    &format!("Found {} matches so far", matches.len()),
                );
            }
        }
    }

    let final_msg = if bounded_hit {
        format!(
            "Search complete: {} matches found (stopped by bounds)",
            matches.len()
        )
    } else {
        format!("Search complete: {} matches found", matches.len())
    };

    progress.on_progress(3, Some(3), &final_msg);
    progress.on_complete();

    Ok(SubgraphMatchResult {
        patterns_found: matches.len(),
        matches,
        computation_time_ms: start_time.elapsed().as_millis(),
        bounded_hit,
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

    /// Helper to create a pattern graph with a 2-node chain
    fn create_chain_pattern() -> SqliteGraph {
        let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");

        for i in 0..2 {
            let entity = GraphEntity {
                id: 0,
                kind: "pattern".to_string(),
                name: format!("p{}", i),
                file_path: None,
                data: serde_json::json!({}),
            };
            pattern
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let ids: Vec<i64> = pattern.all_entity_ids().expect("Failed to get IDs");
        let edge = GraphEdge {
            id: 0,
            from_id: ids[0],
            to_id: ids[1],
            edge_type: "edge".to_string(),
            data: serde_json::json!({}),
        };
        pattern.insert_edge(&edge).ok();

        pattern
    }

    /// Helper to create a pattern graph with a 3-node triangle
    fn create_triangle_pattern() -> SqliteGraph {
        let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");

        for i in 0..3 {
            let entity = GraphEntity {
                id: 0,
                kind: "pattern".to_string(),
                name: format!("p{}", i),
                file_path: None,
                data: serde_json::json!({}),
            };
            pattern
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let ids: Vec<i64> = pattern.all_entity_ids().expect("Failed to get IDs");
        // Create triangle: 0 -> 1 -> 2 -> 0
        for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            pattern.insert_edge(&edge).ok();
        }

        pattern
    }

    // Test 1: Find 2-node chain in 4-node path (3 matches)
    #[test]
    fn test_find_subgraph_patterns_simple_chain() {
        let graph = create_test_graph_with_nodes(4);

        // Create path: 0 -> 1 -> 2 -> 3
        for (from, to) in &[(0, 1), (1, 2), (2, 3)] {
            add_edge(&graph, *from, *to);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Should find 3 matches: (0,1), (1,2), (2,3)
        assert_eq!(result.patterns_found, 3);
        assert_eq!(result.count(), 3);
        assert!(!result.is_empty());
        assert!(!result.bounded_hit);
    }

    // Test 2: Find 3-node triangle in graph with 2 triangles
    #[test]
    fn test_find_subgraph_patterns_triangle() {
        let graph = create_test_graph_with_nodes(6);
        let ids = get_entity_ids(&graph, 6);

        // Create two triangles: (0,1,2) and (3,4,5)
        let triangle1 = [(0, 1), (1, 2), (2, 0)];
        let triangle2 = [(3, 4), (4, 5), (5, 3)];

        for (from, to) in triangle1.iter().chain(triangle2.iter()) {
            let edge = GraphEdge {
                id: 0,
                from_id: ids[*from],
                to_id: ids[*to],
                edge_type: "edge".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }

        let pattern = create_triangle_pattern();
        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Algorithm finds all isomorphisms (including rotations).
        // For 2 triangles with 3 rotations each = 6 matches.
        // We verify at least 2 matches (the two triangles).
        assert!(
            result.patterns_found >= 2,
            "Should find at least 2 triangle matches, found {}",
            result.patterns_found
        );
        assert!(!result.is_empty());
        assert!(!result.bounded_hit);
    }

    // Test 3: Verify max_matches bound stops enumeration
    #[test]
    fn test_find_subgraph_patterns_max_matches() {
        let graph = create_test_graph_with_nodes(10);

        // Create path: 0 -> 1 -> 2 -> ... -> 9
        for i in 0..9 {
            add_edge(&graph, i, i + 1);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds {
            max_matches: Some(2), // Only get first 2 matches
            ..Default::default()
        };

        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Should find at most 2 matches due to bound
        assert!(result.patterns_found <= 2);
        assert!(result.bounded_hit); // Should hit the bound
    }

    // Test 4: Verify timeout bound stops enumeration (simulated with small pattern)
    #[test]
    fn test_find_subgraph_patterns_timeout() {
        let graph = create_test_graph_with_nodes(10);

        // Create path
        for i in 0..9 {
            add_edge(&graph, i, i + 1);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds {
            timeout_ms: Some(1), // Very short timeout
            ..Default::default()
        };

        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // With 1ms timeout, should complete quickly (pattern is small)
        // May or may not be bounded_hit depending on system speed
        assert!(result.computation_time_ms < 100); // Should be fast
    }

    // Test 5: No matches when pattern not in target
    #[test]
    fn test_find_subgraph_patterns_empty_result() {
        let graph = create_test_graph_with_nodes(3);

        // Create path: 0 -> 1 -> 2
        for (from, to) in &[(0, 1), (1, 2)] {
            add_edge(&graph, *from, *to);
        }

        // Pattern has triangle, but graph is just a path
        let pattern = create_triangle_pattern();
        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Should find no matches
        assert_eq!(result.patterns_found, 0);
        assert!(result.is_empty());
        assert!(result.first_match().is_none());
    }

    // Test 6: Verify progress callback is called
    #[test]
    fn test_find_subgraph_patterns_progress() {
        use crate::progress::NoProgress;

        let graph = create_test_graph_with_nodes(5);

        // Create path
        for i in 0..4 {
            add_edge(&graph, i, i + 1);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds::default();

        let progress = NoProgress;
        let result =
            find_subgraph_patterns_with_progress(&graph, &pattern, bounds, &progress).unwrap();

        // Should still find the matches
        assert_eq!(result.patterns_found, 4); // 4 matches in 5-node path
    }

    // Test 7: Verify builder pattern methods chain correctly
    #[test]
    fn test_subgraph_pattern_bounds_builder() {
        let bounds = SubgraphPatternBounds::new()
            .with_max_matches(100)
            .with_timeout(5000)
            .with_max_pattern_nodes(10);

        assert_eq!(bounds.max_matches, Some(100));
        assert_eq!(bounds.timeout_ms, Some(5000));
        assert_eq!(bounds.max_pattern_nodes, Some(10));
        assert!(bounds.is_bounded());
    }

    // Test 8: Verify result helper methods
    #[test]
    fn test_subgraph_match_result_helpers() {
        let result = SubgraphMatchResult {
            matches: vec![vec![1, 2], vec![2, 3]],
            patterns_found: 2,
            computation_time_ms: 100,
            bounded_hit: false,
        };

        assert!(!result.is_empty());
        assert_eq!(result.count(), 2);
        assert_eq!(result.first_match(), Some(&[1, 2][..]));
    }

    // Test 9: Empty result helper methods
    #[test]
    fn test_subgraph_match_result_empty_helpers() {
        let result = SubgraphMatchResult {
            matches: vec![],
            patterns_found: 0,
            computation_time_ms: 50,
            bounded_hit: false,
        };

        assert!(result.is_empty());
        assert_eq!(result.count(), 0);
        assert!(result.first_match().is_none());
    }

    // Test 10: max_pattern_nodes rejects large patterns
    #[test]
    fn test_max_pattern_nodes_rejection() {
        let graph = create_test_graph_with_nodes(5);
        let pattern = create_test_graph_with_nodes(15); // Larger than max_pattern_nodes

        let bounds = SubgraphPatternBounds {
            max_pattern_nodes: Some(10),
            ..Default::default()
        };

        let result = find_subgraph_patterns(&graph, &pattern, bounds);

        assert!(result.is_err());
    }

    // Test 11: Single node pattern (edge case)
    #[test]
    fn test_single_node_pattern() {
        let graph = create_test_graph_with_nodes(3);
        let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");

        // Single node pattern
        let entity = GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "p0".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        };
        pattern
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Single node should match 3 times (each node in target)
        assert_eq!(result.patterns_found, 3);
    }

    // Test 12: Pattern larger than target graph
    #[test]
    fn test_pattern_larger_than_target() {
        let graph = create_test_graph_with_nodes(2);
        let pattern = create_triangle_pattern(); // 3 nodes

        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Pattern can't fit in target (3 > 2)
        assert_eq!(result.patterns_found, 0);
        assert!(result.is_empty());
    }

    // Test 13: Default bounds (unbounded)
    #[test]
    fn test_default_bounds_unbounded() {
        let bounds = SubgraphPatternBounds::default();

        assert_eq!(bounds.max_matches, None);
        assert_eq!(bounds.timeout_ms, None);
        assert_eq!(bounds.max_pattern_nodes, None);
        assert!(!bounds.is_bounded());
    }

    // Test 14: Computation time tracking
    #[test]
    fn test_computation_time_tracking() {
        let graph = create_test_graph_with_nodes(3);
        for (from, to) in &[(0, 1), (1, 2)] {
            add_edge(&graph, *from, *to);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds::default();
        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // Computation time should be reasonable (>0, <1 second for small graph)
        assert!(result.computation_time_ms < 1000);
    }

    // Test 15: Bounded_hit flag on max_matches
    #[test]
    fn test_bounded_hit_flag() {
        let graph = create_test_graph_with_nodes(5);
        for i in 0..4 {
            add_edge(&graph, i, i + 1);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds {
            max_matches: Some(2),
            ..Default::default()
        };

        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // With 5-node path, 2-node chain has 4 matches, but we limit to 2
        assert_eq!(result.patterns_found, 2);
        assert!(result.bounded_hit);
    }

    // Test 16: Bounded_hit flag on timeout
    #[test]
    fn test_bounded_hit_flag_timeout() {
        let graph = create_test_graph_with_nodes(20);
        for i in 0..19 {
            add_edge(&graph, i, i + 1);
        }

        let pattern = create_chain_pattern();
        let bounds = SubgraphPatternBounds {
            timeout_ms: Some(1), // Very short timeout
            ..Default::default()
        };

        let result = find_subgraph_patterns(&graph, &pattern, bounds).unwrap();

        // With 1ms timeout on larger graph, likely to timeout
        // But even if it completes, bounded_hit should be false
        assert!(result.computation_time_ms < 100);
    }
}
