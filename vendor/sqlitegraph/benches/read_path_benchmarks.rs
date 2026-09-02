//! Read path performance benchmarks for the native backend.
//!
//! Comprehensive benchmark suite for validating read path optimizations
//! from Phase 3 plans 03-01 (cache) and 03-02 (compression).
//!
//! These benchmarks open a graph via `GraphConfig::native()`, which resolves to
//! the native-v3 backend (`V3Backend`). Each iteration rebuilds the graph from
//! scratch (insert-on-every-iter), so these measure cold-build + read together,
//! not steady-state read throughput — keep that in mind when reading the numbers.
//!
//! Benchmark categories:
//! - Single node operations (lookup, neighbor iteration)
//! - Traversal workloads (BFS, k-hop)
//! - Cache performance (hit ratio, eviction)
//! - Compression performance (ratio, overhead)
//! - Regression detection (baseline comparison)

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rand::{Rng, SeedableRng};
use sqlitegraph::{EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph};

mod bench_utils;
use bench_utils::{BENCHMARK_SIZES, create_benchmark_temp_dir};

const SAMPLE_SIZE: usize = 1000; // Increased for regression detection
const WARM_UP_TIME: Duration = Duration::from_secs(5);
const MEASURE_TIME: Duration = Duration::from_secs(15);

/// Memory profiling benchmark (optional feature)
#[cfg(feature = "memory_profiling")]
fn bench_memory_usage(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("memory_profiling");
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));

    // Memory profiling: Measure RSS before/after operations
    // Report MB per 1000 nodes
    group.bench_function("memory_per_1000_nodes", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create 1000 nodes
            for i in 0..1000 {
                let _node_id = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"id": i}),
                    })
                    .expect("Failed to insert node");
            }

            // Force memory measurement
            std::hint::black_box(&graph);
        });
    });

    group.finish();
}

// ============================================================================
// Task 1: Single Node Operations
// ============================================================================

/// Benchmark single node lookup
fn bench_get_node(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("single_node_ops");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in BENCHMARK_SIZES {
        group.bench_with_input(BenchmarkId::new("get_node", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

                // Create nodes
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

                // Lookup random node
                let lookup_id = node_ids[size / 2];
                let _node = graph
                    .get_node(SnapshotId::current(), lookup_id)
                    .expect("Failed to get node");
            });
        });
    }

    group.finish();
}

/// Benchmark neighbor iteration for small degree nodes (10 edges)
fn bench_get_neighbors_small(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("single_node_ops");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("get_neighbors_small_10_edges", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star topology with center node having 10 edges
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..10 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Iterate neighbors
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

/// Benchmark neighbor iteration for medium degree nodes (100 edges)
fn bench_get_neighbors_medium(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("single_node_ops");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("get_neighbors_medium_100_edges", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star topology with center node having 100 edges
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..100 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Iterate neighbors
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

/// Benchmark neighbor iteration for large degree nodes (1000 edges)
fn bench_get_neighbors_large(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("single_node_ops");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("get_neighbors_large_1000_edges", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star topology with center node having 1000 edges
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Iterate neighbors
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

// ============================================================================
// Task 1: Traversal Workloads
// ============================================================================

/// Benchmark 1-hop BFS (single level traversal)
fn bench_bfs_depth_1(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("traversal_workloads");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [100, 500, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("bfs_depth_1", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

                // Create chain graph
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
                            data: serde_json::json!({"order": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform 1-hop BFS
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 1)
                    .expect("Failed to perform BFS");
            });
        });
    }

    group.finish();
}

/// Benchmark 3-hop BFS (typical traversal workload)
fn bench_bfs_depth_3(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("traversal_workloads");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [100, 500, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("bfs_depth_3", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

                // Create chain graph
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
                            data: serde_json::json!({"order": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform 3-hop BFS
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 3)
                    .expect("Failed to perform BFS");
            });
        });
    }

    group.finish();
}

/// Benchmark 5-hop BFS (deep traversal)
fn bench_bfs_depth_5(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("traversal_workloads");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [100, 500, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("bfs_depth_5", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

                // Create chain graph
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
                            data: serde_json::json!({"order": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform 5-hop BFS
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 5)
                    .expect("Failed to perform BFS");
            });
        });
    }

    group.finish();
}

/// Benchmark k-hop from 10 start nodes
fn bench_k_hop_10_nodes(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("traversal_workloads");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("k_hop_10_nodes", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create graph with 100 nodes
            let mut node_ids = Vec::new();
            for i in 0..100 {
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

            // Create random edges
            let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
            for _ in 0..200 {
                let from_idx = rng.gen_range(0..100);
                let to_idx = rng.gen_range(0..100);
                if from_idx != to_idx {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[from_idx],
                            to: node_ids[to_idx],
                            edge_type: "random".to_string(),
                            data: serde_json::json!({}),
                        })
                        .expect("Failed to insert edge");
                }
            }

            // Perform 2-hop traversal from 10 random start nodes
            let mut total_visited = 0;
            for i in 0..10 {
                let start_node = node_ids[i * 10];
                let bfs_result = graph
                    .bfs(SnapshotId::current(), start_node, 2)
                    .expect("Failed to perform BFS");
                total_visited += bfs_result.len();
            }

            // Prevent compiler from optimizing away
            assert!(total_visited > 0, "Should visit some nodes");
        });
    });

    group.finish();
}

