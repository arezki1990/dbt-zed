//! Phase 0.3 (Exp B) — Temporal version-chain benchmarks.
//!
//! PURPOSE: validate that `as_of(version)` historical reads are affordable
//! (p99 < 1ms at 1000 versions) and that the SCC persistence sweep is tractable
//! across the version chain. Decision gate per the substrate experiments plan.
//!
//! Workload: an evolving memory graph (mimics atheneum between dream runs) —
//! insert nodes/edges, churn some edges between each checkpoint, producing a
//! bounded version chain. Then measure:
//!
//! 1. `as_of(version)` binary-search + Arc clone latency over {10, 100, 1000}.
//! 2. `temporal_persistence_sweep` + `compute_temporal_barcode` over the chain.
//!
//! Scope: bench-only, Phase 0. Not referenced from src/.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde_json::json;
use sqlitegraph::{
    BackendDirection, GraphBackend, NeighborQuery, SqliteGraphBackend, compute_temporal_barcode,
    snapshot::SnapshotId, temporal::temporal_persistence_sweep,
};

/// Build a version chain of `n_versions` checkpoints on a live backend,
/// returning the backend (with history retained) and the list of version ids.
fn build_chain(n_versions: usize) -> (SqliteGraphBackend, Vec<u64>) {
    let backend = SqliteGraphBackend::in_memory().expect("backend");

    // Seed a base graph of 50 nodes connected in a ring (guarantees one SCC).
    let mut ids = Vec::new();
    for i in 0..50 {
        let id = backend
            .insert_node(sqlitegraph::NodeSpec {
                kind: "n".to_string(),
                name: format!("n{i}"),
                file_path: None,
                data: json!({}),
            })
            .expect("insert");
        ids.push(id);
    }
    // Ring edges.
    for w in ids.windows(2) {
        backend
            .insert_edge(sqlitegraph::EdgeSpec {
                from: w[0],
                to: w[1],
                edge_type: "ring".to_string(),
                data: json!({}),
            })
            .expect("edge");
    }
    backend
        .insert_edge(sqlitegraph::EdgeSpec {
            from: *ids.last().unwrap(),
            to: ids[0],
            edge_type: "ring".to_string(),
            data: json!({}),
        })
        .expect("close ring");

    // Warm the cache so the snapshot manager sees current adjacency.
    for &id in &ids {
        let _ = backend.neighbors(
            SnapshotId::current(),
            id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        );
    }

    let mut versions = Vec::with_capacity(n_versions);
    for step in 0..n_versions {
        // Churn: add a chord edge from node i to node (i+7)%50, making the
        // SCC landscape shift slightly each version.
        let from = ids[step % 50];
        let to = ids[(step + 7) % 50];
        backend
            .insert_edge(sqlitegraph::EdgeSpec {
                from,
                to,
                edge_type: "chord".to_string(),
                data: json!({}),
            })
            .expect("chord edge");
        // Re-warm the two affected nodes so the snapshot reflects the change.
        for &n in &[from, to] {
            let _ = backend.neighbors(
                SnapshotId::current(),
                n,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            );
        }
        versions.push(backend.graph().checkpoint());
    }

    (backend, versions)
}

fn bench_as_of(c: &mut Criterion) {
    let mut group = c.benchmark_group("temporal_as_of");
    for &n in &[10usize, 100, 1000] {
        let (backend, versions) = build_chain(n);
        let graph = backend.graph();
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &_| {
            b.iter(|| {
                // Look up a spread of versions (oldest, middle, newest).
                let mid = versions[versions.len() / 2];
                let _ = black_box(graph.snapshot_as_of(versions[0]));
                let _ = black_box(graph.snapshot_as_of(mid));
                let _ = black_box(graph.snapshot_as_of(*versions.last().unwrap()));
            })
        });
    }
    group.finish();
}

fn bench_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("temporal_sweep");
    for &n in &[10usize, 50, 100] {
        let (backend, _versions) = build_chain(n);
        let graph = backend.graph();
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &_| {
            b.iter(|| {
                let versions = graph.snapshot_versions();
                let points = temporal_persistence_sweep(&versions);
                let _bars = compute_temporal_barcode(&points);
                black_box(points.len());
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_as_of, bench_sweep);
criterion_main!(benches);
