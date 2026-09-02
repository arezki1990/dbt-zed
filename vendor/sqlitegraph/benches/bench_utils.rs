//! Common benchmark utilities for backend performance comparison.
//!
//! Provides utilities for creating deterministic test graphs and running
//! fair performance comparisons between SQLite and Native backends.
//!
//! This module is consumed by many benchmark binaries; clippy reports dead
//! code per-target, but every helper here is used by at least one bench.

#![allow(
    dead_code,
    reason = "shared bench utilities used across multiple bench targets"
)]

use std::time::Duration;
use tempfile::TempDir;

use rand::{RngCore, SeedableRng};
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::{BackendKind, EdgeSpec, GraphConfig, NodeSpec, open_graph};

/// Common benchmark configuration
pub const SAMPLE_SIZE: usize = 20;
pub const WARM_UP: Duration = Duration::from_millis(300);
pub const MEASURE: Duration = Duration::from_millis(500);

/// Benchmark-owned state that guarantees the backend drops before the temp dir.
///
/// Benchmarks often need an ephemeral on-disk backend. Returning `(backend,
/// temp_dir)` from `iter_batched` is subtly wrong because pattern destructuring
/// drops the temp dir before the backend, which triggers teardown noise in
/// `V3Backend::drop`. This wrapper makes the intended drop order explicit.
pub struct BenchmarkState<B, T> {
    pub backend: B,
    pub temp_dir: T,
}

pub type V3BenchContext = BenchmarkState<V3Backend, TempDir>;

pub fn create_v3_bench_context(db_name: &str) -> V3BenchContext {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join(db_name);
    let backend = V3Backend::create(&db_path)
        .unwrap_or_else(|e| panic!("Failed to create V3 backend at {:?}: {}", db_path, e));

    V3BenchContext { backend, temp_dir }
}

/// Graph topology types for benchmarking
#[derive(Debug, Clone, Copy)]
pub enum GraphTopology {
    /// Linear chain of nodes
    Chain,
    /// Star topology (one central node)
    Star,
    /// Grid topology (2D grid)
    Grid,
    /// Random graph
    Random,
}

/// Benchmark graph specification
#[derive(Debug, Clone)]
pub struct BenchmarkGraph {
    pub node_count: usize,
    pub edge_count: usize,
    pub topology: GraphTopology,
    pub seed: u64,
}

impl BenchmarkGraph {
    /// Create a new benchmark graph specification
    pub fn new(node_count: usize, edge_count: usize, topology: GraphTopology) -> Self {
        Self {
            node_count,
            edge_count,
            topology,
            seed: 0x5F3759DF, // Deterministic seed
        }
    }
}

/// Create a temporary directory for benchmark files
pub fn create_benchmark_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory for benchmark")
}

/// Create a benchmark graph with the specified backend and return statistics
pub fn create_benchmark_graph(backend: BackendKind, spec: &BenchmarkGraph) -> BenchmarkResult {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    let config = match backend {
        BackendKind::SQLite => GraphConfig::sqlite(),
        BackendKind::Native => GraphConfig::native(),
        BackendKind::Combined => GraphConfig::combined(),
    };
    let graph = open_graph(&db_path, &config).expect("Failed to create benchmark graph");

    let start_time = std::time::Instant::now();

    // Generate nodes using individual insertions
    let mut node_ids = Vec::with_capacity(spec.node_count);
    for i in 0..spec.node_count {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({
                    "id": i,
                    "created_at": "benchmark",
                }),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Generate edges based on topology using individual insertions
    let edge_count = generate_edges(&*graph, &node_ids, spec);

    let creation_time = start_time.elapsed();

    BenchmarkResult {
        backend,
        node_count: node_ids.len(),
        edge_count,
        creation_time,
        temp_dir,
        db_path,
    }
}

/// Result of creating a benchmark graph
pub struct BenchmarkResult {
    pub backend: BackendKind,
    pub node_count: usize,
    pub edge_count: usize,
    pub creation_time: Duration,
    pub temp_dir: TempDir,
    pub db_path: std::path::PathBuf,
}

/// Generate edges based on the specified topology
fn generate_edges(
    graph: &dyn sqlitegraph::GraphBackend,
    node_ids: &[i64],
    spec: &BenchmarkGraph,
) -> usize {
    let mut edge_count = 0;
    let mut rng = rand::rngs::StdRng::seed_from_u64(spec.seed);

    match spec.topology {
        GraphTopology::Chain => {
            for i in 0..node_ids.len().min(spec.edge_count) {
                if i + 1 < node_ids.len() {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"order": i}),
                        })
                        .expect("Failed to insert edge");
                    edge_count += 1;
                }
            }
        }
        GraphTopology::Star => {
            if node_ids.is_empty() {
                return 0;
            }
            let center = node_ids[0];
            let take = (spec.edge_count + 1).min(node_ids.len()).saturating_sub(1);
            for (i, &to) in node_ids.iter().enumerate().skip(1).take(take) {
                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({"spoke": i}),
                    })
                    .expect("Failed to insert edge");
                edge_count += 1;
            }
        }
        GraphTopology::Grid => {
            let grid_size = (node_ids.len() as f64).sqrt() as usize;
            for i in 0..node_ids.len().min(spec.edge_count) {
                let row = i / grid_size;
                let col = i % grid_size;

                // Right neighbor
                if col + 1 < grid_size && i + 1 < node_ids.len() && edge_count < spec.edge_count {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "grid".to_string(),
                            data: serde_json::json!({"direction": "right"}),
                        })
                        .expect("Failed to insert edge");
                    edge_count += 1;
                }

                // Bottom neighbor
                if row + 1 < grid_size
                    && i + grid_size < node_ids.len()
                    && edge_count < spec.edge_count
                {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + grid_size],
                            edge_type: "grid".to_string(),
                            data: serde_json::json!({"direction": "down"}),
                        })
                        .expect("Failed to insert edge");
                    edge_count += 1;
                }
            }
        }
        GraphTopology::Random => {
            use rand::Rng;
            for _ in 0..spec.edge_count {
                let from_idx = rng.gen_range(0..node_ids.len());
                let mut to_idx = rng.gen_range(0..node_ids.len());
                while to_idx == from_idx {
                    to_idx = rng.gen_range(0..node_ids.len());
                }

                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[from_idx],
                        to: node_ids[to_idx],
                        edge_type: "random".to_string(),
                        data: serde_json::json!({"random_id": rng.next_u64()}),
                    })
                    .expect("Failed to insert edge");
                edge_count += 1;
            }
        }
    }

    edge_count
}

