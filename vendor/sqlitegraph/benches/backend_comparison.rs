//! Criterion benchmark: curated SQLite vs Native V3 backend comparison.
//!
//! Run with: cargo bench --features native-v3 --bench backend_comparison
//!
//! This suite is intentionally curated to finish in practical time on a
//! developer workstation. It focuses on the highest-signal workload families.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;

// Import graph types
use sqlitegraph::algo::backend::{bfs_traversal, dfs_traversal, k_hop_neighbors, shortest_path};
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NodeSpec, SqliteGraphBackend};
use sqlitegraph::snapshot::SnapshotId;

mod bench_utils;
// Graph topology generators
mod graph_generators;
use bench_utils::create_v3_bench_context;

/// Hardware and environment information
#[derive(Debug, Clone)]
pub struct BenchmarkEnvironment {
    pub cpu: String,
    pub ram_gb: usize,
    pub disk_type: String,
    pub os: String,
    pub kernel: String,
    pub rust_version: String,
    pub rustc_host: String,
    pub cargo_profile: String,
    pub sqlite_version: String,
    pub sqlite_pragmas: Vec<(String, String)>,
}

impl BenchmarkEnvironment {
    pub fn detect() -> Self {
        Self {
            cpu: detect_cpu(),
            ram_gb: detect_ram(),
            disk_type: detect_disk_type(),
            os: std::env::consts::OS.to_string(),
            kernel: detect_kernel(),
            rust_version: rustc_version::version()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            rustc_host: std::env::consts::ARCH.to_string(),
            cargo_profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
            sqlite_version: rusqlite::version().to_string(),
            sqlite_pragmas: vec![
                ("journal_mode".to_string(), "WAL".to_string()),
                ("synchronous".to_string(), "NORMAL".to_string()),
                ("cache_size".to_string(), "-64000".to_string()), // 64MB
                ("mmap_size".to_string(), "268435456".to_string()), // 256MB
                ("temp_store".to_string(), "memory".to_string()),
            ],
        }
    }

    pub fn print(&self) {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║              BENCHMARK ENVIRONMENT                               ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");
        println!("  CPU:              {}", self.cpu);
        println!("  RAM:              {} GB", self.ram_gb);
        println!("  Disk Type:        {}", self.disk_type);
        println!("  Operating System: {}", self.os);
        println!("  Kernel:           {}", self.kernel);
        println!("  Rust Version:     {}", self.rust_version);
        println!("  Target Arch:      {}", self.rustc_host);
        println!("  Cargo Profile:    {}", self.cargo_profile);
        println!("  SQLite Version:   {}", self.sqlite_version);
        println!("\n  SQLite Configuration:");
        for (pragma, value) in &self.sqlite_pragmas {
            println!("    PRAGMA {} = {}", pragma, value);
        }
        println!("\n");
    }
}

fn detect_cpu() -> String {
    // Try to read from /proc/cpuinfo on Linux
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name") {
                    return line
                        .split(':')
                        .nth(1)
                        .unwrap_or("Unknown")
                        .trim()
                        .to_string();
                }
            }
        }
    }
    "Unknown".to_string()
}

fn detect_ram() -> usize {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    let kb: usize = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    return kb / 1024 / 1024; // Convert to GB
                }
            }
        }
    }
    0
}

fn detect_disk_type() -> String {
    #[cfg(target_os = "linux")]
    {
        // Check if we're on tmpfs (RAM disk) or SSD/NVMe
        if let Ok(output) = std::process::Command::new("df")
            .args(["-T", "/tmp"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return parts[1].to_string();
                }
            }
        }
    }
    "Unknown".to_string()
}

fn detect_kernel() -> String {
    // Read from /proc/version as fallback
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        return version
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
    }
    "Unknown".to_string()
}

// ============================================================================
// Benchmark Functions
// ============================================================================

