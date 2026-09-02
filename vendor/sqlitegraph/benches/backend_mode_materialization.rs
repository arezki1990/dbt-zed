use std::path::PathBuf;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use sqlitegraph::backend::{
    BackendDirection, CombinedGraphBackend, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec,
};
use sqlitegraph::config::CombinedConfig;
use sqlitegraph::graph::SqliteGraph;
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

mod bench_utils;
use bench_utils::{MEASURE, SAMPLE_SIZE, WARM_UP, create_v3_bench_context};

const FANOUT: usize = 100;
const BRANCH_FACTOR: usize = 10;

struct GraphHandles {
    root: i64,
    leaf: i64,
    spare: i64,
    victim: i64,
}

struct CombinedDiskFixture {
    temp_dir: TempDir,
    db_path: PathBuf,
    handles: GraphHandles,
    config: CombinedConfig,
}

fn node(name: &str) -> NodeSpec {
    NodeSpec {
        kind: "Node".to_string(),
        name: name.to_string(),
        file_path: None,
        data: serde_json::json!({}),
    }
}

fn edge(from: i64, to: i64, edge_type: &str) -> EdgeSpec {
    EdgeSpec {
        from,
        to,
        edge_type: edge_type.to_string(),
        data: serde_json::json!({}),
    }
}

fn outgoing_query() -> NeighborQuery {
    NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    }
}

fn typed_outgoing_query(edge_type: &str) -> NeighborQuery {
    NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: Some(edge_type.to_string()),
    }
}

fn populate_graph<B: GraphBackend>(backend: &B) -> GraphHandles {
    let root = backend.insert_node(node("root")).expect("root");

    let mut first_leaf = None;
    let mut first_mid = None;
    for i in 0..FANOUT {
        let mid = backend
            .insert_node(node(&format!("mid_{i}")))
            .expect("mid node");
        if first_mid.is_none() {
            first_mid = Some(mid);
        }
        backend
            .insert_edge(edge(root, mid, "LINKS"))
            .expect("root->mid");
        for j in 0..BRANCH_FACTOR {
            let leaf = backend
                .insert_node(node(&format!("leaf_{i}_{j}")))
                .expect("leaf");
            backend
                .insert_edge(edge(mid, leaf, "LINKS"))
                .expect("mid->leaf");
            if first_leaf.is_none() {
                first_leaf = Some(leaf);
            }
        }
    }

    let spare = backend.insert_node(node("spare")).expect("spare");
    let victim = backend.insert_node(node("victim")).expect("victim");
    backend
        .insert_edge(edge(root, victim, "LINKS"))
        .expect("root->victim");

    GraphHandles {
        root,
        leaf: first_leaf.expect("leaf"),
        spare,
        victim: first_mid.expect("mid"),
    }
}

fn build_combined_graph(config: CombinedConfig) -> (CombinedGraphBackend, GraphHandles) {
    let backend = CombinedGraphBackend::in_memory_with_config(config).expect("combined backend");
    let handles = populate_graph(&backend);
    (backend, handles)
}

fn build_combined_disk_fixture(config: CombinedConfig, publish: bool) -> CombinedDiskFixture {
    let temp_dir = bench_utils::create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("combined_materialized.db");
    let graph = SqliteGraph::open(&db_path).expect("open sqlite graph");
    let backend = CombinedGraphBackend::from_graph_with_config(graph, config.clone());
    let handles = populate_graph(&backend);
    if publish {
        backend
            .publish_materialized_views()
            .expect("publish materialized views");
    }
    drop(backend);

    CombinedDiskFixture {
        temp_dir,
        db_path,
        handles,
        config,
    }
}

fn build_native_v3_graph() -> (bench_utils::V3BenchContext, i64) {
    let ctx = create_v3_bench_context("materialization_v3.db");
    let root = ctx.backend.insert_node(node("root")).expect("root");

    for i in 0..FANOUT {
        let linked = ctx
            .backend
            .insert_node(node(&format!("linked_{i}")))
            .expect("linked");
        ctx.backend
            .insert_edge(edge(root, linked, "LINKS"))
            .expect("links edge");

        let typed = ctx
            .backend
            .insert_node(node(&format!("typed_{i}")))
            .expect("typed");
        ctx.backend
            .insert_edge(edge(root, typed, "USES"))
            .expect("uses edge");
    }

    (ctx, root)
}

fn configure_group(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP);
    group.measurement_time(MEASURE);
}

fn configure_heavy_group(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(200));
    group.measurement_time(Duration::from_millis(400));
}

