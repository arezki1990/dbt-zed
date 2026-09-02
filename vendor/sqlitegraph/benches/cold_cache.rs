//! Cold Cache Benchmarks
//!
//! Run with: cargo bench --features v3-bench -- cold_cache
//!
//! Benchmarks V3 backend performance with cold OS page cache.
//! Measures BFS traversal and point lookups after dropping Linux page caches.
//!
//! NOTE: Requires sudo/root privileges to drop caches.
//! Falls back gracefully to warm cache measurements if drop_caches() fails.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

mod bench_utils;
use bench_utils::BenchmarkState;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;

// Benchmark timing constants
const MEASURE: std::time::Duration = std::time::Duration::from_millis(500);
const WARM_UP: std::time::Duration = std::time::Duration::from_millis(300);

// ============================================================================
// DROP CACHES UTILITY
// ============================================================================

/// Attempt to drop Linux page caches to ensure cold cache measurements.
///
/// Requires sudo/root privileges. Falls back gracefully if not available.
fn drop_caches() {
    // Try to write to /proc/sys/vm/drop_caches
    // This requires root privileges
    match OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(b"3\n") {
                eprintln!("Warning: Failed to drop caches (write failed): {}", e);
                eprintln!("Benchmark will run with warm cache");
            } else {
                // Sync to ensure caches are dropped
                let _ = file.sync_all();
            }
        }
        Err(e) => {
            eprintln!("Warning: Failed to open /proc/sys/vm/drop_caches: {}", e);
            eprintln!("This is expected without root privileges");
            eprintln!("Benchmark will run with warm cache");
        }
    }

    // Give the system a moment to process
    std::thread::sleep(Duration::from_millis(100));
}

// ============================================================================
// COLD CACHE BFS BENCHMARK
// ============================================================================

fn bench_cold_cache_bfs(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_cache/bfs");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);
    group.sample_size(10);

    let test_sizes = [1000, 10000, 100000];

    for size in test_sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            // Setup: Create a fresh database for each iteration
            b.iter_batched(
                || {
                    let temp_dir = bench_utils::create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("cold_cache_bfs.db");
                    let backend = V3Backend::create(&db_path).unwrap();

                    // Create chain graph: 0 -> 1 -> 2 -> ... -> (size-1)
                    let mut node_ids = Vec::with_capacity(size);
                    for i in 0..size {
                        let node_id = backend
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({ "id": i }),
                            })
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    // Create edges (chain)
                    for i in 0..size.saturating_sub(1) {
                        backend
                            .insert_edge(EdgeSpec {
                                from: node_ids[i],
                                to: node_ids[i + 1],
                                edge_type: "NEXT".to_string(),
                                data: serde_json::json!({}),
                            })
                            .unwrap();
                    }

                    // Flush to disk
                    backend.flush().unwrap();

                    // Drop the backend to close file handles
                    drop(backend);

                    // Drop OS page caches
                    drop_caches();

                    // Reopen backend (this will be cold cache)
                    let backend = V3Backend::open(&db_path).unwrap();

                    (BenchmarkState { backend, temp_dir }, node_ids)
                },
                |(ctx, node_ids)| {
                    // BFS traversal from first node
                    let backend = &ctx.backend;
                    let snapshot = SnapshotId::current();
                    let mut visited = std::collections::HashSet::new();
                    let mut queue = vec![node_ids[0]];

                    while let Some(node_id) = queue.pop() {
                        if visited.contains(&node_id) {
                            continue;
                        }
                        visited.insert(node_id);

                        // Get neighbors using the correct API
                        let neighbors = backend
                            .neighbors(
                                snapshot,
                                node_id,
                                NeighborQuery {
                                    direction: BackendDirection::Outgoing,
                                    edge_type: None,
                                },
                            )
                            .unwrap();

                        for neighbor_id in neighbors {
                            if !visited.contains(&neighbor_id) {
                                queue.push(neighbor_id);
                            }
                        }
                    }

                    black_box(visited.len());
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// COLD CACHE POINT LOOKUP BENCHMARK
// ============================================================================

fn bench_cold_cache_point_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_cache/point_lookup");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);
    group.sample_size(10);

    let test_sizes = [1000, 10000, 100000];

    for size in test_sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            // Setup: Create a fresh database for each iteration
            b.iter_batched(
                || {
                    let temp_dir = bench_utils::create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("cold_cache_lookup.db");
                    let backend = V3Backend::create(&db_path).unwrap();

                    // Create nodes
                    let mut node_ids = Vec::with_capacity(size);
                    for i in 0..size {
                        let node_id = backend
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({ "id": i }),
                            })
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    // Flush to disk
                    backend.flush().unwrap();

                    // Drop the backend to close file handles
                    drop(backend);

                    // Drop OS page caches
                    drop_caches();

                    // Reopen backend (this will be cold cache)
                    let backend = V3Backend::open(&db_path).unwrap();

                    (BenchmarkState { backend, temp_dir }, node_ids)
                },
                |(ctx, node_ids)| {
                    // Point lookups: fetch every 10th node
                    let backend = &ctx.backend;
                    let snapshot = SnapshotId::current();
                    let mut count = 0;

                    for i in (0..size).step_by(10) {
                        let node = backend.get_node(snapshot, node_ids[i]).unwrap();
                        black_box(node);
                        count += 1;
                    }

                    black_box(count);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// COLD CACHE SEQUENTIAL SCAN BENCHMARK
// ============================================================================

fn bench_cold_cache_sequential_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_cache/sequential_scan");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);
    group.sample_size(10);

    let test_sizes = [1000, 10000, 100000];

    for size in test_sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            // Setup: Create a fresh database for each iteration
            b.iter_batched(
                || {
                    let temp_dir = bench_utils::create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("cold_cache_scan.db");
                    let backend = V3Backend::create(&db_path).unwrap();

                    // Create nodes
                    let mut node_ids = Vec::with_capacity(size);
                    for i in 0..size {
                        let node_id = backend
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({ "id": i }),
                            })
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    // Flush to disk
                    backend.flush().unwrap();

                    // Drop the backend to close file handles
                    drop(backend);

                    // Drop OS page caches
                    drop_caches();

                    // Reopen backend (this will be cold cache)
                    let backend = V3Backend::open(&db_path).unwrap();

                    (BenchmarkState { backend, temp_dir }, node_ids)
                },
                |(ctx, node_ids)| {
                    // Sequential scan: fetch all nodes in order
                    let backend = &ctx.backend;
                    let snapshot = SnapshotId::current();
                    let mut count = 0;

                    for node_id in &node_ids {
                        let node = backend.get_node(snapshot, *node_id).unwrap();
                        black_box(node);
                        count += 1;
                    }

                    black_box(count);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_cold_cache_bfs,
    bench_cold_cache_point_lookup,
    bench_cold_cache_sequential_scan,
);
criterion_main!(benches);
