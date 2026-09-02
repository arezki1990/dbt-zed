//! Native Disk I/O Profiling Benchmarks for Phase 14.
//!
//! These benchmarks specifically target V1 native I/O performance characteristics
//! identified in the analysis, including:
//! - Read amplification from 64KB buffer
//! - Cache thrashing patterns
//! - Corruption boundary conditions
//! - Space inefficiency measurements

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use sqlitegraph::{
    BackendDirection, EdgeSpec, NodeSpec, SnapshotId, config::GraphConfig, open_graph,
};
use std::time::Duration;
use tempfile::TempDir;

/// Benchmark sequential node access in V1 native backend
/// Measures read amplification effect with sequential access patterns
fn bench_sequential_node_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential_node_access");

    // Test different graph sizes to show I/O amplification scaling
    for &size in &[10, 50, 100, 200] {
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("seq_access.db");

            let config = GraphConfig::native();
            let graph = open_graph(&db_path, &config).unwrap();

            // Create sequential graph
            for i in 1..=size {
                let node_spec = NodeSpec {
                    kind: "seq_test".to_string(),
                    name: format!("seq_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"seq_id": i}),
                };
                graph.insert_node(node_spec).unwrap();
            }

            // Benchmark sequential access
            b.iter(|| {
                for i in 1..=size {
                    black_box(graph.get_node(SnapshotId::current(), i).unwrap());
                }
            });
        });
    }

    group.finish();
}