fn bench_combined_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_reads");
    configure_group(&mut group);

    let (sqlite_backend, sqlite_handles) = build_combined_graph(CombinedConfig::new());
    let (materialized_backend, materialized_handles) =
        build_combined_graph(CombinedConfig::new().with_materialized_reads());
    materialized_backend
        .publish_materialized_views()
        .expect("publish materialized views");

    group.bench_function(BenchmarkId::new("neighbors", "sqlite_only"), |b| {
        b.iter(|| {
            let backend = black_box(&sqlite_backend);
            let node = black_box(sqlite_handles.root);
            let query = black_box(outgoing_query());
            black_box(
                backend
                    .neighbors(SnapshotId::current(), node, query)
                    .expect("combined neighbors sqlite"),
            );
        });
    });

    group.bench_function(BenchmarkId::new("neighbors", "prefer_materialized"), |b| {
        b.iter(|| {
            let backend = black_box(&materialized_backend);
            let node = black_box(materialized_handles.root);
            let query = black_box(outgoing_query());
            black_box(
                backend
                    .neighbors(SnapshotId::current(), node, query)
                    .expect("combined neighbors materialized"),
            );
        });
    });

    group.bench_function(BenchmarkId::new("bfs_depth2", "sqlite_only"), |b| {
        b.iter(|| {
            let backend = black_box(&sqlite_backend);
            let start = black_box(sqlite_handles.root);
            black_box(
                backend
                    .bfs(SnapshotId::current(), start, black_box(2))
                    .expect("combined bfs sqlite"),
            );
        });
    });

    group.bench_function(BenchmarkId::new("bfs_depth2", "prefer_materialized"), |b| {
        b.iter(|| {
            let backend = black_box(&materialized_backend);
            let start = black_box(materialized_handles.root);
            black_box(
                backend
                    .bfs(SnapshotId::current(), start, black_box(2))
                    .expect("combined bfs materialized"),
            );
        });
    });

    group.bench_function(BenchmarkId::new("shortest_path", "sqlite_only"), |b| {
        b.iter(|| {
            let backend = black_box(&sqlite_backend);
            let start = black_box(sqlite_handles.root);
            let end = black_box(sqlite_handles.leaf);
            black_box(
                backend
                    .shortest_path(SnapshotId::current(), start, end)
                    .expect("combined shortest path sqlite"),
            );
        });
    });

    group.bench_function(
        BenchmarkId::new("shortest_path", "prefer_materialized"),
        |b| {
            b.iter(|| {
                let backend = black_box(&materialized_backend);
                let start = black_box(materialized_handles.root);
                let end = black_box(materialized_handles.leaf);
                black_box(
                    backend
                        .shortest_path(SnapshotId::current(), start, end)
                        .expect("combined shortest path materialized"),
                );
            });
        },
    );

    group.finish();
}

