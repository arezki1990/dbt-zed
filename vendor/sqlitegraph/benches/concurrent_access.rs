//! Concurrent read/write access benchmarks for V3 backend
//!
//! Measures performance degradation under concurrent workloads
//! to validate thread-safety and identify bottlenecks.

use std::sync::Arc;
use std::thread;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use sqlitegraph::snapshot::SnapshotId;
use sqlitegraph::{GraphBackend, NativeGraphBackend, NodeSpec};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP};

/// Benchmark concurrent reads (4 threads reading simultaneously)
fn bench_concurrent_reads(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("concurrent_reads");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = tempfile::tempdir().unwrap();
                let db_path = temp_dir.path().join("concurrent.db");

                // Setup: Create graph with data
                let graph = NativeGraphBackend::create(&db_path).unwrap();

                // Insert nodes
                for i in 0..size {
                    graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({}),
                        })
                        .unwrap();
                }

                // Spawn 4 reader threads
                let graph = Arc::new(graph);
                let handles: Vec<_> = (0..4)
                    .map(|_| {
                        let graph = Arc::clone(&graph);
                        thread::spawn(move || {
                            let snapshot = SnapshotId::current();
                            // Read random nodes
                            for i in 0..size / 10 {
                                let node_id = (i * 7) % size;
                                let _ = black_box(graph.get_node(snapshot, node_id as i64));
                            }
                        })
                    })
                    .collect();

                // Wait for all threads
                for handle in handles {
                    handle.join().unwrap();
                }
            });
        });
    }

    group.finish();
}

/// Benchmark 80% reads / 20% writes mixed workload
fn bench_mixed_workload(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("mixed_workload_80_20");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = tempfile::tempdir().unwrap();
                let db_path = temp_dir.path().join("mixed.db");

                let graph = Arc::new(NativeGraphBackend::create(&db_path).unwrap());

                // Initial population
                for i in 0..size {
                    graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({}),
                        })
                        .unwrap();
                }

                let handles: Vec<_> = (0..4)
                    .map(|i| {
                        let graph = Arc::clone(&graph);
                        thread::spawn(move || {
                            for j in 0..100 {
                                if i < 3 {
                                    // 75% reads (3 of 4 threads)
                                    let node_id = (j * 7) % size;
                                    let snapshot = SnapshotId::current();
                                    let _ = black_box(graph.get_node(snapshot, node_id as i64));
                                } else {
                                    // 25% writes (1 of 4 threads)
                                    if j < 20 {
                                        let _ = graph.insert_node(NodeSpec {
                                            kind: "Node".to_string(),
                                            name: format!("new_{}", j),
                                            file_path: None,
                                            data: serde_json::json!({}),
                                        });
                                    }
                                }
                            }
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.join().unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    name = concurrent_benches;
    config = Criterion::default().sample_size(10);
    targets = bench_concurrent_reads, bench_mixed_workload
);

criterion_main!(concurrent_benches);
