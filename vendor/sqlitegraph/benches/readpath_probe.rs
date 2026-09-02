//! Focused read-path microbenchmark: V3 native vs SQLite backend.
//!
//! Isolates:
//!   - cold get_node (cache cleared / fresh backend per iter)
//!   - warm get_node (repeated lookups, cache hot)
//!   - neighbors (edge read path) cold + warm
//!   - reopen cost (the File::open-per-miss tax)
//!
//! Run: cargo bench --features native-v3 --bench readpath_probe
//!   or with --no-run to just compile.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tempfile::TempDir;

use serde_json::json;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{BackendDirection, GraphBackend, NeighborQuery, SqliteGraphBackend};
use sqlitegraph::graph::SqliteGraph;
use sqlitegraph::snapshot::SnapshotId;
use sqlitegraph::{EdgeSpec, NodeSpec};

const NODES: usize = 1_000;
const EDGES: usize = 5_000;

/// Build a random-ish graph with deterministic IDs.
fn build_graph() -> (Vec<NodeSpec>, Vec<EdgeSpec>) {
    let mut nodes = Vec::with_capacity(NODES);
    for i in 0..NODES {
        nodes.push(NodeSpec {
            kind: "symbol".to_string(),
            name: format!("node_{i}"),
            file_path: None,
            data: json!(null),
        });
    }
    let mut edges = Vec::with_capacity(EDGES);
    // Simple LCG for determinism without a dep.
    let mut state: u64 = 0x9E3779B97F4A7C15;
    for _ in 0..EDGES {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let from = (state >> 33) as usize % NODES + 1;
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let to = (state >> 33) as usize % NODES + 1;
        if from == to {
            continue;
        }
        edges.push(EdgeSpec {
            from: from as i64,
            to: to as i64,
            edge_type: "calls".to_string(),
            data: json!(null),
        });
    }
    (nodes, edges)
}

fn populate_sqlite(backend: &SqliteGraphBackend, nodes: &[NodeSpec], edges: &[EdgeSpec]) {
    for n in nodes {
        backend.insert_node(n.clone()).unwrap();
    }
    for e in edges {
        let _ = backend.insert_edge(e.clone());
    }
}

fn populate_v3(backend: &V3Backend, nodes: &[NodeSpec], edges: &[EdgeSpec]) {
    let mut batch = backend.begin_batch();
    for n in nodes {
        batch.insert_node(n.clone()).unwrap();
    }
    batch.commit().unwrap();
    let mut batch = backend.begin_batch();
    for e in edges {
        let _ = batch.insert_edge(e.clone());
    }
    batch.commit().unwrap();
}

fn bench_warm_get_node(c: &mut Criterion) {
    let mut g = c.benchmark_group("probe/read_warm_get_node");
    g.measurement_time(Duration::from_secs(3));
    g.sample_size(30);
    let (nodes, edges) = build_graph();

    g.bench_function("sqlite", |b| {
        let backend = SqliteGraphBackend::in_memory().unwrap();
        populate_sqlite(&backend, &nodes, &edges);
        let mut i = 0usize;
        b.iter(|| {
            let id = (i % NODES) as i64 + 1;
            i = i.wrapping_add(1);
            black_box(backend.get_node(SnapshotId::current(), id).ok());
        });
    });

    g.bench_function("v3", |b| {
        let tmp = TempDir::new().unwrap();
        let backend = V3Backend::create(tmp.path().join("v3.db")).unwrap();
        populate_v3(&backend, &nodes, &edges);
        let mut i = 0usize;
        b.iter(|| {
            let id = (i % NODES) as i64 + 1;
            i = i.wrapping_add(1);
            black_box(backend.get_node(SnapshotId::current(), id).ok());
        });
    });
    g.finish();
}

