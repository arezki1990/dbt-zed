//! Traversal algorithms using GraphBackend trait
//!
//! Backend-agnostic BFS, DFS, and k-hop traversal implementations.

use std::collections::{HashSet, VecDeque};

use crate::backend::GraphBackend;
use crate::errors::SqliteGraphError;

/// Breadth-first search traversal from a starting node.
///
/// Returns all nodes reachable from the start node via outgoing edges.
/// Uses only `fetch_outgoing()` trait method.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
/// * `start` - Starting node ID
///
/// # Returns
/// Vector of node IDs in BFS order (not including start node).
pub fn bfs_traversal(graph: &dyn GraphBackend, start: i64) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    visited.insert(start);
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        for neighbor in graph.fetch_outgoing(node)? {
            if visited.insert(neighbor) {
                result.push(neighbor);
                queue.push_back(neighbor);
            }
        }
    }

    Ok(result)
}

/// Depth-first search traversal from a starting node.
///
/// Returns all nodes reachable from the start node via outgoing edges.
/// Uses only `fetch_outgoing()` trait method.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
/// * `start` - Starting node ID
///
/// # Returns
/// Vector of node IDs in DFS order (not including start node).
pub fn dfs_traversal(graph: &dyn GraphBackend, start: i64) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut stack = vec![start];

    visited.insert(start);

    while let Some(node) = stack.pop() {
        for neighbor in graph.fetch_outgoing(node)? {
            if visited.insert(neighbor) {
                result.push(neighbor);
                stack.push(neighbor);
            }
        }
    }

    Ok(result)
}

/// Get all nodes within k hops from a starting node.
///
/// Returns nodes at distance 1 to k (inclusive) from start.
/// Uses only `fetch_outgoing()` trait method.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
/// * `start` - Starting node ID
/// * `k` - Maximum hop distance
///
/// # Returns
/// Vector of node IDs within k hops.
pub fn k_hop_neighbors(
    graph: &dyn GraphBackend,
    start: i64,
    k: usize,
) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    visited.insert(start);
    queue.push_back((start, 0));

    while let Some((node, depth)) = queue.pop_front() {
        if depth >= k {
            continue;
        }

        for neighbor in graph.fetch_outgoing(node)? {
            if visited.insert(neighbor) {
                result.push(neighbor);
                queue.push_back((neighbor, depth + 1));
            }
        }
    }

    Ok(result)
}

#[cfg(all(test, feature = "native-v3"))]
mod tests {
    use super::*;
    use crate::backend::native::v3::V3Backend;
    use crate::backend::{EdgeSpec, NodeSpec};
    use tempfile::TempDir;

    fn create_backend() -> (V3Backend, TempDir) {
        let temp_dir = TempDir::new().expect("invariant: temp dir creation succeeds");
        let db_path = temp_dir.path().join("test.graph");
        let backend = V3Backend::create(&db_path).expect("invariant: backend creation succeeds");
        (backend, temp_dir)
    }

    fn build_tree(backend: &V3Backend) -> (i64, Vec<i64>) {
        let root = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "root".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("invariant: backend operation succeeds");

        let mut children = Vec::new();
        for i in 0..3 {
            let child = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("child_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("invariant: backend operation succeeds");

            backend
                .insert_edge(EdgeSpec {
                    from: root,
                    to: child,
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .expect("invariant: backend operation succeeds");

            children.push(child);
        }

        (root, children)
    }

    #[test]
    fn test_bfs_traversal() {
        let (backend, _temp) = create_backend();
        let (root, children) = build_tree(&backend);

        let result = bfs_traversal(&backend, root).unwrap();

        assert_eq!(result.len(), 3);
        for child in &children {
            assert!(result.contains(child));
        }
    }

    #[test]
    fn test_dfs_traversal() {
        let (backend, _temp) = create_backend();
        let (root, children) = build_tree(&backend);

        let result = dfs_traversal(&backend, root).unwrap();

        assert_eq!(result.len(), 3);
        for child in &children {
            assert!(result.contains(child));
        }
    }

    #[test]
    fn test_k_hop_neighbors() {
        let (backend, _temp) = create_backend();
        let (root, children) = build_tree(&backend);

        // 1-hop should return all children
        let hop1 = k_hop_neighbors(&backend, root, 1).unwrap();
        assert_eq!(hop1.len(), 3);
        for child in &children {
            assert!(hop1.contains(child));
        }
    }

    #[test]
    fn test_traversal_isolated_node() {
        let (backend, _temp) = create_backend();
        let isolated = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "isolated".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let bfs_result = bfs_traversal(&backend, isolated).unwrap();
        assert!(bfs_result.is_empty());

        let dfs_result = dfs_traversal(&backend, isolated).unwrap();
        assert!(dfs_result.is_empty());

        let hop_result = k_hop_neighbors(&backend, isolated, 1).unwrap();
        assert!(hop_result.is_empty());
    }
}