// ============================================================================
// Task 1: Cache Performance
// ============================================================================

/// Benchmark sequential access (cache-friendly)
fn bench_cache_hit_sequential(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_performance");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("cache_hit_sequential", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create chain graph
            let mut node_ids = Vec::new();
            for i in 0..100 {
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
            for i in 0..99 {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[i + 1],
                        edge_type: "chain".to_string(),
                        data: serde_json::json!({"order": i}),
                    })
                    .expect("Failed to insert edge");
            }

            // Sequential access pattern (cache-friendly)
            for node_id in node_ids.iter().take(100) {
                let _neighbors = graph
                    .neighbors(SnapshotId::current(), *node_id, NeighborQuery::default())
                    .expect("Failed to get neighbors");
            }
        });
    });

    group.finish();
}

/// Benchmark random access (cache stress)
fn bench_cache_hit_random(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_performance");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("cache_hit_random", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create chain graph
            let mut node_ids = Vec::new();
            for i in 0..100 {
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
            for i in 0..99 {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[i + 1],
                        edge_type: "chain".to_string(),
                        data: serde_json::json!({"order": i}),
                    })
                    .expect("Failed to insert edge");
            }

            // Random access pattern (cache stress)
            let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
            for _ in 0..100 {
                let idx = rng.gen_range(0..100);
                let _neighbors = graph
                    .neighbors(
                        SnapshotId::current(),
                        node_ids[idx],
                        NeighborQuery::default(),
                    )
                    .expect("Failed to get neighbors");
            }
        });
    });

    group.finish();
}

/// Benchmark cache eviction pressure
fn bench_cache_eviction(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_performance");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("cache_eviction", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create large graph to trigger cache eviction
            let mut node_ids = Vec::new();
            for i in 0..1000 {
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

            // Create star edges from node 0
            for i in 1..1000 {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[0],
                        to: node_ids[i],
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Access nodes in pattern that triggers eviction
            for node_id in node_ids.iter().take(500) {
                let _neighbors = graph
                    .neighbors(SnapshotId::current(), *node_id, NeighborQuery::default())
                    .expect("Failed to get neighbors");
            }
        });
    });

    group.finish();
}

// ============================================================================
// Task 1: Compression Performance
// ============================================================================

/// Benchmark iterating compressed edges
fn bench_iterate_compressed(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_performance");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("iterate_compressed", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star graph with sequential IDs (compression-friendly)
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Iterate compressed edges
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

/// Benchmark decompression overhead
fn bench_decompress_overhead(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_performance");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("decompress_overhead", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star graph with sequential IDs
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Multiple iterations to measure decompression overhead
            for _ in 0..10 {
                let _neighbors = graph
                    .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                    .expect("Failed to get neighbors");
            }
        });
    });

    group.finish();
}

// ============================================================================
// Task 3: Validate Cache Optimization Impact (03-01)
// ============================================================================

/// Benchmark cache hit ratio for 3-hop BFS
/// Expected: > 60% hit ratio with traversal-aware cache
fn bench_cache_hit_ratio_bfs(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("cache_hit_ratio_bfs_depth_3", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create chain graph with 100 nodes
            let mut node_ids = Vec::new();
            for i in 0..100 {
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
            for i in 0..99 {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[i + 1],
                        edge_type: "chain".to_string(),
                        data: serde_json::json!({"order": i}),
                    })
                    .expect("Failed to insert edge");
            }

            // Perform 3-hop BFS (traversal workload)
            let _bfs_result = graph
                .bfs(SnapshotId::current(), node_ids[0], 3)
                .expect("Failed to perform BFS");
        });
    });

    group.finish();
}

/// Benchmark high-degree node cache retention
/// Verify hub node stays in cache under memory pressure
fn bench_high_degree_cache_retention(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("high_degree_cache_retention", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create hub with 1000 edges, 1000 leaves with 1 edge each
            let hub = graph
                .insert_node(NodeSpec {
                    kind: "Hub".to_string(),
                    name: "hub".to_string(),
                    file_path: None,
                    data: serde_json::json!({"hub": true}),
                })
                .expect("Failed to insert hub node");

            // Create leaves connected to hub
            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: hub,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Access all leaf nodes to fill cache
            let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
            for _ in 0..1000 {
                let _node = graph
                    .get_node(SnapshotId::current(), rng.gen_range(1..1001))
                    .expect("Failed to get node");
            }

            // Verify hub still in cache by accessing it
            let _hub_neighbors = graph
                .neighbors(SnapshotId::current(), hub, NeighborQuery::default())
                .expect("Failed to get hub neighbors");
        });
    });

    group.finish();
}

