//! Write cost regression benchmark for pub/sub emission.
//!
//! Measures write-path cost with pub/sub event emission to ensure ≤+10% increase vs baseline.
//! This validates that publisher.emit() on the commit path doesn't degrade write performance.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sqlitegraph::backend::SubscriptionFilter;
use sqlitegraph::{EdgeSpec, NodeSpec, open_graph};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP, create_benchmark_temp_dir};

/// Benchmark write cost with pub/sub (0 subscribers - baseline)
///
/// Creates a Publisher but has no subscribers. This measures the overhead
/// of emit() calls when there are no receivers to deliver to.
fn bench_write_cost_no_subscribers(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_pubsub_write_baseline");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Test various graph sizes to detect scaling issues
    for &size in &[100, 500, 1000, 5000] {
        group.bench_with_input(
            BenchmarkId::new("no_subscribers", size),
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

/// Benchmark write cost with N subscribers (receivers dropped immediately)
///
/// Subscribes N receivers but drops them immediately. This isolates the emit()
/// cost from receiver processing cost. We measure channel send overhead without
/// waiting for receivers to consume events.
fn bench_write_cost_with_subscribers(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_pubsub_write_with_subs");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Test with different subscriber counts
    for &subscriber_count in &[1, 5, 10] {
        let size = 1000; // Fixed size for subscriber comparison

        group.bench_with_input(
            BenchmarkId::new("with_subscribers", subscriber_count),
            &subscriber_count,
            |b, &subscriber_count| {
                b.iter(|| {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("benchmark.db");

                    let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                        .expect("Failed to create graph");

                    // Subscribe N receivers and drop them immediately
                    // This measures emit() overhead without receiver processing
                    for _ in 0..subscriber_count {
                        let (_id, _rx) = graph
                            .subscribe(SubscriptionFilter::all())
                            .expect("Failed to subscribe");
                        // Drop rx immediately - we only measure emit() cost
                    }

                    // Create nodes
                    let mut node_ids = Vec::with_capacity(size);
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

                    std::mem::forget(temp_dir);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark write operations per 1000 for normalization
///
/// Provides normalized metrics for comparison across different subscriber counts.
/// All benchmarks use 1000 operations.
fn bench_write_cost_per_operation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("regression_pubsub_write_per_1k");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    const SIZE: usize = 1000;

    // Benchmark with 0 subscribers (baseline)
    group.bench_function("0_subscribers", |b| {
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

    // Benchmark with 1 subscriber
    group.bench_function("1_subscriber", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                .expect("Failed to create graph");

            // Subscribe and drop receiver immediately
            let (_id, _rx) = graph
                .subscribe(SubscriptionFilter::all())
                .expect("Failed to subscribe");

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

    // Benchmark with 5 subscribers
    group.bench_function("5_subscribers", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                .expect("Failed to create graph");

            // Subscribe 5 receivers and drop them
            for _ in 0..5 {
                let (_id, _rx) = graph
                    .subscribe(SubscriptionFilter::all())
                    .expect("Failed to subscribe");
            }

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

    // Benchmark with 10 subscribers
    group.bench_function("10_subscribers", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                .expect("Failed to create graph");

            // Subscribe 10 receivers and drop them
            for _ in 0..10 {
                let (_id, _rx) = graph
                    .subscribe(SubscriptionFilter::all())
                    .expect("Failed to subscribe");
            }

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
    bench_write_cost_no_subscribers,
    bench_write_cost_with_subscribers,
    bench_write_cost_per_operation
);
criterion_main!(benches);