/// Common benchmark graph sizes
pub const BENCHMARK_SIZES: &[usize] = &[100, 500];

/// Small benchmark graphs for quick testing
pub const SMALL_SIZES: &[usize] = &[10, 50];

/// Medium benchmark graphs for typical performance testing
pub const MEDIUM_SIZES: &[usize] = &[1_000, 5_000];

/// Large benchmark graphs for stress testing
pub const LARGE_SIZES: &[usize] = &[10_000, 20_000];

/// In-memory CPU-only graph for performance ceiling
#[derive(Debug)]
pub struct BenchInMemoryGraph {
    pub adjacency: Vec<Vec<u32>>,
}

impl BenchInMemoryGraph {
    /// Create an in-memory graph from benchmark specification
    pub fn from_spec(spec: &BenchmarkGraph) -> Self {
        let mut adjacency = vec![Vec::new(); spec.node_count];
        let mut rng = rand::rngs::StdRng::seed_from_u64(spec.seed);

        match spec.topology {
            GraphTopology::Chain => {
                let limit = spec.node_count.min(spec.edge_count);
                for (i, adj) in adjacency.iter_mut().enumerate().take(limit) {
                    if i + 1 < spec.node_count {
                        adj.push(i as u32 + 1);
                    }
                }
            }
            GraphTopology::Star => {
                if spec.node_count > 0 {
                    for i in 1..spec.node_count.min(spec.edge_count + 1) {
                        adjacency[0].push(i as u32);
                    }
                }
            }
            GraphTopology::Random => {
                use rand::Rng;
                for _ in 0..spec.edge_count {
                    let from_idx = rng.gen_range(0..spec.node_count);
                    let mut to_idx = rng.gen_range(0..spec.node_count);
                    while to_idx == from_idx {
                        to_idx = rng.gen_range(0..spec.node_count);
                    }
                    adjacency[from_idx].push(to_idx as u32);
                }
            }
            GraphTopology::Grid => {
                let grid_size = (spec.node_count as f64).sqrt() as usize;
                let limit = spec.node_count.min(spec.edge_count);
                for (i, adj) in adjacency.iter_mut().enumerate().take(limit) {
                    let row = i / grid_size;
                    let col = i % grid_size;

                    // Right neighbor
                    if col + 1 < grid_size && i + 1 < spec.node_count {
                        adj.push((i + 1) as u32);
                    }

                    // Bottom neighbor
                    if row + 1 < grid_size && i + grid_size < spec.node_count {
                        adj.push((i + grid_size) as u32);
                    }
                }
            }
        }

        Self { adjacency }
    }

    /// Get neighbors for BFS traversal
    pub fn neighbors(&self, node: u32) -> &[u32] {
        if node as usize >= self.adjacency.len() {
            &[]
        } else {
            &self.adjacency[node as usize]
        }
    }

    /// Get node count
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }
}
