//! Comprehensive performance benchmarks for SQLiteGraph.
//!
//! Covers all performance-critical operations with regression detection.
//! Run with: cargo bench --bench comprehensive_performance
//!
//! Baseline comparison:
//!   cargo bench --bench comprehensive_performance -- --save-baseline main
//!   cargo bench --bench comprehensive_performance -- --baseline main

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};
use std::time::Duration;

mod bench_utils;
use bench_utils::create_benchmark_temp_dir;

// ============================================================================
// Configuration
// ============================================================================

const SAMPLE_SIZE: usize = 100;
const WARM_UP_TIME: Duration = Duration::from_secs(5);
const MEASURE_TIME: Duration = Duration::from_secs(15);

// ============================================================================
// WAL Recovery Benchmarks
// ============================================================================

fn bench_wal_recovery_throughput(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wal_recovery");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &tx_count in &[10, 50, 100, 500] {
        group.bench_with_input(
            BenchmarkId::from_parameter(tx_count),
            &tx_count,
            |b, &count| {
                b.iter_batched(
                    || {
                        let temp_dir = create_benchmark_temp_dir();
                        let db_path = temp_dir.path().join("wal.db");

                        // Setup: Generate WAL by inserting nodes in transactions
                        {
                            let graph = open_graph(&db_path, &GraphConfig::native()).unwrap();
                            for i in 0..count {
                                let _ = graph.insert_node(NodeSpec {
                                    kind: "Node".to_string(),
                                    name: format!("node_{}", i),
                                    file_path: None,
                                    data: serde_json::json!({"id": i}),
                                });
                            }
                            // Explicitly drop to close WAL
                            drop(graph);
                        }

                        (temp_dir, db_path)
                    },
                    |(temp_dir, db_path)| {
                        // Measure: Recovery time (reopen graph)
                        let start = std::time::Instant::now();
                        let graph = open_graph(&db_path, &GraphConfig::native()).unwrap();
                        let duration = start.elapsed();

                        // Cleanup
                        drop(graph);
                        drop(temp_dir);

                        duration
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ============================================================================
// Insert Throughput Benchmarks
// ============================================================================

fn bench_insert_throughput(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("insert_throughput");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &batch_size in &[1, 10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter_batched(
                    create_benchmark_temp_dir,
                    |temp_dir| {
                        let db_path = temp_dir.path().join("insert.db");
                        let graph = open_graph(&db_path, &GraphConfig::native()).unwrap();

                        let start = std::time::Instant::now();
                        for i in 0..size {
                            let _ = graph.insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({"id": i}),
                            });
                        }
                        let duration = start.elapsed();

                        drop(graph);
                        drop(temp_dir);

                        duration
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ============================================================================
// Traversal Performance Benchmarks
// ============================================================================

fn bench_traversal_performance(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("traversal");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Setup: Create graph with edges for BFS benchmark
    for &depth in &[10, 50, 100, 500] {
        group.bench_with_input(BenchmarkId::new("bfs_depth", depth), &depth, |b, &d| {
            b.iter_batched(
                || {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("traverse.db");
                    let graph = open_graph(&db_path, &GraphConfig::native()).unwrap();

                    // Create chain graph: 0 -> 1 -> 2 -> ... -> depth
                    let mut node_ids = Vec::new();
                    for i in 0..d {
                        let node_id = graph
                            .insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({"id": i}),
                            })
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    // Create chain edges
                    for i in 0..(d as usize).saturating_sub(1) {
                        let _ = graph.insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "next".to_string(),
                            data: serde_json::json!({}),
                        });
                    }

                    (temp_dir, graph, node_ids)
                },
                |(temp_dir, graph, node_ids)| {
                    let start = std::time::Instant::now();
                    // Perform BFS to specified depth
                    let _results = graph
                        .bfs(SnapshotId::current(), node_ids[0], d as u32)
                        .unwrap();
                    let duration = start.elapsed();

                    drop(graph);
                    drop(temp_dir);

                    duration
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================================
// Memory Performance
// ============================================================================

fn bench_memory_efficiency(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("memory");
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(10));

    for &node_count in &[100, 1000, 10000] {
        group.bench_with_input(
            BenchmarkId::new("nodes_memory", node_count),
            &node_count,
            |b, &count| {
                b.iter_batched(
                    create_benchmark_temp_dir,
                    |temp_dir| {
                        let db_path = temp_dir.path().join("memory.db");
                        let graph = open_graph(&db_path, &GraphConfig::native()).unwrap();

                        for i in 0..count {
                            let _ = graph.insert_node(NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::json!({"id": i, "data": "x".repeat(100)}),
                            });
                        }

                        // Force measurement
                        std::hint::black_box(&graph);
                        let size = count;

                        drop(graph);
                        drop(temp_dir);

                        size
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark Group Registration
// ============================================================================

criterion_group!(
    benches,
    bench_wal_recovery_throughput,
    bench_insert_throughput,
    bench_traversal_performance,
    bench_memory_efficiency
);
criterion_main!(benches);
