//! Graph operations using GraphBackend trait
//!
//! Backend-agnostic implementations of shortest path, SCC, topological sort.

use std::collections::{HashSet, VecDeque};

use ahash::AHashMap;

use crate::backend::GraphBackend;
use crate::errors::SqliteGraphError;

/// Result of strongly connected components computation.
#[derive(Debug, Clone)]
pub struct SccResult {
    /// List of SCCs, each is a vector of node IDs
    pub components: Vec<Vec<i64>>,
    /// Mapping from node ID to component index
    pub node_to_component: AHashMap<i64, usize>,
}

/// Computes strongly connected components using Tarjan's algorithm.
///
/// Backend-agnostic version using GraphBackend trait.
/// Uses `all_entity_ids()` and `fetch_outgoing()` trait methods.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
///
/// # Returns
/// SccResult containing components and node-to-component mapping.
pub fn strongly_connected_components(
    graph: &dyn GraphBackend,
) -> Result<SccResult, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        return Ok(SccResult {
            components: Vec::new(),
            node_to_component: AHashMap::new(),
        });
    }

    let mut index = 0;
    let mut stack: Vec<i64> = Vec::new();
    let mut on_stack: HashSet<i64> = HashSet::new();
    let mut indices: AHashMap<i64, usize> = AHashMap::new();
    let mut lowlinks: AHashMap<i64, usize> = AHashMap::new();
    let mut components: Vec<Vec<i64>> = Vec::new();
    let mut node_to_component: AHashMap<i64, usize> = AHashMap::new();

    fn strongconnect(
        v: i64,
        graph: &dyn GraphBackend,
        index: &mut usize,
        stack: &mut Vec<i64>,
        on_stack: &mut HashSet<i64>,
        indices: &mut AHashMap<i64, usize>,
        lowlinks: &mut AHashMap<i64, usize>,
        components: &mut Vec<Vec<i64>>,
        node_to_component: &mut AHashMap<i64, usize>,
    ) -> Result<(), SqliteGraphError> {
        indices.insert(v, *index);
        lowlinks.insert(v, *index);
        *index += 1;
        stack.push(v);
        on_stack.insert(v);

        for &w in &graph.fetch_outgoing(v)? {
            if !indices.contains_key(&w) {
                strongconnect(
                    w,
                    graph,
                    index,
                    stack,
                    on_stack,
                    indices,
                    lowlinks,
                    components,
                    node_to_component,
                )?;
                let v_low = lowlinks[&v];
                let w_low = lowlinks[&w];
                lowlinks.insert(v, v_low.min(w_low));
            } else if on_stack.contains(&w) {
                let v_low = lowlinks[&v];
                let w_idx = indices[&w];
                lowlinks.insert(v, v_low.min(w_idx));
            }
        }

        if lowlinks[&v] == indices[&v] {
            let mut component = Vec::new();
            loop {
                let w = stack.pop().expect("invariant: stack non-empty during DFS");
                on_stack.remove(&w);
                node_to_component.insert(w, components.len());
                component.push(w);
                if w == v {
                    break;
                }
            }
            components.push(component);
        }

        Ok(())
    }

    for &v in &all_ids {
        if !indices.contains_key(&v) {
            strongconnect(
                v,
                graph,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlinks,
                &mut components,
                &mut node_to_component,
            )?;
        }
    }

    Ok(SccResult {
        components,
        node_to_component,
    })
}

/// Finds the shortest path between two nodes using BFS.
///
/// Backend-agnostic version for unweighted graphs.
/// Uses `fetch_outgoing()` trait method.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
/// * `start` - Starting node ID
/// * `end` - Target node ID
///
/// # Returns
/// Some(path) if a path exists, None otherwise.
/// Path includes both start and end nodes.
pub fn shortest_path(
    graph: &dyn GraphBackend,
    start: i64,
    end: i64,
) -> Result<Option<Vec<i64>>, SqliteGraphError> {
    if start == end {
        return Ok(Some(vec![start]));
    }

    let mut visited = HashSet::new();
    let mut predecessors: AHashMap<i64, i64> = AHashMap::new();
    let mut queue = VecDeque::new();

    visited.insert(start);
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        for neighbor in graph.fetch_outgoing(node)? {
            if visited.insert(neighbor) {
                predecessors.insert(neighbor, node);

                if neighbor == end {
                    // Reconstruct path
                    let mut path = vec![end];
                    let mut current = end;
                    while let Some(&pred) = predecessors.get(&current) {
                        path.push(pred);
                        current = pred;
                    }
                    path.reverse();
                    return Ok(Some(path));
                }

                queue.push_back(neighbor);
            }
        }
    }

    Ok(None)
}

/// Performs topological sort using Kahn's algorithm.
///
/// Backend-agnostic version using GraphBackend trait.
/// Uses `all_entity_ids()` and `fetch_outgoing()`/`fetch_incoming()` trait methods.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
///
/// # Returns
/// Sorted vector of node IDs, or error if cycle detected.
pub fn topological_sort(graph: &dyn GraphBackend) -> Result<Vec<i64>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Compute in-degree for each node
    let mut in_degree: AHashMap<i64, usize> = AHashMap::new();
    for &node in &all_ids {
        in_degree.insert(node, graph.fetch_incoming(node)?.len());
    }

    // Start with nodes having no incoming edges
    let mut queue: VecDeque<i64> = all_ids
        .iter()
        .filter(|&&node| in_degree[&node] == 0)
        .copied()
        .collect();

    let mut result = Vec::new();

    while let Some(node) = queue.pop_front() {
        result.push(node);

        for neighbor in graph.fetch_outgoing(node)? {
            let new_degree = in_degree[&neighbor] - 1;
            in_degree.insert(neighbor, new_degree);

            if new_degree == 0 {
                queue.push_back(neighbor);
            }
        }
    }

    // Check for cycles
    if result.len() != all_ids.len() {
        return Err(SqliteGraphError::validation(
            "Graph contains cycles, topological sort not possible",
        ));
    }

    Ok(result)
}

