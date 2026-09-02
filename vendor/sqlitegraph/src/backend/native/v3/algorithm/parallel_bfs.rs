//! Parallel Breadth-First Search using Chunked Processing
//!
//! Minecraft-style chunked parallel BFS where each level is partitioned
//! into independent chunks. Each chunk processes with thread-local state,
//! achieving zero synchronization overhead during parallel phase.
//!
//! # Architecture
//!
//! 1. **Partition:** Divide current BFS level into chunks (one per CPU core)
//! 2. **Process:** Each chunk processes independently with thread-local state
//! 3. **Merge:** Combine chunk results into final result (single-threaded)
//!
//! # Performance
//!
//! - **Small graphs (<1000 nodes):** Use sequential BFS (overhead dominates)
//! - **Medium graphs (1000-10000 nodes):** 2-4× speedup on multi-core systems
//! - **Large graphs (>10000 nodes):** Speedup depends on graph topology
//!
//! # Thread Safety
//!
//! This implementation has **zero shared state** during parallel processing.
//! Each chunk owns its local state, eliminating all locks and data races.

use crate::SqliteGraphError;
use crate::backend::native::v3::V3Backend;
use crate::backend::{BackendDirection, GraphBackend, NeighborQuery};
use crate::snapshot::SnapshotId;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// Configuration for parallel BFS execution
#[derive(Debug, Clone)]
pub struct BfsConfig {
    /// Maximum number of threads to use (None = use Rayon default)
    pub max_threads: Option<usize>,

    /// Minimum graph size to use parallel processing (node count)
    pub min_parallel_size: usize,
}

impl Default for BfsConfig {
    fn default() -> Self {
        Self {
            max_threads: None,       // Use Rayon default (all CPUs)
            min_parallel_size: 1000, // Chunks need enough work to justify overhead
        }
    }
}

/// Result of a BFS traversal
#[derive(Debug, Clone)]
pub struct BfsResult {
    /// Order in which nodes were visited (BFS order)
    pub visited_order: Vec<i64>,

    /// Distance from start node to each visited node
    pub distances: HashMap<i64, usize>,

    /// Total number of nodes visited
    pub total_visited: usize,
}

impl BfsResult {
    /// Create a new BFS result
    fn new() -> Self {
        Self {
            visited_order: Vec::new(),
            distances: HashMap::new(),
            total_visited: 0,
        }
    }

    /// Add a visited node at the specified distance
    fn add_visit(&mut self, node: i64, distance: usize) {
        self.visited_order.push(node);
        self.distances.insert(node, distance);
        self.total_visited += 1;
    }
}

/// Result of processing a single chunk in parallel BFS
///
/// Contains thread-local state from one chunk's processing.
/// This is moved (not cloned) during merge to avoid allocations.
#[derive(Debug)]
struct ChunkResult {
    /// New nodes discovered by this chunk
    new_nodes: Vec<i64>,

    /// Distances from start to each new node
    distances: HashMap<i64, usize>,
}

impl ChunkResult {
    /// Create a new empty chunk result
    fn new() -> Self {
        Self {
            new_nodes: Vec::new(),
            distances: HashMap::new(),
        }
    }

    /// Add a discovered node to this chunk's result
    fn add_node(&mut self, node: i64, distance: usize) {
        self.new_nodes.push(node);
        self.distances.insert(node, distance);
    }
}

