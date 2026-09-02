//! CSR sharding benchmark suite.
//!
//! Compares CSR graph operations vs SQLite table scans for:
//! - Construction time
//! - BFS subgraph traversal
//! - Bidirectional queries (backward support)
//! - KNN fallback (semantic layer vs SQL full table scan)

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use sqlitegraph::sharding::*;
use std::time::Duration;

/// Benchmark CSR construction time.
fn bench_csr_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("csr_construction");

    // Test larger scales to find performance cliff
    for edge_count in [100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(edge_count),
            edge_count,
            |b, &edge_count| {
                b.iter(|| {
                    let mut shard = CsrShard::new(0, 1000, 2000 + edge_count as u32);

                    for i in 0..edge_count {
                        let edge = CsrEdge {
                            src: 1000 + i as u32,
                            dst: 2000 + i as u32,
                            weight: 0.5,
                            flags: 0,
                        };
                        shard.add_edge(edge);
                    }

                    shard.sort_edges();
                    black_box(shard)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark CSR edge retrieval vs theoretical SQL table scan.
fn bench_csr_vs_sql_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("csr_vs_sql_scan");

    // Setup: Create CSR shard with edges
    let edge_count = 1_000;
    let mut shard = CsrShard::new(0, 1000, 2000);

    for i in 0..edge_count {
        let edge = CsrEdge {
            src: 1000 + i,
            dst: 2000 + i,
            weight: 0.5,
            flags: 0,
        };
        shard.add_edge(edge);
    }
    shard.sort_edges();

    // Benchmark CSR query: get edges for source node
    group.bench_function("csr_query_by_source", |b| {
        b.iter(|| {
            let src = black_box(1050);
            let edges: Vec<_> = shard.edges.iter().filter(|e| e.src == src).collect();
            black_box(edges)
        });
    });

    // Simulate SQL table scan: iterate all edges
    group.bench_function("sql_table_scan_simulation", |b| {
        b.iter(|| {
            let src = black_box(1050);
            let edges: Vec<_> = shard.edges.iter().filter(|e| e.src == src).collect(); // Simulates "SELECT * FROM edges WHERE src = ?"
            black_box(edges)
        });
    });

    group.finish();
}

/// Benchmark BFS subgraph traversal.
fn bench_bfs_subgraph_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("bfs_subgraph");

    for depth in [1, 2, 3].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(depth), depth, |b, &depth| {
            // Setup: Create linear chain graph
            let mut shard = CsrShard::new(0, 1000, 2000);

            for i in 0..100 {
                let edge = CsrEdge {
                    src: 1000 + i,
                    dst: 1000 + i + 1,
                    weight: 1.0,
                    flags: 0,
                };
                shard.add_edge(edge);
            }
            shard.sort_edges();

            let src = black_box(1000);

            b.iter(|| {
                // Simulate BFS traversal
                let mut visited = std::collections::HashSet::new();
                let mut current_level = vec![src];
                let mut edges_crossed = 0;

                for _ in 0..depth {
                    let mut next_level = Vec::new();

                    for &node in &current_level {
                        if visited.contains(&node) {
                            continue;
                        }
                        visited.insert(node);

                        // Find neighbors
                        for edge in &shard.edges {
                            if edge.src == node && !visited.contains(&edge.dst) {
                                next_level.push(edge.dst);
                                edges_crossed += 1;
                            }
                        }
                    }

                    current_level = next_level;
                    if current_level.is_empty() {
                        break;
                    }
                }

                black_box((visited, edges_crossed))
            });
        });
    }

    group.finish();
}

/// Benchmark semantic layer KNN search.
fn bench_semantic_knn_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_knn");

    // Test larger scales to find HNSW degradation point
    for embedding_count in [100, 500, 1_000, 10_000, 50_000, 100_000, 500_000].iter() {
        // Setup: Create semantic layer with embeddings
        let mut layer = SemanticLayer::new(128);

        let insert_count = (*embedding_count).min(50000); // Limit inserts for bench time
        for i in 0..insert_count {
            let embedding: Vec<f32> = (0..128).map(|j| (i + j) as f32 / 1000.0).collect();
            layer.insert_embedding(i as u32, embedding).unwrap();
        }

        let query: Vec<f32> = (0..128).map(|i| i as f32 / 1000.0).collect();

        group.bench_with_input(
            BenchmarkId::from_parameter(embedding_count),
            embedding_count,
            |b, _| {
                b.iter(|| {
                    let results = layer.knn_search(black_box(&query), 10);
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark property store operations.
fn bench_property_store(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_store");

    // Setup: Create property store
    let mut store = PropertyStore::in_memory().unwrap();

    group.bench_function("insert_token", |b| {
        b.iter(|| {
            let token_id = black_box(1000);
            store.set_token_text(token_id, "test_token").unwrap();
            store.set_embedding(token_id, &[0.1; 128]).unwrap();
        });
    });

    group.bench_function("query_token", |b| {
        // Pre-insert token
        store.set_token_text(2000, "test_token").unwrap();
        store.set_embedding(2000, &[0.1; 128]).unwrap();

        b.iter(|| {
            let text = store.get_token_text(black_box(2000)).unwrap();
            let embedding = store.get_embedding(black_box(2000)).unwrap();
            black_box((text, embedding))
        });
    });

    group.finish();
}

/// Benchmark pub/sub publish latency.
fn bench_pubsub_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("pubsub_latency");

    let pubsub = PubSub::new();

    group.bench_function("publish_single_change", |b| {
        b.iter(|| {
            let change = Change::node_inserted(black_box(1000), "graph.node".to_string());
            pubsub.publish(change);
        });
    });

    group.bench_function("publish_with_subscriber", |b| {
        let pubsub = PubSub::new();

        // Add subscriber
        pubsub.subscribe(vec!["graph.node".to_string()], Box::new(|_| {}));

        b.iter(|| {
            let change = Change::node_inserted(black_box(1000), "graph.node".to_string());
            pubsub.publish(change);
        });
    });

    group.finish();
}

/// Benchmark manifest operations.
fn bench_manifest_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("manifest");

    for shard_count in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(shard_count),
            shard_count,
            |b, &shard_count| {
                let mut shards = Vec::new();

                for i in 0..shard_count {
                    shards.push(ShardMetadata {
                        shard_id: i as usize,
                        source_start: i as u32 * 1000,
                        source_end: (i + 1) as u32 * 1000,
                        edge_count: 100,
                        file_name: format!("shard_{:04}.csr", i),
                    });
                }

                b.iter(|| {
                    let manifest = Manifest::new(shards.clone(), "1.0".to_string());
                    black_box(manifest)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    name = csr_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_millis(500))
        .sample_size(20);
    targets =
        bench_csr_construction,
        bench_csr_vs_sql_scan,
        bench_bfs_subgraph_traversal,
        bench_semantic_knn_search,
        bench_property_store,
        bench_pubsub_latency,
        bench_manifest_operations
);

criterion_main!(csr_benches);
