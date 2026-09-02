//! Curated SQLite vs Native V3 benchmark suite.
//!
//! Run with: cargo bench --features native-v3 --bench sqlite_v3_curated
//!
//! This suite keeps only the highest-signal small-case comparisons so a normal
//! developer run finishes quickly while still producing meaningful backend data.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tempfile::TempDir;

use sqlitegraph::algo::backend::bfs_traversal;
use sqlitegraph::{
    backend::native::v3::{KvValue, V3Backend},
    backend::{BackendDirection, GraphBackend, NeighborQuery, SqliteGraphBackend},
    graph::SqliteGraph,
    snapshot::SnapshotId,
};

mod bench_utils;
use bench_utils::create_v3_bench_context;
mod graph_generators;
use graph_generators::{GraphData, GraphTopology};

const SMALL_NAME: &str = "small";
const SMALL_NODES: usize = 1_000;
const SMALL_EDGES: usize = 5_000;

fn small_random_graph() -> GraphData {
    GraphTopology::Random.generate(SMALL_NODES, SMALL_EDGES)
}

fn small_chain_graph() -> GraphData {
    GraphTopology::Chain.generate(SMALL_NODES, 0)
}

fn bench_insert_nodes(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/write_insert_nodes");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let graph_data = small_chain_graph();
    group.throughput(Throughput::Elements(SMALL_NODES as u64));

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || (SqliteGraphBackend::in_memory().unwrap(), data.nodes.clone()),
                |(backend, nodes)| {
                    for spec in nodes {
                        black_box(backend.insert_node(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || (create_v3_bench_context("v3.db"), data.nodes.clone()),
                |(ctx, nodes)| {
                    for spec in nodes {
                        black_box(ctx.backend.insert_node(spec).unwrap());
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        },
    );

    group.finish();
}

fn bench_get_node(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/read_get_node");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    let graph_data = small_random_graph();

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    (backend, (data.node_count / 2) as i64)
                },
                |(backend, target_id)| {
                    black_box(backend.get_node(SnapshotId::current(), target_id).ok());
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    (ctx, (data.node_count / 2) as i64)
                },
                |(ctx, target_id)| {
                    black_box(ctx.backend.get_node(SnapshotId::current(), target_id).ok());
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_warm_get_node(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/read_warm_get_node");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    let graph_data = small_random_graph();

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            let backend = SqliteGraphBackend::in_memory().unwrap();
            populate_sqlite_backend(&backend, data);
            let node_count = data.node_count;
            let mut idx = 0usize;

            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                black_box(backend.get_node(SnapshotId::current(), target_id).ok());
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            let temp_dir = TempDir::new().unwrap();
            let backend = V3Backend::create(temp_dir.path().join("v3.db")).unwrap();
            populate_v3_backend(&backend, data);
            let node_count = data.node_count;
            let mut idx = 0usize;

            b.iter(|| {
                let target_id = (idx % node_count) as i64 + 1;
                idx = idx.wrapping_add(1);
                black_box(backend.get_node(SnapshotId::current(), target_id).ok());
            });
        },
    );

    group.finish();
}

fn bench_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/read_neighbors");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    let graph_data = small_random_graph();

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    (backend, (data.node_count / 2) as i64)
                },
                |(backend, target_id)| {
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), target_id, query)
                            .ok(),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    (ctx, (data.node_count / 2) as i64)
                },
                |(ctx, target_id)| {
                    let query = NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    };
                    black_box(
                        ctx.backend
                            .neighbors(SnapshotId::current(), target_id, query)
                            .ok(),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_bfs(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/traversal_bfs");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);
    group.throughput(Throughput::Elements(SMALL_NODES as u64));

    let graph_data = small_random_graph();

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let backend = SqliteGraphBackend::in_memory().unwrap();
                    populate_sqlite_backend(&backend, data);
                    backend
                },
                |backend| {
                    let result = bfs_traversal(&backend, 1).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::LargeInput,
            );
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    populate_v3_backend(&ctx.backend, data);
                    ctx
                },
                |ctx| {
                    let result = bfs_traversal(&ctx.backend, 1).unwrap();
                    black_box(result.len());
                },
                criterion::BatchSize::LargeInput,
            );
        },
    );

    group.finish();
}

fn bench_reopen_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/reopen_cost");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let graph_data = small_random_graph();

    group.bench_with_input(
        BenchmarkId::new("sqlite", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("sqlite.db");
                    {
                        let graph = SqliteGraph::open(&db_path).unwrap();
                        let backend = SqliteGraphBackend::from_graph(graph);
                        for spec in data.nodes.clone() {
                            backend.insert_node(spec).unwrap();
                        }
                        for spec in data.edges.clone() {
                            backend.insert_edge(spec).unwrap();
                        }
                    }
                    (temp_dir, db_path)
                },
                |(temp_dir, db_path)| {
                    let graph = SqliteGraph::open(&db_path).unwrap();
                    black_box(SqliteGraphBackend::from_graph(graph));
                    drop(temp_dir);
                },
                criterion::BatchSize::PerIteration,
            );
        },
    );

    group.bench_with_input(
        BenchmarkId::new("v3", SMALL_NAME),
        &graph_data,
        |b, data| {
            b.iter_batched(
                || {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("v3.db");
                    {
                        let backend = V3Backend::create(&db_path).unwrap();
                        for spec in data.nodes.clone() {
                            backend.insert_node(spec).unwrap();
                        }
                        for spec in data.edges.clone() {
                            backend.insert_edge(spec).unwrap();
                        }
                        backend.flush().unwrap();
                    }
                    (temp_dir, db_path)
                },
                |(temp_dir, db_path)| {
                    let backend = V3Backend::open(&db_path).unwrap();
                    black_box(&backend);
                    drop(backend);
                    drop(temp_dir);
                },
                criterion::BatchSize::PerIteration,
            );
        },
    );

    group.finish();
}

fn bench_kv_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("curated/kv_get");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    group.bench_function(BenchmarkId::new("v3", SMALL_NAME), |b| {
        b.iter_batched(
            || {
                let ctx = create_v3_bench_context("v3.db");
                for i in 0..SMALL_NODES {
                    let key = format!("key_{}", i).into_bytes();
                    ctx.backend.kv_set_v3(key, KvValue::Integer(i as i64), None);
                }
                (ctx, format!("key_{}", SMALL_NODES / 2))
            },
            |(ctx, target_key)| {
                black_box(
                    ctx.backend
                        .kv_get_v3(SnapshotId::current(), target_key.as_bytes()),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn populate_sqlite_backend(backend: &SqliteGraphBackend, data: &GraphData) {
    for node in &data.nodes {
        backend.insert_node(node.clone()).unwrap();
    }
    for edge in &data.edges {
        backend.insert_edge(edge.clone()).unwrap();
    }
}

fn populate_v3_backend(backend: &V3Backend, data: &GraphData) {
    let mut batch = backend.begin_batch();
    for node in &data.nodes {
        batch.insert_node(node.clone()).unwrap();
    }
    batch.commit().unwrap();

    let mut batch = backend.begin_batch();
    for edge in &data.edges {
        batch.insert_edge(edge.clone()).unwrap();
    }
    batch.commit().unwrap();
}

criterion_group!(
    benches,
    bench_insert_nodes,
    bench_get_node,
    bench_warm_get_node,
    bench_neighbors,
    bench_bfs,
    bench_reopen_cost,
    bench_kv_get,
);

criterion_main!(benches);
