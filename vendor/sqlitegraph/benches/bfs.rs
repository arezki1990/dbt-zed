//! BFS (Breadth-First Search) performance benchmarks for SQLite vs Native backends.
//!
//! Compares BFS traversal performance across different graph sizes and topologies
//! using the criterion benchmarking framework.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rand::SeedableRng;
use sqlitegraph::{EdgeSpec, NodeSpec, SnapshotId};

mod bench_utils;
use bench_utils::{
    BENCHMARK_SIZES, BenchInMemoryGraph, BenchmarkGraph, GraphTopology, MEASURE, WARM_UP,
    create_benchmark_temp_dir,
};

/// Benchmark BFS traversal on chain graphs
fn bfs_chain(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("bfs_chain");
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

                // Create chain graph using individual insertions
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

                // Perform BFS from first node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], size as u32)
                    .expect("Failed to perform BFS");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create chain graph using individual insertions
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

                    // DEBUG: Track node ID allocation pattern
                    if std::env::var("BFS_DEBUG").is_ok() {
                        println!("[BFS_DEBUG] Created node index={} -> node_id={}", i, node_id);
                    }

                    node_ids.push(node_id);
                }

                // SLOT CORRUPTION DEBUG: Check critical node slots after node creation, before edge creation
                if std::env::var("SLOT_CORRUPTION_DEBUG").is_ok() {
                    // Check node_id=257 specifically (if it exists)
                    if size > 257 {
                        println!("[BFS_TRANSITION] About to check node 257 status");

                        let graph_path = temp_dir.path().join("benchmark.db");
                        let debug_graph = sqlitegraph::open_graph(&graph_path, &sqlitegraph::GraphConfig::native())
                            .expect("Failed to open debug graph");

                        // Try to read node_id=257 directly to see its state
                        match debug_graph.get_node(SnapshotId::current(), 257) {
                            Ok(_) => println!("[BFS_CHECKPOINT] After node creation, before edges: node_id=257 EXISTS"),
                            Err(e) => println!("[BFS_CHECKPOINT] After node creation, before edges: node_id=257 MISSING - {:?}", e),
                        }

                        println!("[BFS_TRANSITION] About to start edge creation loop");
                    }
                }

                // Create chain edges
                for i in 0..size - 1 {
                    let max_created_node_id = *node_ids.iter().max().unwrap_or(&0);
                    assert!(node_ids[i] <= max_created_node_id, "EDGE {} references non-existent FROM node {} (max created: {})", i, node_ids[i], max_created_node_id);
                    assert!(node_ids[i + 1] <= max_created_node_id, "EDGE {} references non-existent TO node {} (max created: {})", i, node_ids[i + 1], max_created_node_id);

                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[i],
                            to: node_ids[i + 1],
                            edge_type: "chain".to_string(),
                            data: serde_json::json!({"order": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform BFS from first node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], size as u32)
                    .expect("Failed to perform BFS");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, size - 1, GraphTopology::Chain);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple BFS implementation
                let mut visited = vec![false; graph.node_count()];
                let mut queue = vec![0u32];
                visited[0] = true;
                while let Some(node) = queue.pop() {
                    for &neighbor in graph.neighbors(node) {
                        if !visited[neighbor as usize] {
                            visited[neighbor as usize] = true;
                            queue.push(neighbor);
                        }
                    }
                }
            });
        });
    }

    group.finish();
}

/// Benchmark BFS traversal on star graphs
fn bfs_star(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("bfs_star");
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

                // Create star graph using individual insertions
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

                // Create star edges (center node 0 connected to all others)
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "star".to_string(),
                            data: serde_json::json!({"spoke": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform BFS from center node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 2)
                    .expect("Failed to perform BFS");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create star graph using individual insertions
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

                // Create star edges (center node 0 connected to all others)
                for i in 1..size {
                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[0],
                            to: node_ids[i],
                            edge_type: "star".to_string(),
                            data: serde_json::json!({"spoke": i}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform BFS from center node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 2)
                    .expect("Failed to perform BFS");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, size - 1, GraphTopology::Star);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple BFS implementation
                let mut visited = vec![false; graph.node_count()];
                let mut queue = vec![0u32];
                visited[0] = true;
                while let Some(node) = queue.pop() {
                    for &neighbor in graph.neighbors(node) {
                        if !visited[neighbor as usize] {
                            visited[neighbor as usize] = true;
                            queue.push(neighbor);
                        }
                    }
                }
            });
        });
    }

    group.finish();
}

/// Benchmark BFS traversal on random graphs
fn bfs_random(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("bfs_random");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 1_000] {
        // Smaller sizes for random graphs
        let edge_count = size * 2; // 2x edges for random connectivity

        // SQLite backend
        group.bench_with_input(BenchmarkId::new("sqlite", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::sqlite())
                    .expect("Failed to create graph");

                // Create random graph using individual insertions
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

                // Create random edges
                use rand::RngCore;
                let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);

                for _ in 0..edge_count {
                    let from_idx = (rng.next_u32() as usize) % size;
                    let mut to_idx = (rng.next_u32() as usize) % size;
                    while to_idx == from_idx {
                        to_idx = (rng.next_u32() as usize) % size;
                    }

                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[from_idx],
                            to: node_ids[to_idx],
                            edge_type: "random".to_string(),
                            data: serde_json::json!({"random_id": rng.next_u64()}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform BFS from first node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 3)
                    .expect("Failed to perform BFS");
            });
        });

        // Native backend
        group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = create_benchmark_temp_dir();
                let db_path = temp_dir.path().join("benchmark.db");

                let graph = sqlitegraph::open_graph(&db_path, &sqlitegraph::GraphConfig::native())
                    .expect("Failed to create graph");

                // Create random graph using individual insertions
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

                // Create random edges
                use rand::RngCore;
                let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);

                for _ in 0..edge_count {
                    let from_idx = (rng.next_u32() as usize) % size;
                    let mut to_idx = (rng.next_u32() as usize) % size;
                    while to_idx == from_idx {
                        to_idx = (rng.next_u32() as usize) % size;
                    }

                    graph
                        .insert_edge(EdgeSpec {
                            from: node_ids[from_idx],
                            to: node_ids[to_idx],
                            edge_type: "random".to_string(),
                            data: serde_json::json!({"random_id": rng.next_u64()}),
                        })
                        .expect("Failed to insert edge");
                }

                // Perform BFS from first node
                let _bfs_result = graph
                    .bfs(SnapshotId::current(), node_ids[0], 3)
                    .expect("Failed to perform BFS");
                std::mem::forget(temp_dir); // Prevent TempDir deletion during benchmark (V2 backend uses async file ops)
            });
        });

        // In-memory CPU-only ceiling
        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, &size| {
            b.iter(|| {
                let spec = BenchmarkGraph::new(size, edge_count, GraphTopology::Random);
                let graph = BenchInMemoryGraph::from_spec(&spec);

                // Simple BFS implementation
                let mut visited = vec![false; graph.node_count()];
                let mut queue = vec![0u32];
                visited[0] = true;
                while let Some(node) = queue.pop() {
                    for &neighbor in graph.neighbors(node) {
                        if !visited[neighbor as usize] {
                            visited[neighbor as usize] = true;
                            queue.push(neighbor);
                        }
                    }
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bfs_chain, bfs_star, bfs_random);
criterion_main!(benches);