/// BFS traversal that returns nodes at exactly the specified depth.
///
/// # Arguments
/// * `graph` - Any backend implementing GraphBackend trait
/// * `start` - Starting node ID
/// * `depth` - Exact depth to reach
///
/// # Returns
/// Vector of node IDs at exactly the specified depth.
pub fn bfs(graph: &dyn GraphBackend, start: i64, depth: u32) -> Result<Vec<i64>, SqliteGraphError> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    visited.insert(start);
    queue.push_back((start, 0));

    while let Some((node, current_depth)) = queue.pop_front() {
        if current_depth == depth {
            result.push(node);
            continue;
        }

        if current_depth < depth {
            for neighbor in graph.fetch_outgoing(node)? {
                if visited.insert(neighbor) {
                    queue.push_back((neighbor, current_depth + 1));
                }
            }
        }
    }

    // Remove start node if it was added
    result.retain(|&id| id != start);

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

    #[test]
    fn test_scc_cycle() {
        let (backend, _temp) = create_backend();

        // Create cycle: 1 -> 2 -> 3 -> 1
        let mut nodes = Vec::new();
        for _ in 0..3 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "node".to_string(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
            nodes.push(id);
        }

        for i in 0..nodes.len() {
            let next = (i + 1) % nodes.len();
            backend
                .insert_edge(EdgeSpec {
                    from: nodes[i],
                    to: nodes[next],
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        let scc = strongly_connected_components(&backend).unwrap();

        assert_eq!(scc.components.len(), 1);
        assert_eq!(scc.components[0].len(), 3);
    }

    #[test]
    fn test_scc_chain() {
        let (backend, _temp) = create_backend();

        // Create chain: 1 -> 2 -> 3
        let mut nodes = Vec::new();
        for _ in 0..3 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "node".to_string(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
            nodes.push(id);
        }

        for i in 0..nodes.len() - 1 {
            backend
                .insert_edge(EdgeSpec {
                    from: nodes[i],
                    to: nodes[i + 1],
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        let scc = strongly_connected_components(&backend).unwrap();

        // Chain has no cycles, so each node is its own SCC
        assert_eq!(scc.components.len(), 3);
    }

    #[test]
    fn test_shortest_path_found() {
        let (backend, _temp) = create_backend();

        let mut nodes = Vec::new();
        for _ in 0..4 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "node".to_string(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
            nodes.push(id);
        }

        // Chain: 1 -> 2 -> 3 -> 4
        for i in 0..nodes.len() - 1 {
            backend
                .insert_edge(EdgeSpec {
                    from: nodes[i],
                    to: nodes[i + 1],
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        let path = shortest_path(&backend, nodes[0], nodes[3]).unwrap();

        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 4);
        assert_eq!(path[0], nodes[0]);
        assert_eq!(path[3], nodes[3]);
    }

    #[test]
    fn test_shortest_path_not_found() {
        let (backend, _temp) = create_backend();

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "a".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "b".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let path = shortest_path(&backend, a, b).unwrap();

        assert!(path.is_none());
    }

    #[test]
    fn test_topological_sort_dag() {
        let (backend, _temp) = create_backend();

        let mut nodes = Vec::new();
        for _ in 0..3 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "node".to_string(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
            nodes.push(id);
        }

        // DAG: 1 -> 2, 1 -> 3
        backend
            .insert_edge(EdgeSpec {
                from: nodes[0],
                to: nodes[1],
                edge_type: "links".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: nodes[0],
                to: nodes[2],
                edge_type: "links".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let sorted = topological_sort(&backend).unwrap();

        assert_eq!(sorted.len(), 3);
        // Node 0 should come first (no incoming edges)
        assert_eq!(sorted[0], nodes[0]);
    }

    #[test]
    fn test_topological_sort_cycle_fails() {
        let (backend, _temp) = create_backend();

        let mut nodes = Vec::new();
        for _ in 0..3 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "node".to_string(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
            nodes.push(id);
        }

        // Cycle: 1 -> 2 -> 3 -> 1
        for i in 0..nodes.len() {
            let next = (i + 1) % nodes.len();
            backend
                .insert_edge(EdgeSpec {
                    from: nodes[i],
                    to: nodes[next],
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        let result = topological_sort(&backend);

        assert!(result.is_err());
    }

    #[test]
    fn test_bfs_depth() {
        let (backend, _temp) = create_backend();

        // Binary tree: root -> left, right
        let root = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "root".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let left = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "left".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let right = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "right".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: root,
                to: left,
                edge_type: "links".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: root,
                to: right,
                edge_type: "links".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let depth1 = bfs(&backend, root, 1).unwrap();

        assert_eq!(depth1.len(), 2);
        assert!(depth1.contains(&left));
        assert!(depth1.contains(&right));
    }
}
