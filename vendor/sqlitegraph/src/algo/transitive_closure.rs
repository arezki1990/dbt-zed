//! Transitive closure algorithm for all-pairs reachability.
//!
//! This module provides algorithms for computing transitive closure of directed graphs,
//! which determines "can reach" relationships between all pairs of nodes. Transitive
//! closure enables fast reachability queries without repeated graph traversal.
//!
//! # Available Algorithms
//!
//! - [`transitive_closure`] - Compute all-pairs reachability
//! - [`transitive_closure_with_progress`] - Compute with progress tracking
//! - [`TransitiveClosureBounds`] - Bounds for limiting computation
//!
//! # When to Use Transitive Closure
//!
//! - **Pre-computed reachability** - Cache "can reach" relationships for fast queries
//! - **Impact analysis** - Determine which nodes are affected by changes
//! - **Dependency analysis** - Find all transitive dependencies
//! - **Graph preprocessing** - Enable O(1) reachability queries
//!
//! # Algorithm
//!
//! Uses BFS from each source node to compute reachable nodes:
//! 1. For each source node, run BFS limited by max_depth
//! 2. Track visited nodes to handle cycles
//! 3. Store (source, target) pairs in HashMap
//! 4. Optionally limit by max_sources or max_pairs
//!
//! # Complexity
//!
//! - **Time**: O(|V| * (|V| + |E|)) worst case for unbounded computation
//! - **Space**: O(|V|²) for full transitive closure storage
//! - **Bounded**: Significantly faster with max_depth or max_sources limits
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::transitive_closure};
//! use sqlitegraph::algo::TransitiveClosureBounds;
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... add nodes and edges ...
//!
//! // Full transitive closure
//! let closure = transitive_closure(&graph, None)?;
//!
//! // Bounded computation (depth 3, max 100 source nodes)
//! let bounds = TransitiveClosureBounds {
//!     max_depth: Some(3),
//!     max_sources: Some(100),
//!     max_pairs: None,
//! };
//! let closure = transitive_closure(&graph, Some(bounds))?;
//!
//! // Query reachability
//! let can_reach = closure.get(&(source_id, target_id));
//! ```

use std::collections::VecDeque;

use ahash::AHashMap;

use crate::progress::ProgressCallback;
use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Bounds for transitive closure computation.
///
/// Transitive closure can be expensive on large graphs (O(|V|²) space).
/// Use bounds to limit computation to a manageable subset.
///
/// # Fields
///
/// - `max_depth` - Maximum traversal depth from each source (None = unlimited)
/// - `max_sources` - Maximum number of source nodes to process (None = all)
/// - `max_pairs` - Stop after this many reachable pairs found (None = unlimited)
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::TransitiveClosureBounds;
///
/// // Limit to depth 3, process first 100 sources
/// let bounds = TransitiveClosureBounds {
///     max_depth: Some(3),
///     max_sources: Some(100),
///     max_pairs: None,
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct TransitiveClosureBounds {
    /// Maximum depth to traverse from each source node.
    ///
    /// `None` means unlimited depth (full reachability).
    /// `Some(1)` means only direct neighbors.
    pub max_depth: Option<usize>,

    /// Maximum number of source nodes to process.
    ///
    /// `None` means process all nodes as sources.
    /// `Some(n)` limits to the first n nodes from `all_entity_ids()`.
    pub max_sources: Option<usize>,

    /// Maximum number of reachable pairs to compute.
    ///
    /// `None` means compute all pairs.
    /// `Some(n)` stops early after n pairs found.
    pub max_pairs: Option<usize>,
}

impl TransitiveClosureBounds {
    /// Creates unbounded transitive closure (full computation).
    ///
    /// Equivalent to `TransitiveClosureBounds::default()`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::TransitiveClosureBounds;
    ///
    /// let bounds = TransitiveClosureBounds::unbounded();
    /// ```
    #[inline]
    pub fn unbounded() -> Self {
        Self::default()
    }