fn bench_cold_get_node(c: &mut Criterion) {
    // Each iter: fresh backend, single lookup = guaranteed cache miss on v3.
    let mut g = c.benchmark_group("probe/read_cold_get_node");
    g.measurement_time(Duration::from_secs(4));
    g.sample_size(10);
    let (nodes, edges) = build_graph();

    g.bench_function("sqlite", |b| {
        b.iter_batched(
            || {
                let backend = SqliteGraphBackend::in_memory().unwrap();
                populate_sqlite(&backend, &nodes, &edges);
                backend
            },
            |backend| {
                black_box(backend.get_node(SnapshotId::current(), 500).ok());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    g.bench_function("v3", |b| {
        b.iter_batched(
            || {
                let tmp = TempDir::new().unwrap();
                let backend = V3Backend::create(tmp.path().join("v3.db")).unwrap();
                populate_v3(&backend, &nodes, &edges);
                backend
            },
            |backend| {
                black_box(backend.get_node(SnapshotId::current(), 500).ok());
            },
            criterion::BatchSize::SmallInput,
        );
    });
    g.finish();
}

fn bench_neighbors_warm(c: &mut Criterion) {
    let mut g = c.benchmark_group("probe/read_neighbors_warm");
    g.measurement_time(Duration::from_secs(3));
    g.sample_size(30);
    let (nodes, edges) = build_graph();
    let q = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    g.bench_function("sqlite", |b| {
        let backend = SqliteGraphBackend::in_memory().unwrap();
        populate_sqlite(&backend, &nodes, &edges);
        let mut i = 0usize;
        b.iter(|| {
            let id = (i % NODES) as i64 + 1;
            i = i.wrapping_add(1);
            black_box(backend.neighbors(SnapshotId::current(), id, q.clone()).ok());
        });
    });

    g.bench_function("v3", |b| {
        let tmp = TempDir::new().unwrap();
        let backend = V3Backend::create(tmp.path().join("v3.db")).unwrap();
        populate_v3(&backend, &nodes, &edges);
        let mut i = 0usize;
        b.iter(|| {
            let id = (i % NODES) as i64 + 1;
            i = i.wrapping_add(1);
            black_box(backend.neighbors(SnapshotId::current(), id, q.clone()).ok());
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_warm_get_node,
    bench_cold_get_node,
    bench_neighbors_warm,
    bench_reopen_read_miss
);
criterion_main!(benches);

/// Isolate cold READ cost (no population in the timed region).
/// Create + populate once, reopen with empty caches, then time
/// sequential get_node over the full range — every lookup is a disk miss
/// until the tiny 64-page cache warms.
fn bench_reopen_read_miss(c: &mut Criterion) {
    let mut g = c.benchmark_group("probe/reopen_read_miss_seq");
    g.measurement_time(Duration::from_secs(4));
    g.sample_size(20);
    let (nodes, edges) = build_graph();

    // --- sqlite: reopen then sequential read ---
    let sqlite_tmp = TempDir::new().unwrap();
    let sqlite_path = sqlite_tmp.path().join("s.db");
    {
        // populate on a file-backed backend, then drop to flush.
        let backend = SqliteGraphBackend::from_graph(SqliteGraph::open(&sqlite_path).unwrap());
        populate_sqlite(&backend, &nodes, &edges);
    }
    g.bench_function("sqlite", |b| {
        b.iter_batched(
            || {
                // Fresh reopen = cold page cache (OS cache may still hit).
                SqliteGraphBackend::from_graph(SqliteGraph::open(&sqlite_path).unwrap())
            },
            |backend| {
                for id in 1..=200 {
                    black_box(backend.get_node(SnapshotId::current(), id).ok());
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // --- v3: reopen then sequential read ---
    let v3_tmp = TempDir::new().unwrap();
    let v3_path = v3_tmp.path().join("v3.db");
    {
        let backend = V3Backend::create(v3_path.clone()).unwrap();
        populate_v3(&backend, &nodes, &edges);
    }
    g.bench_function("v3", |b| {
        b.iter_batched(
            || V3Backend::open(v3_path.clone()).unwrap(),
            |backend| {
                for id in 1..=200 {
                    black_box(backend.get_node(SnapshotId::current(), id).ok());
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });
    g.finish();
}
