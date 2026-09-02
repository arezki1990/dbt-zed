//! Focused diagnostic benches for native-v3 hot paths.
//!
//! These benches are intentionally narrow:
//! - HNSW/turbovec: prove the measured path starts with turbovec already built
//! - CSR: prove whether the measured path is a cache hit or a cache miss

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;

use sqlitegraph::backend::{EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;

mod bench_utils;
use bench_utils::create_v3_bench_context;

fn populate_hnsw_index(
    vector_count: usize,
    dimension: usize,
) -> (bench_utils::V3BenchContext, Vec<f32>) {
    let ctx = create_v3_bench_context(&format!("hnsw_diag_{}.db", vector_count));

    ctx.backend
        .create_hnsw_index("diag_index", dimension)
        .expect("Failed to create HNSW index");

    let query_vec: Vec<f32> = (0..dimension).map(|i| i as f32 * 0.01).collect();

    for i in 0..vector_count {
        let vec: Vec<f32> = (0..dimension)
            .map(|j| (j as f32 * 0.01 + i as f32).cos())
            .collect();
        let metadata = serde_json::json!({ "id": i, "node_id": i });

        ctx.backend
            .insert_hnsw_vector("diag_index", &vec, Some(metadata))
            .expect("Failed to insert vector");
    }

    (ctx, query_vec)
}

fn populate_csr_graph(
    db_name: &str,
    nodes: usize,
    edges_per_node: usize,
) -> (bench_utils::V3BenchContext, i64, NeighborQuery) {
    let ctx = create_v3_bench_context(db_name);
    let mut node_ids = Vec::new();

    for i in 0..nodes {
        let node_id = ctx
            .backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    for i in 0..nodes.saturating_sub(1) {
        let from = node_ids[i];
        for j in 0..edges_per_node {
            let to = node_ids[(i + j + 1) % node_ids.len()];
            ctx.backend
                .insert_edge(EdgeSpec {
                    from,
                    to,
                    edge_type: "LINKS".to_string(),
                    data: serde_json::json!({}),
                })
                .expect("Failed to insert edge");
        }
    }

    let center = node_ids[node_ids.len() / 2];
    let query = NeighborQuery {
        direction: sqlitegraph::backend::BackendDirection::Outgoing,
        edge_type: None,
    };

    (ctx, center, query)
}

pub fn bench_hnsw_turbovec_ready(c: &mut Criterion) {
    let mut group = c.benchmark_group("diag_hnsw_turbovec_ready");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(10);

    for (vector_count, dimension) in [(5000usize, 64usize), (10000, 64), (50000, 64)] {
        group.throughput(Throughput::Elements(vector_count as u64));

        group.bench_function(format!("{}_{}", vector_count, dimension), |b| {
            b.iter_batched(
                || {
                    let (ctx, query_vec) = populate_hnsw_index(vector_count, dimension);
                    let warm_vec: Vec<f32> =
                        (0..dimension).map(|i| (i as f32 * 0.01).sin()).collect();

                    let _ = ctx
                        .backend
                        .hnsw_vector_search("diag_index", &warm_vec, 10)
                        .expect("Warm search failed");

                    assert!(
                        ctx.backend
                            .hnsw_turbovec_ready("diag_index")
                            .expect("Failed to inspect turbovec state"),
                        "Diagnostic HNSW bench must enter measurement with turbovec already built"
                    );

                    (ctx, query_vec)
                },
                |(ctx, query_vec)| {
                    assert!(
                        ctx.backend
                            .hnsw_turbovec_ready("diag_index")
                            .expect("Failed to inspect turbovec state"),
                        "Measured HNSW path lost turbovec-ready state"
                    );

                    let results = ctx
                        .backend
                        .hnsw_vector_search("diag_index", &query_vec, 10)
                        .expect("Search failed");
                    black_box(results);
                    ctx
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

pub fn bench_csr_shared_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("diag_csr_shared_cache_hit");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (nodes, edges_per_node) in [(1000usize, 20usize), (10000, 20)] {
        group.throughput(Throughput::Elements(nodes as u64));

        group.bench_function(format!("{}_{}", nodes, edges_per_node), |b| {
            b.iter_batched(
                || {
                    let (ctx, center, query) =
                        populate_csr_graph("csr_diag_shared.db", nodes, edges_per_node);

                    let warm = ctx
                        .backend
                        .neighbors_shared(SnapshotId::current(), center, query.clone())
                        .expect("Failed to warm CSR shared path");
                    assert!(!warm.is_empty(), "Expected non-empty CSR row");
                    ctx.backend.reset_edge_cache_stats();

                    (ctx, center, query)
                },
                |(ctx, center, query)| {
                    let before = ctx.backend.edge_cache_stats();
                    let neighbors = ctx
                        .backend
                        .neighbors_shared(SnapshotId::current(), center, query)
                        .expect("Failed to get shared neighbors");
                    let after = ctx.backend.edge_cache_stats();

                    assert!(!neighbors.is_empty(), "Expected non-empty CSR row");
                    assert_eq!(
                        after.1, before.1,
                        "CSR shared diagnostic hit path incurred a cache miss"
                    );
                    assert!(
                        after.0 > before.0,
                        "CSR shared diagnostic hit path did not record a cache hit"
                    );

                    black_box(neighbors.len());
                    ctx
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

pub fn bench_csr_wrapper_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("diag_csr_wrapper_cache_hit");
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(10);

    for (nodes, edges_per_node) in [(1000usize, 20usize), (10000, 20)] {
        group.throughput(Throughput::Elements(nodes as u64));

        group.bench_function(format!("{}_{}", nodes, edges_per_node), |b| {
            b.iter_batched(
                || {
                    let (ctx, center, query) =
                        populate_csr_graph("csr_diag_wrapper.db", nodes, edges_per_node);

                    let warm = ctx
                        .backend
                        .neighbors_shared(SnapshotId::current(), center, query.clone())
                        .expect("Failed to warm CSR wrapper path");
                    assert!(!warm.is_empty(), "Expected non-empty CSR row");
                    ctx.backend.reset_edge_cache_stats();

                    (ctx, center, query)
                },
                |(ctx, center, query)| {
                    let before = ctx.backend.edge_cache_stats();
                    let neighbors = ctx
                        .backend
                        .neighbors(SnapshotId::current(), center, query)
                        .expect("Failed to get wrapped neighbors");
                    let after = ctx.backend.edge_cache_stats();

                    assert!(!neighbors.is_empty(), "Expected non-empty CSR row");
                    assert_eq!(
                        after.1, before.1,
                        "CSR wrapper diagnostic hit path incurred a cache miss"
                    );
                    assert!(
                        after.0 > before.0,
                        "CSR wrapper diagnostic hit path did not record a cache hit"
                    );

                    black_box(neighbors.len());
                    ctx
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(
    native_v3_diagnostics,
    bench_hnsw_turbovec_ready,
    bench_csr_shared_cache_hit,
    bench_csr_wrapper_cache_hit,
);
criterion_main!(native_v3_diagnostics);