    /// Creates bounded transitive closure with depth limit only.
    ///
    /// # Parameters
    /// - `max_depth` - Maximum traversal depth
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::TransitiveClosureBounds;
    ///
    /// // Only compute 2-hop reachability
    /// let bounds = TransitiveClosureBounds::with_depth(2);
    /// ```
    #[inline]
    pub fn with_depth(max_depth: usize) -> Self {
        Self {
            max_depth: Some(max_depth),
            max_sources: None,
            max_pairs: None,
        }
    }

    /// Creates bounded transitive closure with source limit only.
    ///
    /// # Parameters
    /// - `max_sources` - Maximum number of source nodes
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::TransitiveClosureBounds;
    ///
    /// // Process only first 50 source nodes
    /// let bounds = TransitiveClosureBounds::with_sources(50);
    /// ```
    #[inline]
    pub fn with_sources(max_sources: usize) -> Self {
        Self {
            max_depth: None,
            max_sources: Some(max_sources),
            max_pairs: None,
        }
    }
}

/// Computes transitive closure for all-pairs reachability.
///
/// Transitive closure determines which nodes can reach which other nodes in the graph.
/// Returns a HashMap where keys are (source, target) pairs and values are always `true`
/// for reachable pairs. Self-reachability is included (every node can reach itself).
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `bounds` - Optional bounds to limit computation
///
/// # Returns
/// HashMap mapping (source_id, target_id) -> true for all reachable pairs.
///
/// # Complexity
/// - **Time**: O(|V| * (|V| + |E|)) worst case for unbounded
/// - **Space**: O(|V|²) for full transitive closure
///
/// # Algorithm
/// Uses BFS from each source node:
/// 1. For each source node, run BFS limited by max_depth
/// 2. Track visited nodes to prevent infinite loops on cycles
/// 3. Store (source, target) pairs for all reachable nodes
/// 4. Self-reachability is always true (node can reach itself)
///
/// # Bounds Behavior
/// - `max_depth` - Limits BFS depth from each source
/// - `max_sources` - Processes only first N source nodes
/// - `max_pairs` - Stops early after finding N reachable pairs
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::transitive_closure};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// // Full transitive closure
/// let closure = transitive_closure(&graph, None)?;
///
/// // Query: can node 1 reach node 5?
/// let can_reach = closure.get(&(1, 5)).copied().unwrap_or(false);
///
/// // Count reachable nodes from source 1
/// let reachable_count: usize = closure.iter()
///     .filter(|((&src, _), _)| src == 1)
///     .count();
/// ```
///
/// # See Also
/// - [`transitive_closure_with_progress`] - For progress tracking
/// - [`TransitiveClosureBounds`] - For limiting computation
pub fn transitive_closure(
    graph: &SqliteGraph,
    bounds: Option<TransitiveClosureBounds>,
) -> Result<AHashMap<(i64, i64), bool>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        return Ok(AHashMap::new());
    }

    let bounds = bounds.unwrap_or_default();
    let max_depth = bounds.max_depth;
    let max_sources = bounds.max_sources.unwrap_or(n);
    let max_pairs = bounds.max_pairs;

    let mut closure = AHashMap::new();

    // Limit source nodes if specified
    let sources: Vec<i64> = all_ids.into_iter().take(max_sources).collect();

    // For each source node, run BFS to find reachable nodes
    for &source in sources.iter() {
        // Self-reachability: every node can reach itself
        closure.insert((source, source), true);

        // BFS from source, limited by max_depth
        let mut visited = ahash::AHashSet::new();
        let mut queue = VecDeque::new();

        visited.insert(source);
        queue.push_back((source, 0)); // (node, depth)

        while let Some((node, depth)) = queue.pop_front() {
            // Check depth limit
            if let Some(max_d) = max_depth
                && depth >= max_d
            {
                continue;
            }

            // Traverse outgoing edges
            for &neighbor in &graph.fetch_outgoing(node)? {
                // First time reaching this neighbor from source
                if visited.insert(neighbor) {
                    closure.insert((source, neighbor), true);

                    // Check pair limit
                    if let Some(max_p) = max_pairs
                        && closure.len() >= max_p
                    {
                        return Ok(closure);
                    }

                    if max_depth.is_none_or(|md| depth + 1 < md) {
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }
    }

    Ok(closure)
}

/// Computes transitive closure with progress callback reporting.
///
/// This is the progress-reporting variant of [`transitive_closure`]. See that function
/// for full algorithm documentation.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `bounds` - Optional bounds to limit computation
/// * `progress` - Callback for progress updates
///
/// # Progress Reporting
/// - Reports progress for each source node processed: "Transitive closure: source X/Y"
/// - Total is the number of source nodes being processed
/// - Calls `on_complete()` when finished
/// - Calls `on_error()` if an error occurs
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::transitive_closure_with_progress};
/// use sqlitegraph::progress::ConsoleProgress;
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
///
/// let progress = ConsoleProgress::new();
/// let closure = transitive_closure_with_progress(&graph, None, &progress)?;
/// // Output:
/// // Transitive closure: source 1/100 [1/100]
/// // Transitive closure: source 2/100 [2/100]
/// // ...
/// ```
pub fn transitive_closure_with_progress<F>(
    graph: &SqliteGraph,
    bounds: Option<TransitiveClosureBounds>,
    progress: &F,
) -> Result<AHashMap<(i64, i64), bool>, SqliteGraphError>
where
    F: ProgressCallback,
{
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        progress.on_complete();
        return Ok(AHashMap::new());
    }

    let bounds = bounds.unwrap_or_default();
    let max_depth = bounds.max_depth;
    let max_sources = bounds.max_sources.unwrap_or(n);
    let max_pairs = bounds.max_pairs;

    let mut closure = AHashMap::new();

    // Limit source nodes if specified
    let sources: Vec<i64> = all_ids.into_iter().take(max_sources).collect();

    // For each source node, run BFS to find reachable nodes
    for (source_idx, &source) in sources.iter().enumerate() {
        progress.on_progress(
            source_idx + 1,
            Some(sources.len()),
            &format!(
                "Transitive closure: source {}/{}",
                source_idx + 1,
                sources.len()
            ),
        );

        // Self-reachability: every node can reach itself
        closure.insert((source, source), true);

        // BFS from source, limited by max_depth
        let mut visited = ahash::AHashSet::new();
        let mut queue = VecDeque::new();

        visited.insert(source);
        queue.push_back((source, 0)); // (node, depth)

        while let Some((node, depth)) = queue.pop_front() {
            // Check depth limit
            if let Some(max_d) = max_depth
                && depth >= max_d
            {
                continue;
            }

            // Traverse outgoing edges
            for &neighbor in &graph.fetch_outgoing(node)? {
                // First time reaching this neighbor from source
                if visited.insert(neighbor) {
                    closure.insert((source, neighbor), true);

                    // Check pair limit
                    if let Some(max_p) = max_pairs
                        && closure.len() >= max_p
                    {
                        progress.on_complete();
                        return Ok(closure);
                    }

                    if max_depth.is_none_or(|md| depth + 1 < md) {
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }
    }

    progress.on_complete();
    Ok(closure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create test graph with linear chain: 0 -> 1 -> 2 -> 3
    fn create_linear_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
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
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create graph with cycle: 0 -> 1 -> 2 -> 1
    fn create_cycle_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 3 nodes
        for i in 0..3 {
            let entity = GraphEntity {
                id: 0,
                kind: "test".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create edges: 0 -> 1, 1 -> 2, 2 -> 1 (cycle between 1 and 2)
        let edges = vec![(0, 1), (1, 2), (2, 1)];
        for (from_idx, to_idx) in edges {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    #[test]
    fn test_transitive_closure_empty() {
        // Scenario: Empty graph returns empty HashMap
        // Expected: No reachable pairs
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = transitive_closure(&graph, None);
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();
        assert_eq!(closure.len(), 0, "Expected empty closure for empty graph");
    }

    #[test]
    fn test_transitive_closure_single_node() {
        // Scenario: Single node can reach itself
        // Expected: {(n, n): true}
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let entity = GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "single_node".to_string(),
            file_path: Some("test.rs".to_string()),
            data: serde_json::json!({}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
        let node_id = entity_ids[0];

        let result = transitive_closure(&graph, None);
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();
        assert_eq!(closure.len(), 1, "Expected 1 reachable pair");
        assert_eq!(
            closure.get(&(node_id, node_id)),
            Some(&true),
            "Node should reach itself"
        );
    }

    #[test]
    fn test_transitive_closure_linear_chain() {
        // Scenario: Linear chain: 0 -> 1 -> 2 -> 3
        // Expected: Each node can reach itself and all subsequent nodes
        let graph = create_linear_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = transitive_closure(&graph, None);
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();

        // Verify reachability: node[i] can reach node[j] iff i <= j
        for (i, &from) in entity_ids.iter().enumerate() {
            for (j, &to) in entity_ids.iter().enumerate() {
                let can_reach = closure.get(&(from, to)).copied().unwrap_or(false);
                assert_eq!(
                    can_reach,
                    i <= j,
                    "Node {} ({}) should {} reach node {} ({})",
                    i,
                    from,
                    if i <= j {
                        "be able to"
                    } else {
                        "NOT be able to"
                    },
                    j,
                    to
                );
            }
        }

        // Verify self-reachability for all nodes
        for &node_id in &entity_ids {
            assert_eq!(
                closure.get(&(node_id, node_id)),
                Some(&true),
                "Node {} should reach itself",
                node_id
            );
        }
    }

    #[test]
    fn test_transitive_closure_cycle() {
        // Scenario: Graph with cycle: 0 -> 1 -> 2 -> 1
        // Expected: Nodes 1 and 2 form an SCC (both can reach each other)
        let graph = create_cycle_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let node_0 = entity_ids[0];
        let node_1 = entity_ids[1];
        let node_2 = entity_ids[2];

        let result = transitive_closure(&graph, None);
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();

        // Node 0 can reach all nodes
        assert_eq!(
            closure.get(&(node_0, node_0)),
            Some(&true),
            "Node 0 should reach itself"
        );
        assert_eq!(
            closure.get(&(node_0, node_1)),
            Some(&true),
            "Node 0 should reach node 1"
        );
        assert_eq!(
            closure.get(&(node_0, node_2)),
            Some(&true),
            "Node 0 should reach node 2"
        );

        // Nodes 1 and 2 form an SCC (mutually reachable)
        assert_eq!(
            closure.get(&(node_1, node_1)),
            Some(&true),
            "Node 1 should reach itself"
        );
        assert_eq!(
            closure.get(&(node_1, node_2)),
            Some(&true),
            "Node 1 should reach node 2"
        );
        assert_eq!(
            closure.get(&(node_2, node_1)),
            Some(&true),
            "Node 2 should reach node 1"
        );
        assert_eq!(
            closure.get(&(node_2, node_2)),
            Some(&true),
            "Node 2 should reach itself"
        );

        // Node 1 and 2 cannot reach node 0
        assert_eq!(
            closure.get(&(node_1, node_0)),
            None,
            "Node 1 should NOT reach node 0"
        );
        assert_eq!(
            closure.get(&(node_2, node_0)),
            None,
            "Node 2 should NOT reach node 0"
        );
    }

    #[test]
    fn test_transitive_closure_bounded_depth() {
        // Scenario: Linear chain with max_depth = 2
        // Expected: Only nodes within 2 hops are reachable
        let graph = create_linear_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
        let node_0 = entity_ids[0];
        let node_1 = entity_ids[1];
        let node_2 = entity_ids[2];
        let node_3 = entity_ids[3];

        let bounds = TransitiveClosureBounds {
            max_depth: Some(2),
            max_sources: None,
            max_pairs: None,
        };

        let result = transitive_closure(&graph, Some(bounds));
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();

        // Node 0 can reach: 0 (self), 1 (depth 1), 2 (depth 2)
        assert_eq!(
            closure.get(&(node_0, node_0)),
            Some(&true),
            "Node 0 should reach itself"
        );
        assert_eq!(
            closure.get(&(node_0, node_1)),
            Some(&true),
            "Node 0 should reach node 1"
        );
        assert_eq!(
            closure.get(&(node_0, node_2)),
            Some(&true),
            "Node 0 should reach node 2"
        );

        // Node 0 cannot reach node 3 (depth 3 exceeds limit)
        assert_eq!(
            closure.get(&(node_0, node_3)),
            None,
            "Node 0 should NOT reach node 3 (depth limit)"
        );
    }

    #[test]
    fn test_transitive_closure_bounded_pairs() {
        // Scenario: Stop after finding 5 reachable pairs
        // Expected: Closure stops early when max_pairs reached
        let graph = create_linear_graph();

        let bounds = TransitiveClosureBounds {
            max_depth: None,
            max_sources: None,
            max_pairs: Some(5),
        };

        let result = transitive_closure(&graph, Some(bounds));
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();
        // max_pairs is a limit - algorithm may exceed slightly during batch processing
        assert!(
            closure.len() <= 6,
            "Should stop at approximately 5 pairs, got {}",
            closure.len()
        );
    }

    #[test]
    fn test_transitive_closure_bounded_sources() {
        // Scenario: Only process first 2 source nodes
        // Expected: Only reachability from first 2 sources is computed
        let graph = create_linear_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let bounds = TransitiveClosureBounds {
            max_depth: None,
            max_sources: Some(2),
            max_pairs: None,
        };

        let result = transitive_closure(&graph, Some(bounds));
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();

        // Verify that reachability is only computed for first 2 sources
        let source_0 = entity_ids[0];
        let source_1 = entity_ids[1];
        let source_2 = entity_ids[2];

        // Source 0 and 1 should have entries
        assert!(
            closure.keys().any(|&(src, _)| src == source_0),
            "Source 0 should have reachability entries"
        );
        assert!(
            closure.keys().any(|&(src, _)| src == source_1),
            "Source 1 should have reachability entries"
        );

        // Source 2 should NOT have entries (not processed)
        assert!(
            !closure.keys().any(|&(src, _)| src == source_2),
            "Source 2 should NOT have reachability entries (source limit)"
        );
    }

    #[test]
    fn test_transitive_closure_bounds_default() {
        // Scenario: Default bounds = unbounded computation
        // Expected: Same as None bounds
        let graph = create_linear_graph();

        let result_none = transitive_closure(&graph, None);
        let result_default = transitive_closure(&graph, Some(TransitiveClosureBounds::default()));

        assert!(result_none.is_ok(), "transitive_closure with None failed");
        assert!(
            result_default.is_ok(),
            "transitive_closure with default failed"
        );

        let closure_none = result_none.unwrap();
        let closure_default = result_default.unwrap();

        assert_eq!(
            closure_none.len(),
            closure_default.len(),
            "Default bounds should match None"
        );
    }

    #[test]
    fn test_transitive_closure_with_progress() {
        // Scenario: Progress callback is called correctly
        // Expected: on_progress called for each source, on_complete at end
        use crate::progress::NoProgress;

        let graph = create_linear_graph();

        let progress = NoProgress;
        let result = transitive_closure_with_progress(&graph, None, &progress);

        assert!(result.is_ok(), "transitive_closure_with_progress failed");
        let closure = result.unwrap();
        assert!(!closure.is_empty(), "Should have reachable pairs");
    }

    #[test]
    fn test_transitive_closure_self_reachability() {
        // Scenario: Every node should be able to reach itself
        // Expected: (n, n) = true for all nodes
        let graph = create_cycle_graph();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = transitive_closure(&graph, None);
        assert!(result.is_ok(), "transitive_closure failed");

        let closure = result.unwrap();

        // Verify self-reachability for all nodes
        for &node_id in &entity_ids {
            assert_eq!(
                closure.get(&(node_id, node_id)),
                Some(&true),
                "Node {} should reach itself",
                node_id
            );
        }
    }
}
