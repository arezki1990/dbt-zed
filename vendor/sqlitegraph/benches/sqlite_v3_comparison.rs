//! Phase 1 benchmark: SQLite vs Native V3 backend.
//!
//! Run with: cargo bench --features native-v3 --bench sqlite_v3_comparison
//!
//! Minimal but useful benchmark suite comparing SQLite and Native V3
//! on supported operations only.
//!
//! EXCLUDED FROM BENCHMARK (unsupported on V3):
//! - pattern_search (returns Unsupported error)
//! - snapshot_import (returns Unsupported error)
//!
//! SEMANTIC DIFFERENCES (documented inline):
//! - query_nodes_by_name_pattern: V3 uses substring matching, SQLite uses GLOB
//! - query_nodes_by_kind: V3 uses O(n) scan, SQLite uses indexed lookup

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tempfile::TempDir;

use rand::{Rng, SeedableRng};
use sqlitegraph::{
    backend::native::v3::{KvValue, V3Backend},
    backend::{
        BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec, SqliteGraphBackend,
    },
    graph::SqliteGraph,
    snapshot::SnapshotId,
};

mod bench_utils;
use bench_utils::create_v3_bench_context;
mod graph_generators;
use graph_generators::*;

/// Benchmark sizes for Phase 1
const SIZES: &[(&str, usize, usize)] = &[
    ("small", 1_000, 5_000),    // 1K nodes, 5K edges
    ("medium", 10_000, 50_000), // 10K nodes, 50K edges
];

// ============================================================================
// A. WRITE PATH BENCHMARKS
// ============================================================================

/// Benchmark: Node insertion throughput
fn bench_insert_nodes(c: &mut Criterion) {
    let mut group = c.benchmark_group("write/insert_nodes");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, _edges) in SIZES {
        let graph_data = GraphTopology::Chain.generate(*nodes, 0);

        group.throughput(Throughput::Elements(*nodes as u64));

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    (backend, data.nodes.clone())
                },
                |(backend, nodes)| {
                    for spec in nodes {
                        black_box(backend.insert_node(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    (ctx, data.nodes.clone())
                },
                |(ctx, nodes)| {
                    let backend = &ctx.backend;
                    for spec in nodes {
                        black_box(backend.insert_node(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: Edge insertion throughput
fn bench_insert_edges(c: &mut Criterion) {
    let mut group = c.benchmark_group("write/insert_edges");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        group.throughput(Throughput::Elements(*edges as u64));

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    // Insert nodes first (not measured)
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    (backend, data.edges.clone())
                },
                |(backend, edges)| {
                    for spec in edges {
                        black_box(backend.insert_edge(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    (ctx, data.edges.clone())
                },
                |(ctx, edges)| {
                    let backend = &ctx.backend;
                    for spec in edges {
                        black_box(backend.insert_edge(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// B. READ PATH BENCHMARKS
// ============================================================================

/// Benchmark: Point lookup (get_node by ID)
fn bench_get_node(c: &mut Criterion) {
    let mut group = c.benchmark_group("read/get_node");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    // Pick a node ID in the middle
                    let target_id = (data.node_count / 2) as i64;
                    (backend, target_id)
                },
                |(backend, target_id)| {
                    black_box(backend.get_node(SnapshotId::current(), target_id).ok());
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    let target_id = (data.node_count / 2) as i64;
                    (ctx, target_id)
                },
                |(ctx, target_id)| {
                    let backend = &ctx.backend;
                    black_box(backend.get_node(SnapshotId::current(), target_id).ok());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: Point lookup (get_node by ID) - WARM cache variant
///
/// This measures performance when the page cache is warm (backend reused
/// across iterations). This represents the steady-state performance after
/// the working set has been loaded into cache.
fn bench_warm_get_node(c: &mut Criterion) {
    let mut group = c.benchmark_group("read/warm_get_node");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            // Setup: create backend once
            let backend = SqliteGraphBackend::in_memory().unwrap();
            for spec in &data.nodes {
                backend.insert_node(spec.clone()).unwrap();
            }
            for spec in &data.edges {
                backend.insert_edge(spec.clone()).unwrap();
            }
            let node_count = data.node_count;
            let mut idx = 0;

            // Warm cache: reuse same backend for all iterations
            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                black_box(backend.get_node(SnapshotId::current(), target_id).ok());
            });
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            let temp = TempDir::new().unwrap();
            let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
            for spec in &data.nodes {
                backend.insert_node(spec.clone()).unwrap();
            }
            for spec in &data.edges {
                backend.insert_edge(spec.clone()).unwrap();
            }
            let node_count = data.node_count;
            let mut idx = 0;

            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                black_box(backend.get_node(SnapshotId::current(), target_id).ok());
            });
        });
    }

    group.finish();
}

/// Benchmark: Neighbor fetch
fn bench_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("read/neighbors");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    let target_id = (data.node_count / 2) as i64;
                    (backend, target_id)
                },
                |(backend, target_id)| {
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), target_id, query)
                            .ok(),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    let target_id = (data.node_count / 2) as i64;
                    (ctx, target_id)
                },
                |(ctx, target_id)| {
                    let backend = &ctx.backend;
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), target_id, query)
                            .ok(),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: Neighbor fetch - WARM cache variant
///
/// This measures performance when the page cache is warm (backend reused
/// across iterations).
fn bench_warm_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("read/warm_neighbors");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            let backend = SqliteGraphBackend::in_memory().unwrap();
            for spec in &data.nodes {
                backend.insert_node(spec.clone()).unwrap();
            }
            for spec in &data.edges {
                backend.insert_edge(spec.clone()).unwrap();
            }
            let node_count = data.node_count;
            let mut idx = 0;

            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                let query = NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                };
                black_box(
                    backend
                        .neighbors(SnapshotId::current(), target_id, query)
                        .ok(),
                );
            });
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            let temp = TempDir::new().unwrap();
            let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
            for spec in &data.nodes {
                backend.insert_node(spec.clone()).unwrap();
            }
            for spec in &data.edges {
                backend.insert_edge(spec.clone()).unwrap();
            }
            let node_count = data.node_count;
            let mut idx = 0;

            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                let query = NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                };
                black_box(
                    backend
                        .neighbors(SnapshotId::current(), target_id, query)
                        .ok(),
                );
            });
        });
    }

    group.finish();
}

