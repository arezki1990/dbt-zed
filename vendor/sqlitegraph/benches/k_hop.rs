//! K-Hop traversal performance benchmarks for SQLite vs Native backends.
//!
//! Compares multi-hop traversal performance (depth 1, 2, 3) across different
//! graph topologies using the criterion benchmarking framework.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sqlitegraph::{BackendDirection, EdgeSpec, NodeSpec, SnapshotId};

mod bench_utils;
use bench_utils::{
    BENCHMARK_SIZES, BenchInMemoryGraph, BenchmarkGraph, GraphTopology, MEASURE, WARM_UP,
    create_benchmark_temp_dir,
};

/// Benchmark 1-hop traversals (direct neighbors)
fn k_hop_1(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("k_hop_1");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in BENCHMARK_SIZES {
        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Create star graph for clear 1-hop patterns using individual insertions
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

                // Create star edges from center node (node 0) to all others
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "neighbor".to_string(),
                            data: serde_json::json!({"hop": 1}),
                        })
                        .expect("Failed to insert edge");
                }

                // 1-hop traversal from center node using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        1,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 1-hop traversal");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create star graph for clear 1-hop patterns using individual insertions
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

                // Create star edges from center node (node 0) to all others
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "neighbor".to_string(),
                            data: serde_json::json!({"hop": 1}),
                        })
                        .expect("Failed to insert edge");
                }

                // 1-hop traversal from center node using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        1,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 1-hop traversal");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, size - 1, GraphTopology::Star);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple 1-hop traversal
                let start_node = 0u32;
                let mut visited = std::collections::HashSet::new();
                let mut current_hop = vec![start_node];
                visited.insert(start_node);

                for _ in 0..1 {
                    let mut next_hop = Vec::new();
                    for &node in &current_hop {
                        for &neighbor in graph.neighbors(node) {
                            if !visited.contains(&neighbor) {
                                visited.insert(neighbor);
                                next_hop.push(neighbor);
                            }
                        }
                    }
                    current_hop = next_hop;
                }
            });
        });
    }

    group.finish();
}

/// Benchmark 2-hop traversals (neighbors of neighbors)
fn k_hop_2(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("k_hop_2");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 1_000] {
        // Smaller sizes for 2-hop
        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Create chain graph for 2-hop patterns using individual insertions
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

                // Create chain edges
                for i in 0..size - 1 {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"hop": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // 2-hop traversal using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        2,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 2-hop traversal");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create chain graph for 2-hop patterns using individual insertions
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

                // Create chain edges
                for i in 0..size - 1 {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"hop": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // 2-hop traversal using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        2,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 2-hop traversal");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, size - 1, GraphTopology::Chain);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple 2-hop traversal
                let start_node = 0u32;
                let mut visited = std::collections::HashSet::new();
                let mut current_hop = vec![start_node];
                visited.insert(start_node);

                for _ in 0..2 {
                    let mut next_hop = Vec::new();
                    for &node in &current_hop {
                        for &neighbor in graph.neighbors(node) {
                            if !visited.contains(&neighbor) {
                                visited.insert(neighbor);
                                next_hop.push(neighbor);
                            }
                        }
                    }
                    current_hop = next_hop;
                }
            });
        });
    }

    group.finish();
}

/// Benchmark 3-hop traversals (deep neighbor exploration)
fn k_hop_3(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("k_hop_3");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500] {
        // Even smaller sizes for 3-hop
        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Create chain graph for 3-hop patterns using individual insertions
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

                // Create chain edges
                for i in 0..size - 1 {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"hop": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // 3-hop traversal using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        3,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 3-hop traversal");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create chain graph for 3-hop patterns using individual insertions
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

                // Create chain edges
                for i in 0..size - 1 {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"hop": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // 3-hop traversal using k_hop API
                let _k_hop_result = graph
                    .k_hop(
                        SnapshotId::current(),
                        node_ids[0],
                        3,
                        BackendDirection::Outgoing,
                    )
                    .expect("Failed to perform 3-hop traversal");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, size - 1, GraphTopology::Chain);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple 3-hop traversal
                let start_node = 0u32;
                let mut visited = std::collections::HashSet::new();
                let mut current_hop = vec![start_node];
                visited.insert(start_node);

                for _ in 0..3 {
                    let mut next_hop = Vec::new();
                    for &node in &current_hop {
                        for &neighbor in graph.neighbors(node) {
                            if !visited.contains(&neighbor) {
                                visited.insert(neighbor);
                                next_hop.push(neighbor);
                            }
                        }
                    }
                    current_hop = next_hop;
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, k_hop_1, k_hop_2, k_hop_3);
criterion_main!(benches);
