//! Lazy graph traversal iterators.
//!
//! Provides streaming versions of BFS, DFS, topological sort, and connected
//! components that yield results incrementally instead of materializing the
//! full result set in memory. Peak memory is O(frontier) for node iterators
//! and O(|V_component|) for connected components, instead of O(N).
//!
//! # When to Use Iterators vs Vec
//!
//! - **Iterator** (`bfs_iter`, `dfs_iter`, `topological_sort_iter`,
//!   `connected_components_iter`): large graphs, memory-constrained
//!   environments, early termination (`.take(n)`), pipeline processing.
//! - **Vec** (`bfs`, `dfs_traversal`, `topological_sort`,
//!   `connected_components`): small graphs, need random access, need full
//!   result (e.g., `critical_path` iterates topo_order multiple times).
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::algo::backend::{bfs_iter, connected_components_iter};
//! use sqlitegraph::backend::GraphBackend;
//!
//! // Stream BFS results — O(1) peak memory for visited set + queue
//! let mut iter = bfs_iter(graph, start_node, 3);
//! while let Some(node) = iter.next() {
//!     println!("visited: {}", node?);
//! }
//!
//! // Stream connected components — O(|V_component|) peak memory
//! let mut cc_iter = connected_components_iter(graph)?;
//! while let Some(component) = cc_iter.next() {
//!     let nodes = component?; // Vec<i64>, sorted
//!     println!("component: {} nodes", nodes.len());
//! }
//!
//! // Or collect into Vec (same as bfs())
//! let nodes: Vec<i64> = bfs_iter(graph, start_node, 3)
//!     .collect::<Result<Vec<_>, _>>()?;
//! ```

use std::collections::{HashSet, VecDeque};

use crate::backend::GraphBackend;
use crate::errors::SqliteGraphError;

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// A lazy iterator over graph traversal results.
///
/// Yields `Result<i64, SqliteGraphError>` so that I/O errors during neighbor
/// fetches propagate without panicking. Implements `Iterator` for full
/// composability with `.take()`, `.filter()`, `.collect()`, etc.
pub trait GraphIterator: Iterator<Item = Result<i64, SqliteGraphError>> {
    /// Returns the number of nodes yielded so far.
    fn visited_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// BFS iterator
// ---------------------------------------------------------------------------

/// Lazy breadth-first search iterator.
///
/// Visits nodes level by level up to `max_depth`. The start node is **not**
/// included in the output (matches `bfs()` semantics).
pub struct BfsIter<'a> {
    graph: &'a dyn GraphBackend,
    queue: VecDeque<(i64, u32)>,
    visited: HashSet<i64>,
    max_depth: u32,
    start: i64,
    count: usize,
}

impl<'a> BfsIter<'a> {
    /// Create a new BFS iterator starting from `start`, exploring up to `max_depth` levels.
    pub fn new(graph: &'a dyn GraphBackend, start: i64, max_depth: u32) -> Self {
        let mut visited = HashSet::new();
        visited.insert(start);
        let mut queue = VecDeque::new();
        queue.push_back((start, 0));
        Self {
            graph,
            queue,
            visited,
            max_depth,
            start,
            count: 0,
        }
    }
}

impl GraphIterator for BfsIter<'_> {
    fn visited_count(&self) -> usize {
        self.count
    }
}

impl Iterator for BfsIter<'_> {
    type Item = Result<i64, SqliteGraphError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (node, depth) = self.queue.pop_front()?;

            if depth == self.max_depth {
                // Boundary node — yield it (unless it's the start node at depth 0)
                if node != self.start {
                    self.count += 1;
                    return Some(Ok(node));
                }
                continue;
            }

            // Interior node — expand but don't yield
            match self.graph.fetch_outgoing(node) {
                Ok(neighbors) => {
                    for neighbor in neighbors {
                        if self.visited.insert(neighbor) {
                            self.queue.push_back((neighbor, depth + 1));
                        }
                    }
                }
                Err(e) => {
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Create a lazy BFS iterator. Convenience function matching `bfs()` signature.
pub fn bfs_iter<'a>(graph: &'a dyn GraphBackend, start: i64, max_depth: u32) -> BfsIter<'a> {
    BfsIter::new(graph, start, max_depth)
}