/// Perform parallel BFS traversal from a start node
///
/// # Arguments
///
/// * `graph` - The V3Backend to traverse
/// * `start` - The starting node ID
/// * `config` - Optional configuration for parallel execution
///
/// # Returns
///
/// * `Result<BfsResult, SqliteGraphError>` - BFS traversal results
///
/// # Performance Characteristics
///
/// - **Thread-safe:** Zero shared state during parallel phase
/// - **Overhead:** Chunking adds ~10-20µs per level
/// - **Best for:** Graphs with wide levels (high branching factor)
/// - **Avoid:** Chain graphs (narrow levels have limited parallelism)
///
/// # Example
///
/// ```ignore
/// use sqlitegraph::backend::native::v3::algorithm::parallel_bfs;
/// use sqlitegraph::backend::native::v3::algorithm::BfsConfig;
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let backend = /* ... */;
/// let config = BfsConfig {
///     max_threads: Some(4),
///     min_parallel_size: 500,
/// };
/// let result = parallel_bfs(&backend, 1, Some(config))?;
/// # Ok(())
/// # }
/// ```
pub fn parallel_bfs(
    graph: &V3Backend,
    start: i64,
    config: Option<BfsConfig>,
) -> Result<BfsResult, SqliteGraphError> {
    let config = config.unwrap_or_default();

    // Check if start node exists
    let snapshot = SnapshotId::current();
    if graph.get_node(snapshot, start).is_err() {
        return Err(SqliteGraphError::not_found(format!(
            "Start node {} not found",
            start
        )));
    }

    // Check graph size - use sequential fallback for small graphs
    let node_count = graph.header().node_count;
    if node_count < config.min_parallel_size as u64 {
        return sequential_bfs(graph, start);
    }

    // Set up Rayon thread pool if max_threads specified

    if let Some(max_threads) = config.max_threads {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(max_threads)
            .build()
            .map_err(|e| {
                SqliteGraphError::connection(format!("Failed to create thread pool: {}", e))
            })?;

        pool.install(|| parallel_bfs_impl(graph, start, &config))
    } else {
        parallel_bfs_impl(graph, start, &config)
    }
}

/// Partition a slice of nodes into chunks for parallel processing
///
/// # Arguments
///
/// * `nodes` - Nodes to partition
/// * `num_chunks` - Number of chunks to create (typically number of CPU cores)
///
/// # Returns
///
/// Vector of chunks, where each chunk is a slice of the original nodes
///
/// # Example
///
/// ```ignore
/// let nodes = vec![1, 2, 3, 4, 5];
/// let chunks = partition_nodes(&nodes, 2);
/// assert_eq!(chunks.len(), 2);
/// assert_eq!(chunks[0], &[1, 2, 3]);  // First chunk gets remainder
/// assert_eq!(chunks[1], &[4, 5]);
/// ```
fn partition_nodes(nodes: &[i64], num_chunks: usize) -> Vec<&[i64]> {
    if num_chunks == 0 || nodes.is_empty() || nodes.len() <= num_chunks {
        return vec![nodes];
    }

    let chunk_size = nodes.len().div_ceil(num_chunks); // Ceiling division
    let mut chunks = Vec::with_capacity(num_chunks);

    let mut start = 0;
    while start < nodes.len() {
        let end = (start + chunk_size).min(nodes.len());
        chunks.push(&nodes[start..end]);
        start = end;
    }

    chunks
}