/// Benchmark: BFS Traversal
fn bench_bfs_traversal(c: &mut Criterion) {
    let env = BenchmarkEnvironment::detect();
    env.print();

    let mut group = c.benchmark_group("bfs_traversal");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    for (name, nodes, edges) in &[
        ("small_random_1k_5k", 1000, 5000),
        ("medium_random_10k_50k", 10000, 50000),
    ] {
        group.throughput(Throughput::Elements(*nodes as u64));

        // Generate graph data
        let graph_data = generate_random_graph(*nodes, *edges);

        // SQLite benchmark
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    backend
                },
                |backend| {
                    let result = bfs_traversal(&backend, black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // V3 benchmark
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    ctx
                },
                |ctx| {
                    let result = bfs_traversal(&ctx.backend, black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: DFS Traversal
fn bench_dfs_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("dfs_traversal");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(10);

    for (name, nodes, edges) in &[
        ("small_random_1k_5k", 1000, 5000),
        ("medium_random_10k_50k", 10000, 50000),
    ] {
        group.throughput(Throughput::Elements(*nodes as u64));
        let graph_data = generate_random_graph(*nodes, *edges);

        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    backend
                },
                |backend| {
                    let result = dfs_traversal(&backend, black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    ctx
                },
                |ctx| {
                    let result = dfs_traversal(&ctx.backend, black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: k-hop neighbors
fn bench_k_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("k_hop_neighbors");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (name, nodes, edges, k) in &[
        ("small_k2", 1000, 5000, 2),
        ("medium_k2", 10000, 50000, 2),
        ("medium_k3", 10000, 50000, 3),
    ] {
        let graph_data = generate_random_graph(*nodes, *edges);

        group.bench_with_input(
            BenchmarkId::new("sqlite", name),
            &(graph_data.clone(), *k),
            |b, (data, k)| {
                b.iter_batched(
                    || {
                        let backend = SqliteGraphBackend::in_memory().unwrap();
                        populate_sqlite_backend(&backend, data);
                        backend
                    },
                    |backend| {
                        let result =
                            k_hop_neighbors(&backend, black_box(1), black_box(*k)).unwrap();
                        black_box(result.len());
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("v3", name),
            &(graph_data.clone(), *k),
            |b, (data, k)| {
                b.iter_batched(
                    || {
                        let ctx = create_v3_bench_context("v3.db");
                        populate_v3_backend(&ctx.backend, data);
                        ctx
                    },
                    |ctx| {
                        let result =
                            k_hop_neighbors(&ctx.backend, black_box(1), black_box(*k)).unwrap();
                        black_box(result.len());
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: Shortest path
fn bench_shortest_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("shortest_path");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (name, nodes, edges) in &[("small", 1000, 5000), ("medium", 10000, 50000)] {
        let graph_data = generate_random_graph(*nodes, *edges);
        let target = (*nodes / 10) as i64; // Target is 10% through the graph

        group.bench_with_input(
            BenchmarkId::new("sqlite", name),
            &(graph_data.clone(), target),
            |b, (data, target)| {
                b.iter_batched(
                    || {
                        let backend = SqliteGraphBackend::in_memory().unwrap();
                        populate_sqlite_backend(&backend, data);
                        backend
                    },
                    |backend| {
                        let result =
                            shortest_path(&backend, black_box(1), black_box(*target)).unwrap();
                        black_box(result.map(|p| p.len()));
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("v3", name),
            &(graph_data.clone(), target),
            |b, (data, target)| {
                b.iter_batched(
                    || {
                        let ctx = create_v3_bench_context("v3.db");
                        populate_v3_backend(&ctx.backend, data);
                        ctx
                    },
                    |ctx| {
                        let result =
                            shortest_path(&ctx.backend, black_box(1), black_box(*target)).unwrap();
                        black_box(result.map(|p| p.len()));
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: Point lookup (get_node)
fn bench_point_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("point_lookup");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (name, nodes) in &[("1k", 1000), ("10k", 10000), ("100k", 100000)] {
        let graph_data = generate_random_graph(*nodes, 0); // No edges for pure lookup test
        let target_node = (*nodes / 2) as i64;

        group.bench_with_input(
            BenchmarkId::new("sqlite", name),
            &(graph_data.clone(), target_node),
            |b, (data, target)| {
                b.iter_batched(
                    || {
                        let backend = SqliteGraphBackend::in_memory().unwrap();
                        populate_sqlite_backend(&backend, data);
                        (backend, *target)
                    },
                    |(backend, target)| {
                        let snapshot = SnapshotId::current();
                        let result = backend.get_node(snapshot, black_box(target)).unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("v3", name),
            &(graph_data.clone(), target_node),
            |b, (data, target)| {
                b.iter_batched(
                    || {
                        let ctx = create_v3_bench_context("v3.db");
                        populate_v3_backend(&ctx.backend, data);
                        (ctx, *target)
                    },
                    |(ctx, target)| {
                        let snapshot = SnapshotId::current();
                        let result = ctx.backend.get_node(snapshot, black_box(target)).unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: Fetch outgoing edges
fn bench_fetch_outgoing(c: &mut Criterion) {
    let mut group = c.benchmark_group("fetch_outgoing");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (name, nodes, edges) in &[
        ("sparse_1k_1k", 1000, 1000),  // 1 edge per node avg
        ("dense_1k_10k", 1000, 10000), // 10 edges per node
        ("sparse_10k_10k", 10000, 10000),
        ("dense_10k_100k", 10000, 100000),
    ] {
        let graph_data = generate_random_graph(*nodes, *edges);

        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    backend
                },
                |backend| {
                    let result = backend.fetch_outgoing(black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    ctx
                },
                |ctx| {
                    let result = ctx.backend.fetch_outgoing(black_box(1)).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: Batch insert performance
fn bench_batch_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_insert");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(10);

    for (name, count) in &[("100_nodes", 100), ("1k_nodes", 1000), ("10k_nodes", 10000)] {
        group.throughput(Throughput::Elements(*count as u64));

        group.bench_with_input(BenchmarkId::new("sqlite", name), count, |b, &count| {
            b.iter_batched(
                || SqliteGraphBackend::in_memory().unwrap(),
                |backend| {
                    for i in 0..count {
                        backend
                            .insert_node(NodeSpec {
                                kind: "Test".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({}),
                            })
                            .unwrap();
                    }
                    black_box(());
                },
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("v3", name), count, |b, &count| {
            b.iter_batched(
                || create_v3_bench_context("v3.db"),
                |ctx| {
                    let mut batch = ctx.backend.begin_batch();
                    for i in 0..count {
                        batch
                            .insert_node(sqlitegraph::backend::NodeSpec {
                                kind: "Test".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({}),
                            })
                            .unwrap();
                    }
                    batch.commit().unwrap();
                    black_box(());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// Helper functions
// ============================================================================

fn populate_sqlite_backend(backend: &SqliteGraphBackend, data: &GraphData) {
    for node in &data.nodes {
        backend.insert_node(node.clone()).unwrap();
    }
    for edge in &data.edges {
        backend.insert_edge(edge.clone()).unwrap();
    }
}

fn populate_v3_backend(backend: &V3Backend, data: &GraphData) {
    // Use batching for V3
    let mut batch = backend.begin_batch();
    for node in &data.nodes {
        batch
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind: node.kind.clone(),
                name: node.name.clone(),
                file_path: node.file_path.clone(),
                data: node.data.clone(),
            })
            .unwrap();
    }
    batch.commit().unwrap();

    let mut batch = backend.begin_batch();
    for edge in &data.edges {
        batch
            .insert_edge(sqlitegraph::backend::EdgeSpec {
                from: edge.from,
                to: edge.to,
                edge_type: edge.edge_type.clone(),
                data: edge.data.clone(),
            })
            .unwrap();
    }
    batch.commit().unwrap();
}

// Graph data structure for batch setup
#[derive(Clone)]
pub struct GraphData {
    pub nodes: Vec<NodeSpec>,
    pub edges: Vec<EdgeSpec>,
}

fn generate_random_graph(node_count: usize, edge_count: usize) -> GraphData {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut nodes = Vec::with_capacity(node_count);
    let mut edges = Vec::with_capacity(edge_count);

    for i in 0..node_count {
        nodes.push(NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        });
    }

    for i in 0..edge_count {
        let mut hasher = DefaultHasher::new();
        i.hash(&mut hasher);
        let hash = hasher.finish();

        let from = ((hash % node_count as u64) + 1) as i64;
        let to = (((hash >> 32) % node_count as u64) + 1) as i64;

        if from != to {
            edges.push(EdgeSpec {
                from,
                to,
                edge_type: "Edge".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    GraphData { nodes, edges }
}

// ============================================================================
// Criterion Groups
// ============================================================================

criterion_group!(
    benches,
    bench_bfs_traversal,
    bench_dfs_traversal,
    bench_k_hop,
    bench_shortest_path,
    bench_point_lookup,
    bench_fetch_outgoing,
    bench_batch_insert,
);

criterion_main!(benches);