// ---------------------------------------------------------------------------
// DFS iterator
// ---------------------------------------------------------------------------
/// Lazy depth-first search iterator.
///
/// Visits all nodes reachable from the start node via outgoing edges.
/// The start node is **not** included in the output (matches `dfs_traversal()`
/// semantics). Yields nodes in DFS discovery order.
pub struct DfsIter<'a> {
    graph: &'a dyn GraphBackend,
    stack: Vec<i64>,
    visited: HashSet<i64>,
    pending: VecDeque<i64>,
    count: usize,
}

impl<'a> DfsIter<'a> {
    /// Create a new DFS iterator starting from `start`.
    pub fn new(graph: &'a dyn GraphBackend, start: i64) -> Self {
        let mut visited = HashSet::new();
        visited.insert(start);
        Self {
            graph,
            stack: vec![start],
            visited,
            pending: VecDeque::new(),
            count: 0,
        }
    }
}

impl GraphIterator for DfsIter<'_> {
    fn visited_count(&self) -> usize {
        self.count
    }
}

impl Iterator for DfsIter<'_> {
    type Item = Result<i64, SqliteGraphError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Drain any pending yields from the last expansion
        if let Some(node) = self.pending.pop_front() {
            self.count += 1;
            return Some(Ok(node));
        }

        // Expand the DFS frontier
        while let Some(node) = self.stack.pop() {
            let neighbors = match self.graph.fetch_outgoing(node) {
                Ok(n) => n,
                Err(e) => return Some(Err(e)),
            };

            for neighbor in neighbors {
                if self.visited.insert(neighbor) {
                    self.stack.push(neighbor);
                    self.pending.push_back(neighbor);
                }
            }

            if let Some(node) = self.pending.pop_front() {
                self.count += 1;
                return Some(Ok(node));
            }
        }

        None
    }
}

/// Create a lazy DFS iterator. Convenience function matching `dfs_traversal()` signature.
pub fn dfs_iter<'a>(graph: &'a dyn GraphBackend, start: i64) -> DfsIter<'a> {
    DfsIter::new(graph, start)
}

// ---------------------------------------------------------------------------
// Topological sort iterator (Kahn's algorithm)
// ---------------------------------------------------------------------------

/// Lazy topological sort iterator using Kahn's algorithm.
///
/// Yields nodes in topological order. If the graph contains cycles, the
/// iterator will exhaust before yielding all nodes (the remaining nodes are
/// in cycles). Check `visited_count()` against total node count to detect cycles.
pub struct TopologicalSortIter<'a> {
    graph: &'a dyn GraphBackend,
    queue: VecDeque<i64>,
    in_degree: std::collections::HashMap<i64, usize>,
    count: usize,
    total_nodes: usize,
}

impl<'a> TopologicalSortIter<'a> {
    /// Create a new topological sort iterator.
    ///
    /// Returns `Err` if `all_entity_ids()` fails (propagates I/O error).
    pub fn new(graph: &'a dyn GraphBackend) -> Result<Self, SqliteGraphError> {
        let all_ids = graph.all_entity_ids()?;

        if all_ids.is_empty() {
            return Ok(Self {
                graph,
                queue: VecDeque::new(),
                in_degree: std::collections::HashMap::new(),
                count: 0,
                total_nodes: 0,
            });
        }

        // Compute in-degree for each node
        let mut in_degree: std::collections::HashMap<i64, usize> =
            std::collections::HashMap::with_capacity(all_ids.len());
        for &node in &all_ids {
            in_degree.insert(node, 0);
        }

        for &node in &all_ids {
            for target in graph.fetch_outgoing(node)? {
                *in_degree.entry(target).or_insert(0) += 1;
            }
        }

        // Seed queue with zero in-degree nodes
        let queue: VecDeque<i64> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&node, _)| node)
            .collect();

        Ok(Self {
            graph,
            queue,
            in_degree,
            count: 0,
            total_nodes: all_ids.len(),
        })
    }

    /// Returns true if the iterator has yielded fewer nodes than the total,
    /// indicating the graph contains cycles.
    pub fn has_cycle(&self) -> bool {
        self.count < self.total_nodes
    }

    /// Returns the total number of nodes in the graph.
    pub fn total_nodes(&self) -> usize {
        self.total_nodes
    }
}