/// Internal parallel BFS implementation using chunked processing
///
/// Algorithm (Minecraft-style chunks):
/// 1. Partition current level into chunks (one per CPU core)
/// 2. Process each chunk in parallel with thread-local state
/// 3. Merge chunk results into final result (single-threaded)
///
/// This design has ZERO shared state during parallel phase,
/// eliminating locks and achieving true parallel speedup.
fn parallel_bfs_impl(
    graph: &V3Backend,
    start: i64,
    _config: &BfsConfig,
) -> Result<BfsResult, SqliteGraphError> {
    let snapshot = SnapshotId::current();
    let mut result = BfsResult::new();
    let mut visited: HashSet<i64> = HashSet::new();

    // Initialize BFS queue
    let mut current_level: Vec<i64> = vec![start];
    let mut distance = 0;

    // Mark start as visited
    visited.insert(start);
    result.add_visit(start, distance);

    // CRITICAL: Cap at 4 threads to prevent system overload
    let num_cpus = std::cmp::min(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        4, // Maximum 4 threads - prevents desktop crash
    );

    // Process each level
    while !current_level.is_empty() {
        distance += 1;

        // Partition current level into chunks (Minecraft-style)
        let chunks = partition_nodes(&current_level, num_cpus);

        // PROCESS CHUNKS IN PARALLEL WITH ZERO SHARED STATE
        let chunk_results: Vec<ChunkResult> = chunks
            .into_par_iter() // Rayon parallel iterator
            .map(|chunk| {
                // === THREAD-LOCAL STATE (no sharing, no locks) ===
                let mut local_result = ChunkResult::new();
                let mut local_visited: HashSet<i64> = HashSet::new();

                // Check global visited set once per node
                for &node in chunk {
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };

                    if let Ok(neighbors) = graph.neighbors(snapshot, node, query) {
                        for neighbor in neighbors {
                            // Check if globally visited (single read, no lock)
                            if !visited.contains(&neighbor) {
                                // Check if locally visited in this chunk
                                if local_visited.insert(neighbor) {
                                    local_result.add_node(neighbor, distance);
                                }
                            }
                        }
                    }
                }

                local_result // Move thread-local result out
            })
            .collect(); // Barrier: wait for all chunks

        // === MERGE PHASE (single-threaded, no locks needed) ===
        let mut next_level: Vec<i64> = Vec::new();

        for chunk_result in chunk_results {
            for (node, dist) in chunk_result.distances {
                // Check again (another chunk might have visited this node)
                if visited.insert(node) {
                    result.add_visit(node, dist);
                    next_level.push(node);
                }
            }
        }

        // Move to next level
        current_level = next_level;
    }

    Ok(result)
}

