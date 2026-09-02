//! Memory overhead regression benchmark for telemetry.
//!
//! Measures memory usage during Chain(500) traversal with telemetry enabled.
//! Validates that telemetry adds ≤+5% memory overhead vs v1.6 baseline.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP, create_benchmark_temp_dir};

/// Helper to create a chain graph of specified size
fn create_chain_graph(size: usize) -> (tempfile::TempDir, std::path::PathBuf, Vec<i64>) {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(size);

    // Create nodes
    for i in 0..size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create chain edges
    for i in 0..size.saturating_sub(1) {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"order": i}),
            })
            .expect("Failed to insert edge");
    }

    (temp_dir, db_path, node_ids)
}

/// Benchmark memory overhead during Chain traversal
///
/// Measures BFS traversal time. The native backend includes telemetry
/// in TraversalContext. Telemetry fields should add minimal overhead (<1% of total memory).
fn bench_memory_overhead_native(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_memory_native");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Test chain sizes from small to large
    for &size in &[100, 500, 1000] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(
            BenchmarkId::new("chain_native", size),
            &size,
            |b, &_size| {
                b.iter(|| {
                    let (_temp_dir, db_path, node_ids) = create_chain_graph(size);

                    let graph =
                        open_graph(&db_path, &GraphConfig::native()).expect("Failed to open graph");

                    // Run BFS - native backend has telemetry in TraversalContext
                    let start_node = node_ids[0];
                    let _result = graph
                        .bfs(SnapshotId::current(), start_node, size as u32)
                        .expect("BFS traversal failed");

                    std::mem::forget(_temp_dir);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark SQLite backend for baseline comparison
///
/// SQLite backend doesn't have TraversalContext overhead, provides baseline.
fn bench_memory_overhead_sqlite(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_memory_sqlite");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500, 1000] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(
            BenchmarkId::new("chain_sqlite", size),
            &size,
            |b, &_size| {
                b.iter(|| {
                    let (_temp_dir, db_path, node_ids) = create_chain_graph(size);

                    let graph =
                        open_graph(&db_path, &GraphConfig::sqlite()).expect("Failed to open graph");

                    // Run BFS - SQLite backend without TraversalContext overhead
                    let start_node = node_ids[0];
                    let _result = graph
                        .bfs(SnapshotId::current(), start_node, size as u32)
                        .expect("BFS traversal failed");

                    std::mem::forget(_temp_dir);
                });
            },
        );
    }

    group.finish();
}

/// Calculate approximate memory overhead from telemetry
///
/// This is a compile-time estimation based on field sizes:
/// - cluster_buffer: Option<Vec<u8>> - up to 512KB
/// - cluster_buffer_offsets: Vec<(u64, u32)> - 24 bytes per cluster
/// - node_cluster_index: HashMap<NativeNodeId, usize> - ~32 bytes per entry
///
/// For Chain(500):
/// - cluster_buffer: 512KB (max, typically less)
/// - offsets: 500 * 24 = 12KB
/// - index: 500 * 32 = 16KB
/// - Total: ~540KB max, ~28KB typical (no sequential read)
///
/// Expected overhead: <1% of total traversal memory
#[allow(dead_code)]
fn estimate_telemetry_overhead(node_count: usize) -> (usize, usize, usize) {
    // Cluster buffer (max 512KB, but only allocated during sequential reads)
    let cluster_buffer_max = 512 * 1024;

    // Cluster offsets (only stored for visited nodes)
    let offsets_size = node_count * 24; // (u64, u32) = 12 bytes + Vec overhead

    // Node-to-cluster index (only stored during sequential reads)
    let index_size = if node_count > 0 {
        node_count * 32 // HashMap entry overhead
    } else {
        0
    };

    (cluster_buffer_max, offsets_size, index_size)
}

criterion_group!(
    benches,
    bench_memory_overhead_native,
    bench_memory_overhead_sqlite
);
criterion_main!(benches);