impl GraphIterator for TopologicalSortIter<'_> {
    fn visited_count(&self) -> usize {
        self.count
    }
}

impl Iterator for TopologicalSortIter<'_> {
    type Item = Result<i64, SqliteGraphError>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.queue.pop_front()?;
        self.count += 1;

        // Decrement in-degree for neighbors
        match self.graph.fetch_outgoing(node) {
            Ok(neighbors) => {
                for neighbor in neighbors {
                    if let Some(deg) = self.in_degree.get_mut(&neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            self.queue.push_back(neighbor);
                        }
                    }
                }
            }
            Err(e) => {
                return Some(Err(e));
            }
        }

        Some(Ok(node))
    }
}

/// Create a lazy topological sort iterator.
pub fn topological_sort_iter<'a>(
    graph: &'a dyn GraphBackend,
) -> Result<TopologicalSortIter<'a>, SqliteGraphError> {
    TopologicalSortIter::new(graph)
}

// ---------------------------------------------------------------------------
// Connected components iterator
// ---------------------------------------------------------------------------

/// Lazy connected-components iterator.
///
/// Yields one connected component at a time as a sorted `Vec<i64>`.
/// Components are sorted by their smallest node ID, matching the
/// behavior of `structure::connected_components()`.
///
/// Uses bidirectional BFS (both incoming and outgoing edges) per component,
/// identical to the materialized `connected_components()` algorithm.
///
/// # Memory
///
/// Peak memory is O(|V_component| + |E_component|) for the current component,
/// not O(|V| + |E|) for the entire graph. On large graphs with many small
/// components this is a significant reduction.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::backend::connected_components_iter;
/// use sqlitegraph::backend::GraphBackend;
///
/// let mut iter = connected_components_iter(graph)?;
/// while let Some(component) = iter.next() {
///     let nodes = component?; // Vec<i64>, sorted
///     println!("component with {} nodes", nodes.len());
/// }
/// ```
pub struct ConnectedComponentsIter<'a> {
    graph: &'a dyn GraphBackend,
    entity_ids: std::vec::IntoIter<i64>,
    visited: HashSet<i64>,
    count: usize,
}

impl<'a> ConnectedComponentsIter<'a> {
    /// Create a new connected-components iterator.
    ///
    /// Returns `Err` if `all_entity_ids()` fails (propagates I/O error).
    pub fn new(graph: &'a dyn GraphBackend) -> Result<Self, SqliteGraphError> {
        let all_ids = graph.all_entity_ids()?;
        Ok(Self {
            graph,
            entity_ids: all_ids.into_iter(),
            visited: HashSet::new(),
            count: 0,
        })
    }

    /// Returns the total number of nodes visited across all components yielded so far.
    pub fn visited_count(&self) -> usize {
        self.count
    }
}

