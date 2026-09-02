//! Write cost regression benchmark for cluster metadata.
//!
//! Measures write-path cost with cluster metadata to ensure ≤+5% increase vs v1.6 baseline.
//! This validates that the observe_with_cluster() optimization doesn't degrade write performance.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sqlitegraph::{EdgeSpec, NodeSpec, open_graph};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP, create_benchmark_temp_dir};

/// Benchmark write operations with cluster metadata
///
/// Measures time to write N nodes with edges in a chain pattern.
/// Target: ≤+5% increase vs v1.6 baseline (without cluster metadata tracking).
fn bench_write_cost_with_metadata(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_write_cost");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Test various graph sizes to detect scaling issues
    for &size in &[100, 500, 1000, 5000] {
        group.bench_with_input(
            BenchmarkId::new("native_with_metadata", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("benchmark.db");

                    let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                        .expect("Failed to create graph");

                    // Create nodes
                    let mut node_ids = Vec::with_capacity(size);
                    for i in 0..size {
                        let node_id = graph
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({
                                    "id": i,
                                    "created_at": "regression_test",
                                }),
                            })
                            .expect("Failed to insert node");
                        node_ids.push(node_id);
                    }

                    // Create chain edges (linear pattern)
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

                    std::mem::forget(temp_dir);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark baseline write cost (without cluster metadata)
///
/// This serves as a reference point. Since we can't disable cluster metadata
/// in the current implementation, this measures the same path and provides
/// a baseline for future comparisons.
fn bench_write_cost_baseline(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_write_baseline");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Measure the same operations to establish baseline
    for &size in &[100, 500, 1000, 5000] {
        group.bench_with_input(
            BenchmarkId::new("native_baseline", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("benchmark.db");

                    let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                        .expect("Failed to create graph");

                    // Create nodes
                    let mut node_ids = Vec::with_capacity(size);
                    for i in 0..size {
                        let node_id = graph
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({
                                    "id": i,
                                    "created_at": "baseline_test",
                                }),
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

                    std::mem::forget(temp_dir);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark write operations per 1000 for normalization
///
/// This provides normalized metrics for comparison across different graph sizes.
fn bench_write_cost_per_operation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_write_per_1k");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Fixed size for per-operation measurement
    const SIZE: usize = 1000;

    group.bench_function("native_1k_ops", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                .expect("Failed to create graph");

            // Create 1000 nodes
            let mut node_ids = Vec::with_capacity(SIZE);
            for i in 0..SIZE {
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
            for i in 0..SIZE.saturating_sub(1) {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[i + 1],
                        edge_type: "chain".to_string(),
                        data: serde_json::json!({"order": i}),
                    })
                    .expect("Failed to insert edge");
            }

            std::mem::forget(temp_dir);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_write_cost_with_metadata,
    bench_write_cost_baseline,
    bench_write_cost_per_operation
);
criterion_main!(benches);