/// Benchmark cache thrashing in V1 native backend
/// Tests performance degradation when accessing >100 unique nodes
fn bench_cache_thrashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_thrashing");

    // Test different access patterns around the 100-entry cache boundary
    for &cache_test_size in &[50, 100, 150, 200] {
        group.bench_with_input(
            BenchmarkId::new("native", cache_test_size),
            &cache_test_size,
            |b, &cache_test_size| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("cache_thrash.db");

                let config = GraphConfig::native();
                let graph = open_graph(&db_path, &config).unwrap();

                // Create graph with more nodes than cache size
                for i in 1..=cache_test_size {
                    let node_spec = NodeSpec {
                        kind: "cache_test".to_string(),
                        name: format!("cache_node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"cache_id": i}),
                    };
                    graph.insert_node(node_spec).unwrap();
                }

                // Benchmark random access that forces cache thrashing
                b.iter(|| {
                    for i in (1..=cache_test_size).rev() {
                        black_box(graph.get_node(SnapshotId::current(), i).unwrap());
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark corruption boundary conditions
/// Tests I/O behavior around node 257 boundary where corruption occurs
fn bench_corruption_boundary(c: &mut Criterion) {
    let mut group = c.benchmark_group("corruption_boundary");

    // Test access patterns around the known corruption boundary
    for &boundary_node in &[250, 255, 256, 257, 258, 260, 300] {
        group.bench_with_input(
            BenchmarkId::new("native", boundary_node),
            &boundary_node,
            |b, &boundary_node| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("boundary_test.db");

                let config = GraphConfig::native();
                let graph = open_graph(&db_path, &config).unwrap();

                // Create enough nodes to reach boundary
                for i in 1..=boundary_node.max(300) {
                    let node_spec = NodeSpec {
                        kind: "boundary_test".to_string(),
                        name: format!("boundary_node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"boundary_id": i}),
                    };
                    graph.insert_node(node_spec).unwrap();
                }

                // Benchmark accessing boundary node
                b.iter(|| {
                    // Note: This should fail with corruption before the fix
                    black_box(
                        graph
                            .get_node(SnapshotId::current(), boundary_node)
                            .unwrap(),
                    );
                });
            },
        );
    }

    group.finish();
}

/// Benchmark edge insertion corruption
/// Tests edge insertion behavior around node 257 boundary
fn bench_edge_insertion_boundary(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_insertion_boundary");

    // Test edge insertion around the corruption boundary
    for &edge_count in &[100, 200, 250, 256, 257, 300] {
        group.bench_with_input(
            BenchmarkId::new("native", edge_count),
            &edge_count,
            |b, &edge_count| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("edge_boundary.db");

                let config = GraphConfig::native();
                let graph = open_graph(&db_path, &config).unwrap();

                // Create nodes
                for i in 1..=edge_count {
                    let node_spec = NodeSpec {
                        kind: "edge_boundary_test".to_string(),
                        name: format!("edge_node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"edge_id": i}),
                    };
                    graph.insert_node(node_spec).unwrap();
                }

                // Benchmark edge insertion that may hit corruption boundary
                b.iter(|| {
                    // Insert edge that will trigger boundary condition
                    let edge_spec = EdgeSpec {
                        from: (edge_count / 2) as i64, // Use middle node
                        to: edge_count as i64,         // Use boundary node
                        edge_type: "boundary_test".to_string(),
                        data: serde_json::json!({"edge_boundary": true}),
                    };

                    // Note: This should fail with corruption before the fix
                    black_box(graph.insert_edge(edge_spec).unwrap());
                });
            },
        );
    }

    group.finish();
}

/// Benchmark space efficiency comparison
/// Measures actual file size vs theoretical minimum
fn bench_space_efficiency(c: &mut Criterion) {
    let mut group = c.benchmark_group("space_efficiency");
    group.measurement_time(Duration::from_secs(10));

    // Test different node counts to measure space overhead scaling
    for &node_count in &[10, 50, 100, 200, 500] {
        group.bench_with_input(
            BenchmarkId::new("native_file_size", node_count),
            &node_count,
            |b, &node_count| {
                b.iter(|| {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("space_test.db");

                    let config = GraphConfig::native();
                    let graph = open_graph(&db_path, &config).unwrap();

                    // Create nodes
                    for i in 1..=node_count {
                        let node_spec = NodeSpec {
                            kind: "space_test".to_string(),
                            name: format!("space_node_{}", i),
                            file_path: None,
                            data: serde_json::json!({"space_id": i}),
                        };
                        graph.insert_node(node_spec).unwrap();
                    }

                    // Force write to disk and measure file size
                    drop(graph);
                    let file_size = std::fs::metadata(&db_path).unwrap().len();

                    black_box(file_size);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark k-hop performance with different topologies
/// Tests the exponential performance degradation observed in analysis
fn bench_k_hop_topology_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("k_hop_topology_performance");

    // Test different topologies and sizes
    let topologies = ["chain", "star", "random"];
    let sizes = [10, 50, 100];

    for &topology in &topologies {
        for &size in &sizes {
            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", topology, size), size),
                &(topology, size),
                |b, &(topology, size)| {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("topology_test.db");

                    let config = GraphConfig::native();
                    let mut graph = open_graph(&db_path, &config).unwrap();

                    // Create topology-specific graph
                    create_topology(&mut graph, topology, size);

                    // Benchmark k-hop from appropriate start node
                    let start_node = match topology {
                        "chain" => 1,
                        "star" => 1,
                        "random" => 1,
                        _ => 1,
                    };

                    b.iter(|| {
                        black_box(
                            graph
                                .k_hop(
                                    SnapshotId::current(),
                                    start_node,
                                    1,
                                    BackendDirection::Outgoing,
                                )
                                .unwrap(),
                        );
                    });
                },
            );
        }
    }

    group.finish();
}

/// Helper function to create different graph topologies
fn create_topology(graph: &mut Box<dyn sqlitegraph::GraphBackend>, topology: &str, size: i64) {
    match topology {
        "chain" => {
            // Create chain: 1 -> 2 -> 3 -> ... -> size
            for i in 1..=size {
                let node_spec = NodeSpec {
                    kind: "chain".to_string(),
                    name: format!("chain_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"chain_id": i}),
                };
                graph.insert_node(node_spec).unwrap();
            }

            for i in 1..size {
                let edge_spec = EdgeSpec {
                    from: i,
                    to: i + 1,
                    edge_type: "chain".to_string(),
                    data: serde_json::json!({"chain_edge": i}),
                };
                graph.insert_edge(edge_spec).unwrap();
            }
        }
        "star" => {
            // Create star: center (1) connected to all others (2..size)
            for i in 1..=size {
                let node_spec = NodeSpec {
                    kind: if i == 1 { "center" } else { "leaf" }.to_string(),
                    name: format!("star_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"star_id": i}),
                };
                graph.insert_node(node_spec).unwrap();
            }

            for i in 2..=size {
                let edge_spec = EdgeSpec {
                    from: 1,
                    to: i,
                    edge_type: "star".to_string(),
                    data: serde_json::json!({"star_edge": i}),
                };
                graph.insert_edge(edge_spec).unwrap();
            }
        }
        "random" => {
            // Create random topology
            for i in 1..=size {
                let node_spec = NodeSpec {
                    kind: "random".to_string(),
                    name: format!("random_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"random_id": i}),
                };
                graph.insert_node(node_spec).unwrap();
            }

            // Create random edges (approximately size edges)
            use std::collections::HashSet;
            let mut edges = HashSet::new();

            for i in 1..=size {
                let from = ((i * 7) % size) + 1;
                let to = ((i * 13) % size) + 1;

                if from != to {
                    edges.insert((from, to));
                }
            }

            for (from, to) in edges {
                let edge_spec = EdgeSpec {
                    from,
                    to,
                    edge_type: "random".to_string(),
                    data: serde_json::json!({"random_edge": true}),
                };
                graph.insert_edge(edge_spec).unwrap();
            }
        }
        _ => panic!("Unknown topology: {}", topology),
    }
}

criterion_group!(
    benches,
    bench_sequential_node_access,
    bench_cache_thrashing,
    bench_corruption_boundary,
    bench_edge_insertion_boundary,
    bench_space_efficiency,
    bench_k_hop_topology_performance
);
criterion_main!(benches);
