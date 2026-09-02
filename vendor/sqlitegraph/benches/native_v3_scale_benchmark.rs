//! Native-v3 Scale Testing — Finding Performance Cliff
//!
//! Tests native-v3 components at scale to identify where performance degrades.
//! Focus areas:
//! - CSR sharding capacity (edges before slowdown)
//! - Snapshot stats table growth impact
//! - Property store memory limits
//! - HNSW semantic layer search degradation
//! - Combined system stress test

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use sqlitegraph::sharding::*;
use sqlitegraph::{GraphEntity, SqliteGraph};
use std::time::Duration;

/// Test CSR construction scaling — find where it slows down
fn bench_csr_construction_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("csr_scale_construction");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    // Exponential scale to find cliff
    for edge_count in [1_000, 10_000, 100_000, 500_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(edge_count),
            edge_count,
            |b, &edge_count| {
                b.iter(|| {
                    let mut shard = CsrShard::new(0, 1000, 1000 + edge_count as u32);

                    for i in 0..edge_count {
                        let edge = CsrEdge {
                            src: 1000 + (i % 10000) as u32, // Limit source range
                            dst: 2000 + i as u32,
                            weight: 0.5,
                            flags: 0,
                        };
                        shard.add_edge(edge);
                    }

                    shard.sort_edges();
                    black_box(shard.edge_count())
                });
            },
        );
    }
    group.finish();
}

/// Test snapshot_stats table performance with many snapshots
fn bench_snapshot_stats_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_stats_scale");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for snapshot_count in [10, 100, 1_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(snapshot_count),
            snapshot_count,
            |b, &snapshot_count| {
                b.iter(|| {
                    let graph = SqliteGraph::open_in_memory().unwrap();

                    // Create many snapshots
                    for i in 0..snapshot_count {
                        let snapshot_id = format!("snapshot_{}", i);
                        let _timestamp = graph.create_snapshot(&snapshot_id).unwrap();

                        // Insert one entity per snapshot
                        let entities = vec![GraphEntity {
                            id: 0,
                            kind: "Test".to_string(),
                            name: format!("entity_{}", i),
                            file_path: None,
                            data: serde_json::json!({"idx": i}),
                        }];

                        let _ = graph.batch_insert_entities_with_snapshot(&entities, &snapshot_id);
                    }

                    // Query as of latest timestamp
                    let stats = graph.query_as_of(999999999).unwrap();
                    black_box(stats.total_entities)
                });
            },
        );
    }
    group.finish();
}

/// Test HNSW semantic layer search degradation
fn bench_semantic_layer_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_layer_scale");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(10);

    // Cap at 5K embeddings — 10K+ hits exponential cliff (>20s per KNN search)
    for embedding_count in [100, 1_000, 5_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(embedding_count),
            embedding_count,
            |b, &embedding_count| {
                b.iter(|| {
                    let mut layer = SemanticLayer::new(128);

                    // Insert subset to keep bench time reasonable
                    let insert_count = embedding_count.min(50000);
                    for i in 0..insert_count {
                        let embedding: Vec<f32> =
                            (0..128).map(|j| (i + j) as f32 / 100000.0).collect();
                        layer.insert_embedding(i as u32, embedding).unwrap();
                    }

                    let query: Vec<f32> = (0..128).map(|i| i as f32 / 100000.0).collect();
                    let results = layer.knn_search(black_box(&query), 10);
                    black_box(results.len())
                });
            },
        );
    }
    group.finish();
}

/// Test property store memory growth
fn bench_property_store_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_store_scale");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for token_count in [1_000, 10_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(token_count),
            token_count,
            |b, &token_count| {
                b.iter(|| {
                    let mut store = PropertyStore::in_memory().unwrap();

                    // Insert tokens
                    for i in 0..token_count {
                        let token_text = format!("token_{}", i % 1000); // Reuse text
                        store.set_token_text(i as u32, &token_text).unwrap();

                        // Set embedding for subset
                        if i % 10 == 0 {
                            store.set_embedding(i as u32, &[0.1; 128]).unwrap();
                        }
                    }

                    // Query random token
                    let query_id = (token_count / 2).max(1) as u32;
                    let text = store.get_token_text(black_box(query_id)).unwrap();
                    black_box(text)
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    name = scale_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .sample_size(10);
    targets =
        bench_csr_construction_scale,
        bench_snapshot_stats_scale,
        bench_semantic_layer_scale,
        bench_property_store_scale
);

criterion_main!(scale_benches);