// ============================================================================
// D. REOPEN / COLD-START BENCHMARKS
// ============================================================================

/// Benchmark: Reopen cost (time to open existing database)
fn bench_reopen_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("reopen/cost");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // SQLite (in-memory doesn't persist, use file)
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let temp = TempDir::new().unwrap();
                    let db_path = temp.path().join("sqlite.db");

                    // Setup: create and populate database
                    {
                        let graph = SqliteGraph::open(&db_path).unwrap();
                        let backend = SqliteGraphBackend::from_graph(graph);
                        for spec in data.nodes.clone() {
                            backend.insert_node(spec).unwrap();
                        }
                        for spec in data.edges.clone() {
                            backend.insert_edge(spec).unwrap();
                        }
                    }

                    (temp, db_path, data.node_count)
                },
                |(temp, db_path, _node_count)| {
                    // Measure: reopen time
                    let graph = SqliteGraph::open(&db_path).unwrap();
                    black_box(SqliteGraphBackend::from_graph(graph));
                    drop(temp); // Cleanup
                },
                criterion::BatchSize::PerIteration,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let temp = TempDir::new().unwrap();
                    let db_path = temp.path().join("v3.db");

                    // Setup: create and populate database
                    {
                        let backend = V3Backend::create(&db_path).unwrap();
                        for spec in data.nodes.clone() {
                            backend.insert_node(spec).unwrap();
                        }
                        for spec in data.edges.clone() {
                            backend.insert_edge(spec).unwrap();
                        }
                        backend.flush().unwrap();
                    }

                    (temp, db_path, data.node_count)
                },
                |(temp, db_path, _node_count)| {
                    // Measure: reopen time
                    let backend = V3Backend::open(&db_path).unwrap();
                    black_box(&backend);
                    drop(backend);
                    drop(temp);
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

/// Benchmark: Cold neighbors query after reopen
fn bench_cold_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("reopen/cold_neighbors");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        // V3 only (SQLite in-memory doesn't have reopen semantics)
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let temp = TempDir::new().unwrap();
                    let db_path = temp.path().join("v3.db");
                    let target_id = (data.node_count / 2) as i64;

                    // Setup: create and populate
                    {
                        let backend = V3Backend::create(&db_path).unwrap();
                        for spec in data.nodes.clone() {
                            backend.insert_node(spec).unwrap();
                        }
                        for spec in data.edges.clone() {
                            backend.insert_edge(spec).unwrap();
                        }
                        backend.flush().unwrap();
                    }

                    (temp, db_path, target_id)
                },
                |(temp, db_path, target_id)| {
                    // Measure: reopen + first neighbors query
                    let backend = V3Backend::open(&db_path).unwrap();
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), target_id, query)
                            .unwrap(),
                    );
                    drop(backend);
                    drop(temp);
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ============================================================================
// E. KV BENCHMARKS (V3 only - V3 has dedicated KV store)
// ============================================================================