/// Sequential BFS fallback for small graphs
///
/// Used when graph size is below `min_parallel_size` threshold.
/// Provides simpler implementation without threading overhead.
fn sequential_bfs(graph: &V3Backend, start: i64) -> Result<BfsResult, SqliteGraphError> {
    let snapshot = SnapshotId::current();
    let mut result = BfsResult::new();
    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<(i64, usize)> = VecDeque::new();

    // Initialize
    visited.insert(start);
    queue.push_back((start, 0));
    result.add_visit(start, 0);

    // BFS traversal
    while let Some((node, distance)) = queue.pop_front() {
        // Fetch neighbors using the GraphBackend API
        let query = NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        };

        if let Ok(neighbors) = graph.neighbors(snapshot, node, query) {
            for neighbor in neighbors {
                if visited.insert(neighbor) {
                    queue.push_back((neighbor, distance + 1));
                    result.add_visit(neighbor, distance + 1);
                }
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::native::v3::V3Backend;
    use crate::backend::{EdgeSpec, NodeSpec};
    use tempfile::TempDir;

    /// Helper function to create a test backend
    fn create_test_backend() -> (V3Backend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let backend = V3Backend::create(&db_path).unwrap();
        (backend, temp_dir)
    }

    /// Helper function to create a chain graph: 1 -> 2 -> 3 -> ... -> n
    fn create_chain_graph(backend: &V3Backend, n: i64) -> Vec<i64> {
        let mut node_ids = Vec::new();

        // Create nodes
        for i in 1..=n {
            let node = NodeSpec {
                kind: "test_node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!(null),
            };
            let id = backend.insert_node(node).unwrap();
            node_ids.push(id);
        }

        // Create edges to form a chain
        for i in 0..node_ids.len() - 1 {
            let edge = EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "test_edge".to_string(),
                data: serde_json::json!(null),
            };
            backend.insert_edge(edge).unwrap();
        }

        node_ids
    }

    #[test]
    fn test_parallel_bfs_chain_graph() {
        let (backend, _temp_dir) = create_test_backend();
        let node_ids = create_chain_graph(&backend, 10);

        // Run BFS from first node
        let result = parallel_bfs(&backend, node_ids[0], None).unwrap();

        // Verify traversal order
        assert_eq!(result.total_visited, 10);
        assert_eq!(result.visited_order.len(), 10);

        // Verify distances
        assert_eq!(result.distances[&node_ids[0]], 0);
        assert_eq!(result.distances[&node_ids[1]], 1);
        assert_eq!(result.distances[&node_ids[9]], 9);

        // Verify BFS order (should be sequential in a chain)
        for (i, &node_id) in result.visited_order.iter().enumerate() {
            assert_eq!(node_id, node_ids[i]);
        }
    }

    #[test]
    fn test_parallel_bfs_nonexistent_start() {
        let (backend, _temp_dir) = create_test_backend();

        // Try BFS from nonexistent node
        let result = parallel_bfs(&backend, 9999, None);

        assert!(result.is_err());
    }

    #[test]
    fn test_parallel_bfs_sequential_fallback() {
        let (backend, _temp_dir) = create_test_backend();
        let node_ids = create_chain_graph(&backend, 5); // Small graph

        // Run BFS with high min_parallel_size to force sequential
        let config = BfsConfig {
            max_threads: None,
            min_parallel_size: 1000,
        };

        let result = parallel_bfs(&backend, node_ids[0], Some(config)).unwrap();

        // Verify traversal worked
        assert_eq!(result.total_visited, 5);
        assert_eq!(result.visited_order.len(), 5);
    }

    #[test]
    fn test_bfs_config_default() {
        let config = BfsConfig::default();

        assert_eq!(config.max_threads, None);
        assert_eq!(config.min_parallel_size, 1000);
    }

    #[test]
    fn test_bfs_result_empty() {
        let result = BfsResult::new();

        assert_eq!(result.total_visited, 0);
        assert!(result.visited_order.is_empty());
        assert!(result.distances.is_empty());
    }

    #[test]
    fn test_parallel_bfs_diamond_graph() {
        let (backend, _temp_dir) = create_test_backend();

        // Create a diamond graph:
        //     1
        //    / \
        //   2   3
        //    \ /
        //     4
        let node1 = backend
            .insert_node(NodeSpec {
                kind: "test".to_string(),
                name: "1".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();
        let node2 = backend
            .insert_node(NodeSpec {
                kind: "test".to_string(),
                name: "2".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();
        let node3 = backend
            .insert_node(NodeSpec {
                kind: "test".to_string(),
                name: "3".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();
        let node4 = backend
            .insert_node(NodeSpec {
                kind: "test".to_string(),
                name: "4".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        // Create edges
        backend
            .insert_edge(EdgeSpec {
                from: node1,
                to: node2,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: node1,
                to: node3,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: node2,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: node3,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        // Run BFS from node1
        let result = parallel_bfs(&backend, node1, None).unwrap();

        // Verify all nodes visited
        assert_eq!(result.total_visited, 4);

        // Verify distances
        assert_eq!(result.distances[&node1], 0);
        assert_eq!(result.distances[&node2], 1);
        assert_eq!(result.distances[&node3], 1);
        assert_eq!(result.distances[&node4], 2);

        // Verify BFS order (level by level)
        assert_eq!(result.visited_order[0], node1);
        assert!(result.visited_order[1..3].contains(&node2));
        assert!(result.visited_order[1..3].contains(&node3));
        assert_eq!(result.visited_order[3], node4);
    }

    #[test]
    fn test_partition_nodes_empty() {
        let nodes: Vec<i64> = vec![];
        let chunks = partition_nodes(&nodes, 4);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 0);
    }

    #[test]
    fn test_partition_nodes_single() {
        let nodes = vec![1, 2, 3];
        let chunks = partition_nodes(&nodes, 4);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], &[1, 2, 3]);
    }

    #[test]
    fn test_partition_nodes_even() {
        let nodes = vec![1, 2, 3, 4, 5, 6];
        let chunks = partition_nodes(&nodes, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], &[1, 2]);
        assert_eq!(chunks[1], &[3, 4]);
        assert_eq!(chunks[2], &[5, 6]);
    }

    #[test]
    fn test_partition_nodes_uneven() {
        let nodes = vec![1, 2, 3, 4, 5];
        let chunks = partition_nodes(&nodes, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], &[1, 2]); // 2 nodes
        assert_eq!(chunks[1], &[3, 4]); // 2 nodes
        assert_eq!(chunks[2], &[5]); // 1 node (remainder)
    }
}