fn bench_combined_cold_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_cold_reads");
    configure_group(&mut group);

    group.bench_function(BenchmarkId::new("neighbors", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new()),
            |(backend, handles)| {
                black_box(
                    backend
                        .neighbors(
                            SnapshotId::current(),
                            black_box(handles.root),
                            black_box(outgoing_query()),
                        )
                        .expect("cold combined neighbors sqlite"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(BenchmarkId::new("neighbors", "prefer_materialized"), |b| {
        b.iter_batched(
            || {
                let built = build_combined_graph(CombinedConfig::new().with_materialized_reads());
                built
                    .0
                    .publish_materialized_views()
                    .expect("publish materialized");
                built
            },
            |(backend, handles)| {
                black_box(
                    backend
                        .neighbors(
                            SnapshotId::current(),
                            black_box(handles.root),
                            black_box(outgoing_query()),
                        )
                        .expect("cold combined neighbors materialized"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(BenchmarkId::new("bfs_depth2", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new()),
            |(backend, handles)| {
                black_box(
                    backend
                        .bfs(SnapshotId::current(), black_box(handles.root), black_box(2))
                        .expect("cold combined bfs sqlite"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(BenchmarkId::new("bfs_depth2", "prefer_materialized"), |b| {
        b.iter_batched(
            || {
                let built = build_combined_graph(CombinedConfig::new().with_materialized_reads());
                built
                    .0
                    .publish_materialized_views()
                    .expect("publish materialized");
                built
            },
            |(backend, handles)| {
                black_box(
                    backend
                        .bfs(SnapshotId::current(), black_box(handles.root), black_box(2))
                        .expect("cold combined bfs materialized"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(BenchmarkId::new("shortest_path", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new()),
            |(backend, handles)| {
                black_box(
                    backend
                        .shortest_path(
                            SnapshotId::current(),
                            black_box(handles.root),
                            black_box(handles.leaf),
                        )
                        .expect("cold combined shortest path sqlite"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(
        BenchmarkId::new("shortest_path", "prefer_materialized"),
        |b| {
            b.iter_batched(
                || {
                    let built =
                        build_combined_graph(CombinedConfig::new().with_materialized_reads());
                    built
                        .0
                        .publish_materialized_views()
                        .expect("publish materialized");
                    built
                },
                |(backend, handles)| {
                    black_box(
                        backend
                            .shortest_path(
                                SnapshotId::current(),
                                black_box(handles.root),
                                black_box(handles.leaf),
                            )
                            .expect("cold combined shortest path materialized"),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_combined_reopen_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_reopen_reads");
    configure_heavy_group(&mut group);

    group.bench_function(BenchmarkId::new("neighbors", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_disk_fixture(CombinedConfig::new(), false),
            |fixture| {
                let backend = CombinedGraphBackend::from_graph_with_config(
                    SqliteGraph::open(&fixture.db_path).expect("reopen sqlite graph"),
                    fixture.config.clone(),
                );
                black_box(
                    backend
                        .neighbors(
                            SnapshotId::current(),
                            black_box(fixture.handles.root),
                            black_box(outgoing_query()),
                        )
                        .expect("reopen neighbors sqlite"),
                );
                black_box(&fixture.temp_dir);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(BenchmarkId::new("neighbors", "prefer_materialized"), |b| {
        b.iter_batched(
            || build_combined_disk_fixture(CombinedConfig::new().with_materialized_reads(), true),
            |fixture| {
                let backend = CombinedGraphBackend::from_graph_with_config(
                    SqliteGraph::open(&fixture.db_path).expect("reopen sqlite graph"),
                    fixture.config.clone(),
                );
                black_box(
                    backend
                        .neighbors(
                            SnapshotId::current(),
                            black_box(fixture.handles.root),
                            black_box(outgoing_query()),
                        )
                        .expect("reopen neighbors materialized"),
                );
                black_box(&fixture.temp_dir);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_combined_publish_rebuild(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_publish_rebuild");
    configure_heavy_group(&mut group);

    group.bench_function("publish_materialized_views", |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new().with_materialized_reads()),
            |(backend, _)| {
                black_box(
                    backend
                        .publish_materialized_views()
                        .expect("publish materialized views"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_combined_mixed_workloads(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_mixed_workloads");
    configure_heavy_group(&mut group);

    group.bench_function(BenchmarkId::new("read_heavy_90_10", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new()),
            |(backend, handles)| {
                for _ in 0..5 {
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), handles.root, outgoing_query())
                            .expect("read-heavy neighbors sqlite"),
                    );
                }
                for _ in 0..2 {
                    black_box(
                        backend
                            .bfs(SnapshotId::current(), handles.root, 2)
                            .expect("read-heavy bfs sqlite"),
                    );
                    black_box(
                        backend
                            .shortest_path(SnapshotId::current(), handles.root, handles.leaf)
                            .expect("read-heavy shortest path sqlite"),
                    );
                }
                black_box(
                    backend
                        .insert_edge(edge(handles.root, handles.spare, "LINKS"))
                        .expect("read-heavy write sqlite"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(
        BenchmarkId::new("read_heavy_90_10", "prefer_materialized"),
        |b| {
            b.iter_batched(
                || {
                    let built =
                        build_combined_graph(CombinedConfig::new().with_materialized_reads());
                    built
                        .0
                        .publish_materialized_views()
                        .expect("publish materialized");
                    built
                },
                |(backend, handles)| {
                    for _ in 0..5 {
                        black_box(
                            backend
                                .neighbors(SnapshotId::current(), handles.root, outgoing_query())
                                .expect("read-heavy neighbors materialized"),
                        );
                    }
                    for _ in 0..2 {
                        black_box(
                            backend
                                .bfs(SnapshotId::current(), handles.root, 2)
                                .expect("read-heavy bfs materialized"),
                        );
                        black_box(
                            backend
                                .shortest_path(SnapshotId::current(), handles.root, handles.leaf)
                                .expect("read-heavy shortest path materialized"),
                        );
                    }
                    black_box(
                        backend
                            .insert_edge(edge(handles.root, handles.spare, "LINKS"))
                            .expect("read-heavy write materialized"),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.bench_function(BenchmarkId::new("balanced_50_50", "sqlite_only"), |b| {
        b.iter_batched(
            || build_combined_graph(CombinedConfig::new()),
            |(backend, handles)| {
                black_box(
                    backend
                        .neighbors(SnapshotId::current(), handles.root, outgoing_query())
                        .expect("balanced neighbors sqlite"),
                );
                black_box(
                    backend
                        .insert_edge(edge(handles.root, handles.spare, "LINKS"))
                        .expect("balanced insert sqlite"),
                );
                black_box(
                    backend
                        .delete_entity(handles.victim)
                        .expect("balanced delete sqlite"),
                );
                black_box(
                    backend
                        .bfs(SnapshotId::current(), handles.root, 2)
                        .expect("balanced bfs sqlite"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function(
        BenchmarkId::new("balanced_50_50", "prefer_materialized"),
        |b| {
            b.iter_batched(
                || {
                    let built =
                        build_combined_graph(CombinedConfig::new().with_materialized_reads());
                    built
                        .0
                        .publish_materialized_views()
                        .expect("publish materialized");
                    built
                },
                |(backend, handles)| {
                    black_box(
                        backend
                            .neighbors(SnapshotId::current(), handles.root, outgoing_query())
                            .expect("balanced neighbors materialized"),
                    );
                    black_box(
                        backend
                            .insert_edge(edge(handles.root, handles.spare, "LINKS"))
                            .expect("balanced insert materialized"),
                    );
                    black_box(
                        backend
                            .delete_entity(handles.victim)
                            .expect("balanced delete materialized"),
                    );
                    black_box(
                        backend
                            .bfs(SnapshotId::current(), handles.root, 2)
                            .expect("balanced bfs materialized"),
                    );
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_combined_incremental_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_incremental_writes");
    configure_group(&mut group);

    group.bench_function("insert_edge/sqlite_only", |b| {
        b.iter_batched(
            || {
                let backend = CombinedGraphBackend::in_memory_with_config(CombinedConfig::new())
                    .expect("combined sqlite-only");
                let a = backend.insert_node(node("a")).expect("a");
                let z = backend.insert_node(node("z")).expect("z");
                (backend, a, z)
            },
            |(backend, a, z)| {
                black_box(
                    backend
                        .insert_edge(edge(a, z, "LINKS"))
                        .expect("insert edge"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("insert_edge/prefer_materialized", |b| {
        b.iter_batched(
            || {
                let backend = CombinedGraphBackend::in_memory_with_config(
                    CombinedConfig::new().with_materialized_reads(),
                )
                .expect("combined materialized");
                let a = backend.insert_node(node("a")).expect("a");
                let z = backend.insert_node(node("z")).expect("z");
                (backend, a, z)
            },
            |(backend, a, z)| {
                black_box(
                    backend
                        .insert_edge(edge(a, z, "LINKS"))
                        .expect("insert edge"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("delete_entity/sqlite_only", |b| {
        b.iter_batched(
            || {
                let backend = CombinedGraphBackend::in_memory_with_config(CombinedConfig::new())
                    .expect("combined sqlite-only");
                let a = backend.insert_node(node("a")).expect("a");
                let b = backend.insert_node(node("b")).expect("b");
                let c = backend.insert_node(node("c")).expect("c");
                backend.insert_edge(edge(a, b, "LINKS")).expect("a->b");
                backend.insert_edge(edge(b, c, "LINKS")).expect("b->c");
                (backend, b)
            },
            |(backend, victim)| {
                backend.delete_entity(victim).expect("delete entity");
                black_box(());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("delete_entity/prefer_materialized", |b| {
        b.iter_batched(
            || {
                let backend = CombinedGraphBackend::in_memory_with_config(
                    CombinedConfig::new().with_materialized_reads(),
                )
                .expect("combined materialized");
                let a = backend.insert_node(node("a")).expect("a");
                let b = backend.insert_node(node("b")).expect("b");
                let c = backend.insert_node(node("c")).expect("c");
                backend.insert_edge(edge(a, b, "LINKS")).expect("a->b");
                backend.insert_edge(edge(b, c, "LINKS")).expect("b->c");
                (backend, b)
            },
            |(backend, victim)| {
                backend.delete_entity(victim).expect("delete entity");
                black_box(());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_native_v3_csr_vs_filtered(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_v3_reads");
    configure_group(&mut group);
    group.measurement_time(Duration::from_millis(800));

    let (ctx, root) = build_native_v3_graph();

    group.bench_function(
        BenchmarkId::new("neighbors", "outgoing_unfiltered_csr"),
        |b| {
            b.iter(|| {
                let backend = black_box(&ctx.backend);
                let node = black_box(root);
                let query = black_box(outgoing_query());
                black_box(
                    backend
                        .neighbors(SnapshotId::current(), node, query)
                        .expect("v3 unfiltered neighbors"),
                );
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("neighbors", "outgoing_typed_filtered"),
        |b| {
            b.iter(|| {
                let backend = black_box(&ctx.backend);
                let node = black_box(root);
                let query = black_box(typed_outgoing_query("LINKS"));
                black_box(
                    backend
                        .neighbors(SnapshotId::current(), node, query)
                        .expect("v3 typed neighbors"),
                );
            });
        },
    );

    group.finish();
}

criterion_group!(
    backend_mode_materialization,
    bench_combined_reads,
    bench_combined_cold_reads,
    bench_combined_reopen_reads,
    bench_combined_publish_rebuild,
    bench_combined_mixed_workloads,
    bench_combined_incremental_writes,
    bench_native_v3_csr_vs_filtered
);
criterion_main!(backend_mode_materialization);
