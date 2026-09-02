//! Insert performance benchmarks for SQLite vs Native backends.
//!
//! Compares node and edge insertion throughput across different batch sizes
//! and graph topologies using the criterion benchmarking framework.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rand::{RngCore, SeedableRng};
use sqlitegraph::{EdgeSpec, NodeSpec, open_graph};

mod bench_utils;
use bench_utils::{BENCHMARK_SIZES, MEASURE, WARM_UP, create_benchmark_temp_dir};

/// Benchmark node insertions
fn insert_nodes(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("insert_nodes");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in BENCHMARK_SIZES {
        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Insert nodes individually to benchmark node insertion throughput
                let mut node_ids = Vec::new();
                for i in 0..size {
                    let node_id = graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({
                                "id": i,
                                "created_at": "benchmark",
                                "batch_size": size,
                            }),
                        })
                        .expect("Failed to insert node");
                    node_ids.push(node_id);
                }
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Insert nodes individually to benchmark node insertion throughput
                let mut node_ids = Vec::new();
                for i in 0..size {
                    let node_id = graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({
                                "id": i,
                                "created_at": "benchmark",
                                "batch_size": size,
                            }),
                        })
                        .expect("Failed to insert node");
                    node_ids.push(node_id);
                }
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });
    }

    group.finish();
}

/// Benchmark edge insertions
fn insert_edges(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("insert_edges");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in BENCHMARK_SIZES {
        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Create nodes first using individual insertions
                let mut node_ids = Vec::new();
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

                // Create edges in a star pattern using individual insertions
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "star".to_string(),
                            data: serde_json::json!({
                                "from": 0,
                                "to": i,
                                "edge_id": i,
                            }),
                        })
                        .expect("Failed to insert edge");
                }
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create nodes first using individual insertions
                let mut node_ids = Vec::new();
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

                // Create edges in a star pattern using individual insertions
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "star".to_string(),
                            data: serde_json::json!({
                                "from": 0,
                                "to": i,
                                "edge_id": i,
                            }),
                        })
                        .expect("Failed to insert edge");
                }
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });
    }

    group.finish();
}

/// Benchmark mixed node and edge insertions
fn insert_mixed(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("insert_mixed");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500] {
        // Smaller sizes for mixed inserts
        // SQLite backend
        group.bench_with_input(
            BenchmarkId::new("sqlite", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("benchmark.db");

                    let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                        .expect("Failed to create graph");

                    let node_count = size;
                    let edge_count = size; // 1:1 node:edge ratio

                    // Create nodes using individual insertions
                    let mut node_ids = Vec::new();
                    for i in 0..node_count {
                        let node_id = graph.insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({
                                "id": i,
                                "type": "mixed_insert",
                            }),
                        }).expect("Failed to insert node");
                        node_ids.push(node_id);
                    }

                    // Create mixed topology edges using individual insertions
                    let mut rng = rand::rngs::StdRng::seed_from_u64(0xA17C);

                    for i in 0..edge_count {
                        if i % 3 == 0 && node_count > 1 {
                            // Chain edges
                            if i + 1 < node_count {
                                graph.insert_edge(EdgeSpec {
                                    from: node_ids[i],
                                    to: node_ids[i + 1],
                                    edge_type: "chain".to_string(),
                                    data: serde_json::json!({"pattern": "chain", "index": i}),
                                }).expect("Failed to insert edge");
                            }
                        } else if i % 3 == 1 && node_count > 2 {
                            // Star edges from center
                            let center = 0;
                            let leaf = (i % (node_count - 1)) + 1;
                            graph.insert_edge(EdgeSpec {
                                from: node_ids[center],
                                to: node_ids[leaf],
                                edge_type: "star".to_string(),
                                data: serde_json::json!({"pattern": "star", "leaf": leaf}),
                            }).expect("Failed to insert edge");
                        } else {
                            // Random edges
                            let from_idx = (rng.next_u32() as usize) % node_count;
                            let mut to_idx = (rng.next_u32() as usize) % node_count;
                            while to_idx == from_idx {
                                to_idx = (rng.next_u32() as usize) % node_count;
                            }
                            graph.insert_edge(EdgeSpec {
                                from: node_ids[from_idx],
                                to: node_ids[to_idx],
                                edge_type: "random".to_string(),
                                data: serde_json::json!({"pattern": "random", "seed": rng.next_u64()}),
                            }).expect("Failed to insert edge");
                        }
                    }
                });
            },
        );

        // Native backend
        group.bench_with_input(
            BenchmarkId::new("native", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = create_benchmark_temp_dir();
                    let db_path = temp_dir.path().join("benchmark.db");

                    let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                        .expect("Failed to create graph");

                    let node_count = size;
                    let edge_count = size; // 1:1 node:edge ratio

                    // Create nodes using individual insertions
                    let mut node_ids = Vec::new();
                    for i in 0..node_count {
                        let node_id = graph.insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::json!({
                                "id": i,
                                "type": "mixed_insert",
                            }),
                        }).expect("Failed to insert node");
                        node_ids.push(node_id);
                    }

                    // Create mixed topology edges using individual insertions
                    let mut rng = rand::rngs::StdRng::seed_from_u64(0xA17C);

                    for i in 0..edge_count {
                        if i % 3 == 0 && node_count > 1 {
                            // Chain edges
                            if i + 1 < node_count {
                                graph.insert_edge(EdgeSpec {
                                    from: node_ids[i],
                                    to: node_ids[i + 1],
                                    edge_type: "chain".to_string(),
                                    data: serde_json::json!({"pattern": "chain", "index": i}),
                                }).expect("Failed to insert edge");
                            }
                        } else if i % 3 == 1 && node_count > 2 {
                            // Star edges from center
                            let center = 0;
                            let leaf = (i % (node_count - 1)) + 1;
                            graph.insert_edge(EdgeSpec {
                                from: node_ids[center],
                                to: node_ids[leaf],
                                edge_type: "star".to_string(),
                                data: serde_json::json!({"pattern": "star", "leaf": leaf}),
                            }).expect("Failed to insert edge");
                        } else {
                            // Random edges
                            let from_idx = (rng.next_u32() as usize) % node_count;
                            let mut to_idx = (rng.next_u32() as usize) % node_count;
                            while to_idx == from_idx {
                                to_idx = (rng.next_u32() as usize) % node_count;
                            }
                            graph.insert_edge(EdgeSpec {
                                from: node_ids[from_idx],
                                to: node_ids[to_idx],
                                edge_type: "random".to_string(),
                                data: serde_json::json!({"pattern": "random", "seed": rng.next_u64()}),
                            }).expect("Failed to insert edge");
                        }
                    }
                    std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark incremental insertions (small batches)
