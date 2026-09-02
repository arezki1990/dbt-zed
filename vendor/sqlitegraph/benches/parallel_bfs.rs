//! Parallel BFS Performance Benchmark
//!
//! Compares sequential vs parallel BFS performance on the V3 backend.
//! Tests different graph sizes and topologies to identify crossover points.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rand::SeedableRng;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::algorithm::parallel_bfs::{BfsConfig, parallel_bfs};
use sqlitegraph::{EdgeSpec, GraphBackend, NodeSpec};
use tempfile::TempDir;

/// Create a chain graph: 0 -> 1 -> 2 -> ... -> (n-1)
fn create_chain_graph(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create nodes
    for i in 0..n {
        let node = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!(null),
        };
        let id = backend.insert_node(node).expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create edges to form a chain
    for i in 0..node_ids.len() - 1 {
        let edge = EdgeSpec {
            from: node_ids[i],
            to: node_ids[i + 1],
            edge_type: "chain".to_string(),
            data: serde_json::json!(null),
        };
        backend.insert_edge(edge).expect("Failed to insert edge");
    }

    node_ids
}

/// Create a star graph: center connected to all other nodes
fn create_star_graph(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create nodes
    for i in 0..n {
        let node = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!(null),
        };
        let id = backend.insert_node(node).expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create star edges (center node 0 connected to all others)
    for i in 1..n {
        let edge = EdgeSpec {
            from: node_ids[0],
            to: node_ids[i],
            edge_type: "star".to_string(),
            data: serde_json::json!(null),
        };
        backend.insert_edge(edge).expect("Failed to insert edge");
    }

    node_ids
}

/// Create a random graph with specified number of edges
fn create_random_graph(backend: &V3Backend, n: usize, edge_count: usize) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create nodes
    for i in 0..n {
        let node = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!(null),
        };
        let id = backend.insert_node(node).expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create random edges
    use rand::RngCore;
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);

    for _ in 0..edge_count {
        let from_idx = (rng.next_u32() as usize) % n;
        let mut to_idx = (rng.next_u32() as usize) % n;
        while to_idx == from_idx {
            to_idx = (rng.next_u32() as usize) % n;
        }

        let edge = EdgeSpec {
            from: node_ids[from_idx],
            to: node_ids[to_idx],
            edge_type: "random".to_string(),
            data: serde_json::json!(null),
        };
        backend.insert_edge(edge).expect("Failed to insert edge");
    }

    node_ids
}

/// Benchmark sequential vs parallel BFS on chain graphs
fn bench_bfs_chain(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parallel_bfs_chain");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));

    // Test different graph sizes
    for &size in &[100, 1_000, 10_000, 50_000] {
        group.throughput(Throughput::Elements(size as u64));

        // Sequential BFS (forced by high min_parallel_size)
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let db_path = temp_dir.path().join("benchmark.db");
                let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                let node_ids = create_chain_graph(&backend, size);

                // Force sequential execution
                let config = BfsConfig {
                    max_threads: None,
                    min_parallel_size: size * 10, // Force sequential
                };

                let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                    .expect("Failed to perform BFS");
            });
        });

        // Parallel BFS with default config
        group.bench_with_input(
            BenchmarkId::new("parallel_default", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    let db_path = temp_dir.path().join("benchmark.db");
                    let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                    let node_ids = create_chain_graph(&backend, size);

                    let _result =
                        parallel_bfs(&backend, node_ids[0], None).expect("Failed to perform BFS");
                });
            },
        );

        // Parallel BFS with 4 threads
        group.bench_with_input(
            BenchmarkId::new("parallel_4threads", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    let db_path = temp_dir.path().join("benchmark.db");
                    let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                    let node_ids = create_chain_graph(&backend, size);

                    let config = BfsConfig {
                        max_threads: Some(4),
                        min_parallel_size: 1000,
                    };

                    let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                        .expect("Failed to perform BFS");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark sequential vs parallel BFS on star graphs
fn bench_bfs_star(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parallel_bfs_star");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));

    // Star graphs test parallel processing of wide levels
    for &size in &[100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));

        // Sequential BFS
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let db_path = temp_dir.path().join("benchmark.db");
                let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                let node_ids = create_star_graph(&backend, size);

                let config = BfsConfig {
                    max_threads: None,
                    min_parallel_size: size * 10, // Force sequential
                };

                let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                    .expect("Failed to perform BFS");
            });
        });

        // Parallel BFS with 4 threads
        group.bench_with_input(
            BenchmarkId::new("parallel_4threads", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    let db_path = temp_dir.path().join("benchmark.db");
                    let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                    let node_ids = create_star_graph(&backend, size);

                    let config = BfsConfig {
                        max_threads: Some(4),
                        min_parallel_size: 1000,
                    };

                    let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                        .expect("Failed to perform BFS");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark sequential vs parallel BFS on random graphs
fn bench_bfs_random(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parallel_bfs_random");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));

    for &size in &[100, 1_000, 10_000] {
        let edge_count = size * 3; // 3x edges for connectivity
        group.throughput(Throughput::Elements(size as u64));

        // Sequential BFS
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let db_path = temp_dir.path().join("benchmark.db");
                let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                let node_ids = create_random_graph(&backend, size, edge_count);

                let config = BfsConfig {
                    max_threads: None,
                    min_parallel_size: size * 10, // Force sequential
                };

                let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                    .expect("Failed to perform BFS");
            });
        });

        // Parallel BFS with 4 threads
        group.bench_with_input(
            BenchmarkId::new("parallel_4threads", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    let db_path = temp_dir.path().join("benchmark.db");
                    let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                    let node_ids = create_random_graph(&backend, size, edge_count);

                    let config = BfsConfig {
                        max_threads: Some(4),
                        min_parallel_size: 1000,
                    };

                    let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                        .expect("Failed to perform BFS");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark to find the crossover point where parallel becomes faster
fn bench_crossover_analysis(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_analysis");
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(2));

    // Test small graph sizes to find crossover point
    for &size in &[100, 500, 1_000, 2_000, 5_000] {
        group.throughput(Throughput::Elements(size as u64));

        // Sequential BFS
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let db_path = temp_dir.path().join("benchmark.db");
                let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                let node_ids = create_chain_graph(&backend, size);

                let config = BfsConfig {
                    max_threads: None,
                    min_parallel_size: size * 10, // Force sequential
                };

                let _result = parallel_bfs(&backend, node_ids[0], Some(config))
                    .expect("Failed to perform BFS");
            });
        });

        // Parallel BFS
        group.bench_with_input(BenchmarkId::new("parallel", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let db_path = temp_dir.path().join("benchmark.db");
                let backend = V3Backend::create(&db_path).expect("Failed to create backend");

                let node_ids = create_chain_graph(&backend, size);

                let _result =
                    parallel_bfs(&backend, node_ids[0], None).expect("Failed to perform BFS");
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_bfs_chain,
    bench_bfs_star,
    bench_bfs_random,
    bench_crossover_analysis
);
criterion_main!(benches);
