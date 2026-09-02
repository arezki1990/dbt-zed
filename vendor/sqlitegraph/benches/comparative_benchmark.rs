use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::time::Duration;
use tempfile::TempDir;

const DATASET_SIZES: &[usize] = &[100, 1000, 10000];
const EDGE_MULTIPLIERS: &[f64] = &[2.0, 2.0, 2.0]; // edges = nodes * multiplier
const DENSE_EDGE_MULTIPLIER: f64 = 500.0; // For dense graphs

/// Generate a deterministic random graph
fn generate_dataset(seed: u64, num_nodes: usize, edge_multiplier: f64) -> Vec<(u64, u64)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let num_edges = (num_nodes as f64 * edge_multiplier) as usize;
    let mut edges = Vec::with_capacity(num_edges);

    // Create a connected graph first
    for i in 1..num_nodes {
        let j = rng.gen_range(0..i);
        edges.push((j as u64, i as u64));
    }

    // Add random edges
    for _ in edges.len()..num_edges {
        let a = rng.gen_range(0..num_nodes) as u64;
        let b = rng.gen_range(0..num_nodes) as u64;
        if a != b {
            edges.push((a, b));
        }
    }

    edges
}

/// Benchmark SQLiteGraph V2 operations
fn benchmark_sqlitegraph_v2(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlitegraph_v2");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(20);

    for &num_nodes in DATASET_SIZES {
        let edge_multiplier =
            EDGE_MULTIPLIERS[DATASET_SIZES.iter().position(|&n| n == num_nodes).unwrap()];
        let _num_edges = (num_nodes as f64 * edge_multiplier) as usize;

        group.throughput(Throughput::Elements(num_nodes as u64));

        // Benchmark graph creation
        group.bench_with_input(
            BenchmarkId::new("create_graph", num_nodes),
            &num_nodes,
            |b, &num_nodes| {
                b.iter_with_setup(
                    || {
                        let temp_dir = TempDir::new().unwrap();
                        let db_path = temp_dir.path().join("benchmark.db");
                        let config = GraphConfig::native();
                        (db_path.to_str().unwrap().to_string(), config)
                    },
                    |(db_path, config)| {
                        let graph = open_graph(&db_path, &config).unwrap();
                        let edges = generate_dataset(42, num_nodes, edge_multiplier);

                        // Create nodes
                        for i in 0..num_nodes {
                            let node_spec = NodeSpec {
                                kind: "Node".to_string(),
                                name: format!("node_{}", i),
                                file_path: None,
                                data: serde_json::Value::Null,
                            };
                            let node_id = graph.insert_node(node_spec).unwrap();
                            black_box(node_id);
                        }

                        // Create edges
                        for (src, dst) in edges {
                            if src < num_nodes as u64 && dst < num_nodes as u64 {
                                let edge_spec = EdgeSpec {
                                    from: src as i64,
                                    to: dst as i64,
                                    edge_type: "Connects".to_string(),
                                    data: serde_json::json!({"weight": 1.0}),
                                };
                                let _ = graph.insert_edge(edge_spec).unwrap();
                            }
                        }
                    },
                )
            },
        );

        // Benchmark neighbor queries
        group.bench_with_input(
            BenchmarkId::new("neighbor_query", num_nodes),
            &num_nodes,
            |b, &num_nodes| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("benchmark.db");
                let config = GraphConfig::native();
                let graph = open_graph(db_path.to_str().unwrap(), &config).unwrap();

                // Pre-populate graph
                let edges = generate_dataset(42, num_nodes, edge_multiplier);
                for i in 0..num_nodes {
                    let node_spec = NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("node_{}", i),
                        file_path: None,
                        data: serde_json::Value::Null,
                    };
                    let _ = graph.insert_node(node_spec).unwrap();
                }
                for (src, dst) in edges {
                    if src < num_nodes as u64 && dst < num_nodes as u64 {
                        let edge_spec = EdgeSpec {
                            from: src as i64,
                            to: dst as i64,
                            edge_type: "Connects".to_string(),
                            data: serde_json::json!({"weight": 1.0}),
                        };
                        let _ = graph.insert_edge(edge_spec).unwrap();
                    }
                }

                // Query random nodes
                let mut rng = StdRng::seed_from_u64(42);
                b.iter(|| {
                    let node_id = rng.gen_range(1..num_nodes) as u64;
                    let neighbor_query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    let neighbors = graph
                        .neighbors(SnapshotId::current(), node_id as i64, neighbor_query)
                        .unwrap();
                    black_box(neighbors);
                });
            },
        );

        // Benchmark BFS traversal
        group.bench_with_input(
            BenchmarkId::new("bfs_traversal", num_nodes),
            &num_nodes,
            |b, &num_nodes| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("benchmark.db");
                let config = GraphConfig::native();
                let graph = open_graph(db_path.to_str().unwrap(), &config).unwrap();

                // Pre-populate graph
                let edges = generate_dataset(42, num_nodes, edge_multiplier);
                for i in 0..num_nodes {
                    let node_spec = NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("node_{}", i),
                        file_path: None,
                        data: serde_json::Value::Null,
                    };
                    let _ = graph.insert_node(node_spec).unwrap();
                }
                for (src, dst) in edges {
                    if src < num_nodes as u64 && dst < num_nodes as u64 {
                        let edge_spec = EdgeSpec {
                            from: src as i64,
                            to: dst as i64,
                            edge_type: "Connects".to_string(),
                            data: serde_json::json!({"weight": 1.0}),
                        };
                        let _ = graph.insert_edge(edge_spec).unwrap();
                    }
                }

                b.iter(|| {
                    let visited = graph.bfs(SnapshotId::current(), 0, 5).unwrap();
                    black_box(visited);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark dense graph operations
fn benchmark_dense_graphs(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlitegraph_v2_dense");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(10);

    let num_nodes = 1000;
    let edge_multiplier = DENSE_EDGE_MULTIPLIER;
    let _num_edges = (num_nodes as f64 * edge_multiplier) as usize;

    group.throughput(Throughput::Elements(num_nodes as u64));

    group.bench_with_input(
        BenchmarkId::new("create_dense", num_nodes),
        &num_nodes,
        |b, &_| {
            b.iter_with_setup(
                || {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("dense.db");
                    let config = GraphConfig::native();
                    (db_path.to_str().unwrap().to_string(), config)
                },
                |(db_path, config)| {
                    let graph = open_graph(&db_path, &config).unwrap();
                    let edges = generate_dataset(42, num_nodes, edge_multiplier);

                    for i in 0..num_nodes {
                        let node_spec = NodeSpec {
                            kind: "Node".to_string(),
                            name: format!("node_{}", i),
                            file_path: None,
                            data: serde_json::Value::Null,
                        };
                        let _ = graph.insert_node(node_spec).unwrap();
                    }

                    for (src, dst) in edges {
                        if src < num_nodes as u64 && dst < num_nodes as u64 {
                            let edge_spec = EdgeSpec {
                                from: src as i64,
                                to: dst as i64,
                                edge_type: "Connects".to_string(),
                                data: serde_json::json!({"weight": 1.0}),
                            };
                            let _ = graph.insert_edge(edge_spec).unwrap();
                        }
                    }
                },
            )
        },
    );

    group.finish();
}

criterion_group!(benches, benchmark_sqlitegraph_v2, benchmark_dense_graphs);
criterion_main!(benches);