fn insert_incremental(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("insert_incremental");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    const BATCH_SIZE: usize = 100;
    const TOTAL_OPERATIONS: usize = 1_000;

    // SQLite backend
    group.bench_function("sqlite", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                .expect("Failed to create graph");

            let mut entity_counter = 0;
            let mut edge_counter = 0;

            for batch in 0..(TOTAL_OPERATIONS / BATCH_SIZE) {
                // Insert batch of nodes using individual insertions
                let mut node_ids = Vec::new();
                for i in 0..BATCH_SIZE {
                    let node_id = graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", entity_counter + i),
                            file_path: None,
                            data: serde_json::json!({
                                "batch": batch,
                                "local_id": i,
                                "global_id": entity_counter + i,
                            }),
                        })
                        .expect("Failed to insert node");
                    node_ids.push(node_id);
                }

                entity_counter += BATCH_SIZE;

                // Insert batch of edges using individual insertions
                for i in 0..BATCH_SIZE {
                    let from_idx = i;
                    let to_idx = (i + 1) % node_ids.len();
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[from_idx],
                            to: node_ids[to_idx],
                            edge_type: "batch".to_string(),
                            data: serde_json::json!({
                                "batch": batch,
                                "edge_in_batch": i,
                                "global_edge_id": edge_counter + i,
                            }),
                        })
                        .expect("Failed to insert edge");
                }

                edge_counter += BATCH_SIZE;
            }
        });
    });

    // Native backend
    group.bench_function("native", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph = open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                .expect("Failed to create graph");

            let mut entity_counter = 0;
            let mut edge_counter = 0;

            for batch in 0..(TOTAL_OPERATIONS / BATCH_SIZE) {
                // Insert batch of nodes using individual insertions
                let mut node_ids = Vec::new();
                for i in 0..BATCH_SIZE {
                    let node_id = graph
                        .insert_node(NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", entity_counter + i),
                            file_path: None,
                            data: serde_json::json!({
                                "batch": batch,
                                "local_id": i,
                                "global_id": entity_counter + i,
                            }),
                        })
                        .expect("Failed to insert node");
                    node_ids.push(node_id);
                }

                entity_counter += BATCH_SIZE;

                // Insert batch of edges using individual insertions
                for i in 0..BATCH_SIZE {
                    let from_idx = i;
                    let to_idx = (i + 1) % node_ids.len();
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[from_idx],
                            to: node_ids[to_idx],
                            edge_type: "batch".to_string(),
                            data: serde_json::json!({
                                "batch": batch,
                                "edge_in_batch": i,
                                "global_edge_id": edge_counter + i,
                            }),
                        })
                        .expect("Failed to insert edge");
                }

                edge_counter += BATCH_SIZE;
            }
            std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    insert_nodes,
    insert_edges,
    insert_mixed,
    insert_incremental
);
criterion_main!(benches);