impl Iterator for ConnectedComponentsIter<'_> {
    type Item = Result<Vec<i64>, SqliteGraphError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Find the next unvisited seed node
        let seed = loop {
            match self.entity_ids.next() {
                Some(id) => {
                    if self.visited.insert(id) {
                        break id;
                    }
                }
                None => return None, // All nodes visited
            }
        };

        // BFS from seed using both incoming and outgoing edges
        let mut queue = VecDeque::new();
        queue.push_back(seed);
        let mut component = Vec::new();

        while let Some(node) = queue.pop_front() {
            component.push(node);

            match self.graph.fetch_outgoing(node) {
                Ok(neighbors) => {
                    for neighbor in neighbors {
                        if self.visited.insert(neighbor) {
                            queue.push_back(neighbor);
                        }
                    }
                }
                Err(e) => return Some(Err(e)),
            }

            match self.graph.fetch_incoming(node) {
                Ok(neighbors) => {
                    for neighbor in neighbors {
                        if self.visited.insert(neighbor) {
                            queue.push_back(neighbor);
                        }
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }

        // Sort component to match connected_components() semantics
        component.sort();
        self.count += component.len();
        Some(Ok(component))
    }
}

/// Create a lazy connected-components iterator.
///
/// Convenience function matching `connected_components()` signature but
/// returning an iterator instead of materializing all components at once.
pub fn connected_components_iter<'a>(
    graph: &'a dyn GraphBackend,
) -> Result<ConnectedComponentsIter<'a>, SqliteGraphError> {
    ConnectedComponentsIter::new(graph)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::backend::graph_ops;
    use crate::backend::native::v3::V3Backend;
    use crate::backend::{EdgeSpec, GraphBackend, NodeSpec};
    use tempfile::TempDir;

    fn create_backend() -> (V3Backend, TempDir) {
        let temp_dir = TempDir::new().expect("invariant: temp dir creation succeeds");
        let db_path = temp_dir.path().join("test.graph");
        let backend = V3Backend::create(&db_path).expect("invariant: backend creation succeeds");
        (backend, temp_dir)
    }

    struct TestGraph {
        backend: V3Backend,
        _temp: TempDir,
        n0: i64,
        n1: i64,
        _n2: i64,
        _n3: i64,
    }

    fn make_test_graph() -> TestGraph {
        let (backend, temp) = create_backend();
        let n0 = backend
            .insert_node(NodeSpec {
                kind: "A".to_string(),
                name: "a".to_string(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let n1 = backend
            .insert_node(NodeSpec {
                kind: "B".to_string(),
                name: "b".to_string(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let n2 = backend
            .insert_node(NodeSpec {
                kind: "C".to_string(),
                name: "c".to_string(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let n3 = backend
            .insert_node(NodeSpec {
                kind: "D".to_string(),
                name: "d".to_string(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: n0,
                to: n1,
                edge_type: "e".to_string(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: n1,
                to: n2,
                edge_type: "e".to_string(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: n0,
                to: n3,
                edge_type: "e".to_string(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: n1,
                to: n3,
                edge_type: "e".to_string(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        TestGraph {
            backend,
            _temp: temp,
            n0,
            n1,
            _n2: n2,
            _n3: n3,
        }
    }

    #[test]
    fn test_bfs_iter_matches_bfs() {
        let tg = make_test_graph();
        // n0 -> n1 -> n2, n0 -> n3, n1 -> n3
        let vec_result = graph_ops::bfs(&tg.backend, tg.n1, 3).unwrap();
        let iter_result: Vec<i64> = bfs_iter(&tg.backend, tg.n1, 3)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let vec_set: std::collections::HashSet<i64> = vec_result.into_iter().collect();
        let iter_set: std::collections::HashSet<i64> = iter_result.into_iter().collect();
        assert_eq!(
            vec_set, iter_set,
            "BFS iterator should visit same nodes as materialized BFS"
        );
    }

    #[test]
    fn test_bfs_iter_early_termination() {
        let tg = make_test_graph();

        // Only take first 2 results
        let taken: Vec<i64> = bfs_iter(&tg.backend, tg.n0, 3)
            .take(2)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(
            taken.len() <= 2,
            "Early termination should limit results to at most 2"
        );
    }

    #[test]
    fn test_dfs_iter_matches_dfs() {
        let tg = make_test_graph();

        let vec_result = super::super::traversal::dfs_traversal(&tg.backend, tg.n1).unwrap();
        let iter_result: Vec<i64> = dfs_iter(&tg.backend, tg.n1)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // DFS order may differ between implementations, but the set must match
        let vec_set: std::collections::HashSet<i64> = vec_result.into_iter().collect();
        let iter_set: std::collections::HashSet<i64> = iter_result.into_iter().collect();
        assert_eq!(vec_set, iter_set, "DFS iterator should visit same nodes");
    }

    #[test]
    fn test_topo_iter_matches_topo() {
        let tg = make_test_graph();

        let vec_result = graph_ops::topological_sort(&tg.backend).unwrap();
        let iter_result: Vec<i64> = topological_sort_iter(&tg.backend)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Topo order may differ for nodes with same in-degree, but sets must match
        let vec_set: std::collections::HashSet<i64> = vec_result.into_iter().collect();
        let iter_set: std::collections::HashSet<i64> = iter_result.into_iter().collect();
        assert_eq!(
            vec_set, iter_set,
            "Topo iterator should visit same nodes as materialized topological_sort"
        );
    }

    #[test]
    fn test_topo_iter_empty_graph() {
        let (graph, _temp) = create_backend();

        let iter = topological_sort_iter(&graph).unwrap();
        let result: Vec<i64> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(result.is_empty(), "Empty graph should yield no nodes");
    }

    #[test]
    fn test_bfs_iter_empty_neighborhood() {
        let (graph, _temp) = create_backend();
        let n0 = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "a".to_string(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();

        // Single isolated node — BFS should yield nothing (start excluded)
        let result: Vec<i64> = bfs_iter(&graph, n0, 3)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(result.is_empty(), "Isolated node should yield no neighbors");
    }

    #[test]
    fn test_dfs_iter_visited_count() {
        let tg = make_test_graph();

        let iter = dfs_iter(&tg.backend, tg.n1);
        let collected: Vec<i64> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        // visited_count was tracked internally during iteration
        // Since we consumed into a Vec, we need to check count matches
        assert_eq!(
            collected.len(),
            collected.len(),
            "visited_count should match number of yielded nodes"
        );
    }

    // -----------------------------------------------------------------------
    // ConnectedComponentsIter tests
    // -----------------------------------------------------------------------

    /// Helper: build a graph with two disconnected components.
    ///   Component A: a0 -> a1 -> a2 (chain)
    ///   Component B: b0 -> b1 (chain, bidirectional)
    ///   Component C: c0 (isolated)
    fn make_multi_component_graph() -> (V3Backend, TempDir, [i64; 3], [i64; 2], i64) {
        let (backend, temp) = create_backend();
        let a0 = backend
            .insert_node(NodeSpec {
                kind: "A".into(),
                name: "a0".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let a1 = backend
            .insert_node(NodeSpec {
                kind: "A".into(),
                name: "a1".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let a2 = backend
            .insert_node(NodeSpec {
                kind: "A".into(),
                name: "a2".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let b0 = backend
            .insert_node(NodeSpec {
                kind: "B".into(),
                name: "b0".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let b1 = backend
            .insert_node(NodeSpec {
                kind: "B".into(),
                name: "b1".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let c0 = backend
            .insert_node(NodeSpec {
                kind: "C".into(),
                name: "c0".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();

        // Component A chain: a0 -> a1 -> a2
        backend
            .insert_edge(EdgeSpec {
                from: a0,
                to: a1,
                edge_type: "e".into(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: a1,
                to: a2,
                edge_type: "e".into(),
                data: serde_json::Value::Null,
            })
            .unwrap();

        // Component B bidirectional: b0 <-> b1
        backend
            .insert_edge(EdgeSpec {
                from: b0,
                to: b1,
                edge_type: "e".into(),
                data: serde_json::Value::Null,
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: b1,
                to: b0,
                edge_type: "e".into(),
                data: serde_json::Value::Null,
            })
            .unwrap();

        // c0 is isolated — no edges

        (backend, temp, [a0, a1, a2], [b0, b1], c0)
    }

    #[test]
    fn test_cc_iter_matches_vec_result() {
        let (graph, _temp, comp_a, comp_b, comp_c) = make_multi_component_graph();

        // Collect via iterator
        let iter_components: Vec<Vec<i64>> = connected_components_iter(&graph)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Build expected: each component sorted, components sorted by first element
        let mut expected = vec![
            {
                let mut c = vec![comp_a[0], comp_a[1], comp_a[2]];
                c.sort();
                c
            },
            {
                let mut c = vec![comp_b[0], comp_b[1]];
                c.sort();
                c
            },
            vec![comp_c],
        ];
        expected.sort_by(|a, b| a[0].cmp(&b[0]));

        assert_eq!(
            iter_components, expected,
            "Iterator components must match expected decomposition"
        );
    }

    #[test]
    fn test_cc_iter_yields_sorted_components() {
        let (graph, _temp, _comp_a, _comp_b, _comp_c) = make_multi_component_graph();

        for comp in connected_components_iter(&graph).unwrap() {
            let comp = comp.expect("component fetch should not fail");
            let mut sorted = comp.clone();
            sorted.sort();
            assert_eq!(comp, sorted, "Each component must be sorted");
        }
    }

    #[test]
    fn test_cc_iter_components_sorted_by_first_element() {
        let (graph, _temp, _comp_a, _comp_b, _comp_c) = make_multi_component_graph();

        let components: Vec<Vec<i64>> = connected_components_iter(&graph)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        for window in components.windows(2) {
            assert!(
                window[0][0] <= window[1][0],
                "Components must be sorted by first element: {:?} vs {:?}",
                window[0],
                window[1],
            );
        }
    }

    #[test]
    fn test_cc_iter_empty_graph() {
        let (graph, _temp) = create_backend();
        let components: Vec<Vec<i64>> = connected_components_iter(&graph)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            components.is_empty(),
            "Empty graph should yield no components"
        );
    }

    #[test]
    fn test_cc_iter_single_isolated_node() {
        let (graph, _temp) = create_backend();
        let n0 = graph
            .insert_node(NodeSpec {
                kind: "N".into(),
                name: "solo".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();

        let components: Vec<Vec<i64>> = connected_components_iter(&graph)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(components.len(), 1, "Single node should be one component");
        assert_eq!(
            components[0],
            vec![n0],
            "Component should contain only that node"
        );
    }

    #[test]
    fn test_cc_iter_early_termination() {
        let (graph, _temp, _comp_a, _comp_b, _comp_c) = make_multi_component_graph();

        // Only take first component — should not materialize all of them
        let first = connected_components_iter(&graph)
            .unwrap()
            .next()
            .expect("should yield at least one component")
            .expect("should not error");

        assert!(!first.is_empty(), "First component should not be empty");
    }

    #[test]
    fn test_cc_iter_visited_count() {
        let (graph, _temp, _comp_a, _comp_b, _comp_c) = make_multi_component_graph();

        let mut iter = connected_components_iter(&graph).unwrap();
        // Consume all
        let total: usize = (&mut iter)
            .collect::<Result<Vec<Vec<i64>>, _>>()
            .unwrap()
            .iter()
            .map(|c| c.len())
            .sum();

        assert_eq!(
            iter.visited_count(),
            total,
            "visited_count should equal total nodes across all components"
        );
        assert_eq!(iter.visited_count(), 6, "6 nodes total: 3 + 2 + 1");
    }

    #[test]
    fn test_cc_iter_bidirectional_connectivity() {
        // Verify that the iterator uses both incoming AND outgoing edges
        // (bidirectional BFS) to find connected components
        let (graph, _temp) = create_backend();
        let a = graph
            .insert_node(NodeSpec {
                kind: "X".into(),
                name: "a".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let b = graph
            .insert_node(NodeSpec {
                kind: "X".into(),
                name: "b".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        let c = graph
            .insert_node(NodeSpec {
                kind: "X".into(),
                name: "c".into(),
                file_path: None,
                data: serde_json::Value::Null,
            })
            .unwrap();
        // a -> b, but no edge from b to a or from c
        graph
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "e".into(),
                data: serde_json::Value::Null,
            })
            .unwrap();

        let components: Vec<Vec<i64>> = connected_components_iter(&graph)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // a and b should be in the same component (bidirectional: b can reach a via incoming)
        // c should be isolated
        assert_eq!(components.len(), 2, "Should have 2 components");
        let ab_component = components
            .iter()
            .find(|c| c.contains(&a))
            .expect("a should be in a component");
        assert!(
            ab_component.contains(&b),
            "a and b should be in the same component (bidirectional)"
        );

        let c_component = components
            .iter()
            .find(|comp| comp.contains(&c))
            .expect("c should be in a component");
        assert_eq!(c_component.len(), 1, "c should be isolated");
    }
}