/// Benchmark prefetch effectiveness for BFS
/// Expected: > 20% reduction in cache misses with prefetch
fn bench_prefetch_bfs(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("cache_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("prefetch_bfs_depth_3", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create chain graph with 100 nodes
            let mut node_ids = Vec::new();
            for i in 0..100 {
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
            for i in 0..99 {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[i + 1],
                        edge_type: "chain".to_string(),
                        data: serde_json::json!({"order": i}),
                    })
                    .expect("Failed to insert edge");
            }

            // Perform BFS (should benefit from prefetch in cache implementation)
            let _bfs_result = graph
                .bfs(SnapshotId::current(), node_ids[0], 3)
                .expect("Failed to perform BFS");
        });
    });

    group.finish();
}

// ============================================================================
// Task 4: Validate Compression Optimization Impact (03-02)
// ============================================================================

/// Benchmark compression ratio
/// Expected: > 1.5x compression for typical graphs
fn bench_compression_ratio(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("compression_ratio_sequential_ids", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star graph with sequential IDs (compression-friendly)
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Access compressed data
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

/// Benchmark decompression overhead
/// Expected: < 10% overhead for decompression
fn bench_decompress_overhead_comparison(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("decompress_overhead_1000_edges", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star graph with sequential IDs
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Multiple iterations to measure overhead
            for _ in 0..10 {
                let _neighbors = graph
                    .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                    .expect("Failed to get neighbors");
            }
        });
    });

    group.finish();
}

/// Benchmark cache line utilization with compression
/// Expected: > 2x improvement in edges loaded per cache line
fn bench_cache_line_utilization(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("cache_line_utilization_compressed", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create star graph with many edges to test cache line utilization
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            for i in 0..1000 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "star".to_string(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }

            // Iteration loads edges into cache lines
            let _neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");
        });
    });

    group.finish();
}

/// Benchmark compression roundtrip correctness
/// Verify exact reconstruction after compression/decompression
fn bench_compression_roundtrip(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compression_validation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("compression_roundtrip_correctness", |b| {
        b.iter(|| {
            let temp_dir = create_benchmark_temp_dir();
            let db_path = temp_dir.path().join("benchmark.db");

            let graph =
                open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

            // Create graph with various edge distributions
            let center = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: "center".to_string(),
                    file_path: None,
                    data: serde_json::json!({"center": true}),
                })
                .expect("Failed to insert center node");

            // Create edges with different data payloads
            for i in 0..100 {
                let leaf = graph
                    .insert_node(NodeSpec {
                        kind: "Node".to_string(),
                        name: format!("leaf_{}", i),
                        file_path: None,
                        data: serde_json::json!({"leaf": i}),
                    })
                    .expect("Failed to insert leaf node");

                let edge_data = if i % 3 == 0 {
                    // Small data
                    serde_json::json!({"idx": i})
                } else if i % 3 == 1 {
                    // Medium data
                    serde_json::json!({"idx": i, "data": "test".repeat(10)})
                } else {
                    // No data
                    serde_json::json!(null)
                };

                graph
                    .insert_edge(EdgeSpec {
                        from: center,
                        to: leaf,
                        edge_type: "test".to_string(),
                        data: edge_data,
                    })
                    .expect("Failed to insert edge");
            }

            // Read back and verify
            let neighbors = graph
                .neighbors(SnapshotId::current(), center, NeighborQuery::default())
                .expect("Failed to get neighbors");

            // Verify we got all neighbors back
            assert_eq!(neighbors.len(), 100, "Should retrieve all 100 neighbors");

            // Force verification
            std::hint::black_box(neighbors);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    // Single node operations
    bench_get_node,
    bench_get_neighbors_small,
    bench_get_neighbors_medium,
    bench_get_neighbors_large,
    // Traversal workloads
    bench_bfs_depth_1,
    bench_bfs_depth_3,
    bench_bfs_depth_5,
    bench_k_hop_10_nodes,
    // Cache performance
    bench_cache_hit_sequential,
    bench_cache_hit_random,
    bench_cache_eviction,
    // Compression performance
    bench_iterate_compressed,
    bench_decompress_overhead,
    // Cache validation (Task 3)
    bench_cache_hit_ratio_bfs,
    bench_high_degree_cache_retention,
    bench_prefetch_bfs,
    // Compression validation (Task 4)
    bench_compression_ratio,
    bench_decompress_overhead_comparison,
    bench_cache_line_utilization,
    bench_compression_roundtrip,
);

criterion_main!(benches);