/// Benchmark: KV set operations (V3 native KV store)
fn bench_kv_set(c: &mut Criterion) {
    let mut group = c.benchmark_group("kv/set");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, _edges) in SIZES {
        group.throughput(Throughput::Elements(*nodes as u64));

        group.bench_with_input(BenchmarkId::new("v3", name), nodes, |b, &count| {
            b.iter_batched(
                || create_v3_bench_context("v3.db"),
                |ctx| {
                    let backend = &ctx.backend;
                    for i in 0..count {
                        let key = format!("key_{}", i).into_bytes();
                        let _: () = backend.kv_set_v3(key, KvValue::Integer(i as i64), None);
                        black_box(());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: KV get operations
fn bench_kv_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("kv/get");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    for (name, nodes, _edges) in SIZES {
        group.bench_with_input(BenchmarkId::new("v3", name), nodes, |b, &count| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;

                    // Pre-populate KV store
                    for i in 0..count {
                        let key = format!("key_{}", i).into_bytes();
                        backend.kv_set_v3(key, KvValue::Integer(i as i64), None);
                    }

                    let target_key = format!("key_{}", count / 2);
                    (ctx, target_key)
                },
                |(ctx, target_key)| {
                    let backend = &ctx.backend;
                    black_box(backend.kv_get_v3(SnapshotId::current(), target_key.as_bytes()));
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: KV retrieval after reopen
fn bench_kv_reopen(c: &mut Criterion) {
    let mut group = c.benchmark_group("kv/reopen");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, _edges) in SIZES {
        group.bench_with_input(BenchmarkId::new("v3", name), nodes, |b, &count| {
            b.iter_batched(
                || {
                    let temp = TempDir::new().unwrap();
                    let db_path = temp.path().join("v3.db");

                    // Setup: create and populate KV
                    {
                        let backend = V3Backend::create(&db_path).unwrap();
                        for i in 0..count {
                            let key = format!("key_{}", i).into_bytes();
                            backend.kv_set_v3(key, KvValue::Integer(i as i64), None);
                        }
                        backend.flush().unwrap();
                    }

                    let target_key = format!("key_{}", count / 2);
                    (temp, db_path, target_key)
                },
                |(temp, db_path, target_key)| {
                    let backend = V3Backend::open(&db_path).unwrap();
                    black_box(backend.kv_get_v3(SnapshotId::current(), target_key.as_bytes()));
                    drop(backend);
                    drop(temp);
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ============================================================================
// F. QUERY HELPER BENCHMARKS (with semantic notes)
// ============================================================================

/// Benchmark: query_nodes_by_kind
///
/// SEMANTIC NOTE:
/// - SQLite: Uses indexed query (O(log n))
/// - V3: Uses O(n) scan (current implementation)
///
/// Results are correct, but V3 will be slower for large graphs.
fn bench_query_by_kind(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/by_kind");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    // Use smaller sizes for O(n) scan benchmark
    let sizes = &[("tiny", 100, 500), ("small", 1_000, 5_000)];

    for (name, nodes, edges) in sizes {
        let graph_data = generate_mixed_kind_graph(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    backend
                },
                |backend| {
                    let _result = backend.query_nodes_by_kind(SnapshotId::current(), "TargetKind");
                    let _ = black_box(_result);
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    ctx
                },
                |ctx| {
                    let backend = &ctx.backend;
                    let _ =
                        black_box(backend.query_nodes_by_kind(SnapshotId::current(), "TargetKind"));
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark: query_nodes_by_name_pattern
///
/// SEMANTIC NOTE:
/// - SQLite: Uses SQL GLOB pattern matching (wildcards: *, ?, [chars])
/// - V3: Uses case-sensitive substring matching
///
/// Results are NOT directly comparable due to semantic difference!
fn bench_query_by_name_pattern(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/by_name_pattern");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    // Use smaller sizes for O(n) scan benchmark
    let sizes = &[("tiny", 100, 500), ("small", 1_000, 5_000)];

    for (name, nodes, edges) in sizes {
        let graph_data = generate_named_graph(*nodes, *edges);

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    backend
                },
                |backend| {
                    // SQLite GLOB prefix pattern: "target*" matches "target_node_0", etc.
                    let _ = black_box(
                        backend.query_nodes_by_name_pattern(SnapshotId::current(), "target*"),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    ctx
                },
                |ctx| {
                    let backend = &ctx.backend;
                    // V3 prefix match: "target*" matches "target_node_0", etc.
                    // Same semantics as SQLite GLOB "target*"
                    let _ = black_box(
                        backend.query_nodes_by_name_pattern(SnapshotId::current(), "target*"),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// FILE SIZE REPORTING
// ============================================================================

/// Report on-disk file sizes after data insertion
///
/// NOTE: This is informational only, not a Criterion benchmark
/// Run with: cargo test --features native-v3 --benches file_size_reporting -- --nocapture
#[cfg(test)]
mod file_size_tests {

    #[test]
    #[ignore] // Run explicitly with cargo test --ignored
    fn report_file_sizes() {
        println!("\n=== FILE SIZE REPORT: SQLite vs V3 ===\n");

        let (nodes, edges) = (10_000, 50_000);
        let graph_data = GraphTopology::Random.generate(nodes, edges);

        // SQLite
        {
            let temp = TempDir::new().unwrap();
            let db_path = temp.path().join("sqlite.db");
            let graph = SqliteGraph::open(&db_path).unwrap();
            let backend = SqliteGraphBackend::from_graph(graph);

            for spec in graph_data.nodes.clone() {
                backend.insert_node(spec).unwrap();
            }
            for spec in graph_data.edges.clone() {
                backend.insert_edge(spec).unwrap();
            }

            let sqlite_size = metadata(&db_path).unwrap().len();
            println!(
                "SQLite ({nodes} nodes, {edges} edges): {} bytes ({:.2} MB)",
                sqlite_size,
                sqlite_size as f64 / 1024.0 / 1024.0
            );

            // Check for WAL file
            let wal_path = db_path.with_extension("db-wal");
            if wal_path.exists() {
                let wal_size = metadata(&wal_path).unwrap().len();
                println!(
                    "  SQLite WAL: {} bytes ({:.2} MB)",
                    wal_size,
                    wal_size as f64 / 1024.0 / 1024.0
                );
            }
        }

        // V3
        {
            let temp = TempDir::new().unwrap();
            let db_path = temp.path().join("v3.db");
            let backend = V3Backend::create(&db_path).unwrap();

            for spec in graph_data.nodes.clone() {
                backend.insert_node(spec).unwrap();
            }
            for spec in graph_data.edges.clone() {
                backend.insert_edge(spec).unwrap();
            }

            let v3_size = metadata(&db_path).unwrap().len();
            println!(
                "V3 ({nodes} nodes, {edges} edges): {} bytes ({:.2} MB)",
                v3_size,
                v3_size as f64 / 1024.0 / 1024.0
            );

            // Check for WAL file
            let wal_path = db_path.with_extension("v3wal");
            if wal_path.exists() {
                let wal_size = metadata(&wal_path).unwrap().len();
                println!(
                    "  V3 WAL: {} bytes ({:.2} MB)",
                    wal_size,
                    wal_size as f64 / 1024.0 / 1024.0
                );
            }

            // Check for checkpoint file
            let checkpoint_path = db_path.with_extension("v3checkpoint");
            if checkpoint_path.exists() {
                let checkpoint_size = metadata(&checkpoint_path).unwrap().len();
                println!(
                    "  V3 Checkpoint: {} bytes ({:.2} MB)",
                    checkpoint_size,
                    checkpoint_size as f64 / 1024.0 / 1024.0
                );
            }
        }

        println!("\n=== END FILE SIZE REPORT ===\n");
    }
}

// ============================================================================
// TRAVERSAL BENCHMARKS (verify parity)
// ============================================================================

use sqlitegraph::algo::backend::bfs_traversal;

/// Benchmark: BFS traversal
fn bench_bfs(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal/bfs");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for (name, nodes, edges) in SIZES {
        let graph_data = GraphTopology::Random.generate(*nodes, *edges);

        group.throughput(Throughput::Elements(*nodes as u64));

        // SQLite
        group.bench_with_input(BenchmarkId::new("sqlite", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    backend
                },
                |backend| {
                    let result = bfs_traversal(&backend, 1).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::LargeInput,
            );
        });

        // V3
        group.bench_with_input(BenchmarkId::new("v3", name), &graph_data, |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let backend = &ctx.backend;
                    for spec in data.nodes.clone() {
                        backend.insert_node(spec).unwrap();
                    }
                    for spec in data.edges.clone() {
                        backend.insert_edge(spec).unwrap();
                    }
                    ctx
                },
                |ctx| {
                    let backend = &ctx.backend;
                    let result = bfs_traversal(&backend, 1).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Generate graph with mixed kinds for query testing
fn generate_mixed_kind_graph(nodes: usize, edges: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: if i % 10 == 0 {
                "TargetKind"
            } else {
                "OtherKind"
            }
            .to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })
        .collect();

    let mut edge_specs = Vec::new();
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    for _ in 0..edges.min(nodes * 10) {
        let from = (rng.r#gen::<usize>() % nodes + 1) as i64;
        let to = (rng.r#gen::<usize>() % nodes + 1) as i64;
        if from != to {
            edge_specs.push(EdgeSpec {
                from,
                to,
                edge_type: "Edge".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    let edge_count = edge_specs.len();
    GraphData {
        nodes: node_specs,
        edges: edge_specs,
        topology: GraphTopology::Random,
        node_count: nodes,
        edge_count,
    }
}

/// Generate graph with names containing "target" for pattern testing
fn generate_named_graph(nodes: usize, edges: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: if i % 10 == 0 {
                format!("target_node_{}", i)
            } else {
                format!("node_{}", i)
            },
            file_path: None,
            data: serde_json::json!({"id": i}),
        })
        .collect();

    let mut edge_specs = Vec::new();
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    for _ in 0..edges.min(nodes * 10) {
        let from = (rng.r#gen::<usize>() % nodes + 1) as i64;
        let to = (rng.r#gen::<usize>() % nodes + 1) as i64;
        if from != to {
            edge_specs.push(EdgeSpec {
                from,
                to,
                edge_type: "Edge".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    let edge_count = edge_specs.len();
    GraphData {
        nodes: node_specs,
        edges: edge_specs,
        topology: GraphTopology::Random,
        node_count: nodes,
        edge_count,
    }
}

// ============================================================================
// CRITERION ENTRY POINT
// ============================================================================

criterion_group!(
    benches,
    // Write path
    bench_insert_nodes,
    bench_insert_edges,
    // Read path (cold - fresh instance per sample)
    bench_get_node,
    bench_neighbors,
    // Read path (warm - persistent instance, cache is warm)
    bench_warm_get_node,
    bench_warm_neighbors,
    // Traversal
    bench_bfs,
    // Reopen
    bench_reopen_cost,
    bench_cold_neighbors,
    // KV
    bench_kv_set,
    bench_kv_get,
    bench_kv_reopen,
    // Query helpers
    bench_query_by_kind,
    bench_query_by_name_pattern,
);

criterion_main!(benches);
