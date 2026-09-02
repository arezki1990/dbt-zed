//! MVCC Performance Benchmarks
//!
//! **Purpose:** Measure MVCC snapshot system performance characteristics
//! **Scope:** Snapshot acquisition, cloning, iteration, concurrent access
//! **Focus:** Latency, throughput, memory overhead, isolation overhead
//!
//! These benchmarks establish performance baselines for MVCC-lite snapshot
//! operations and track overhead compared to direct graph access.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use sqlitegraph::{GraphEdgeCreate, GraphEntityCreate, SqliteGraph};
use std::sync::Arc;
use std::time::Duration;

//
// BENCHMARK HELPERS
//

/// Helper: Insert entity using proper API
fn insert_entity(
    graph: &SqliteGraph,
    create: GraphEntityCreate,
) -> Result<i64, sqlitegraph::SqliteGraphError> {
    let entity = sqlitegraph::GraphEntity {
        id: 0,
        kind: create.kind,
        name: create.name,
        file_path: create.file_path,
        data: create.data,
    };
    graph.insert_entity(&entity)
}

/// Helper: Insert edge using proper API
fn insert_edge(
    graph: &SqliteGraph,
    create: GraphEdgeCreate,
) -> Result<i64, sqlitegraph::SqliteGraphError> {
    let edge = sqlitegraph::GraphEdge {
        id: 0,
        from_id: create.from_id,
        to_id: create.to_id,
        edge_type: create.edge_type,
        data: create.data,
    };
    graph.insert_edge(&edge)
}

/// Helper: Warm the cache by reading all adjacency data
fn warm_cache(graph: &SqliteGraph) -> Result<(), sqlitegraph::SqliteGraphError> {
    let entity_ids = graph.list_entity_ids()?;
    for &id in &entity_ids {
        let _ = graph.query().outgoing(id);
        let _ = graph.query().incoming(id);
    }
    Ok(())
}

/// Helper: Create benchmark graph with specified size
fn create_benchmark_graph(size: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nodes
    for i in 0..size {
        let entity = GraphEntityCreate {
            kind: "bench".to_string(),
            name: format!("bench_node_{}", i),
            file_path: Some(format!("bench_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        insert_entity(&graph, entity).expect("Failed to insert entity");
    }

    // Create edges (each node connects to next 2 nodes)
    let ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    for (i, &id) in ids.iter().enumerate() {
        for j in 1..=2 {
            let target_idx = (i + j) % ids.len();
            let edge = GraphEdgeCreate {
                from_id: id,
                to_id: ids[target_idx],
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            };
            insert_edge(&graph, edge).expect("Failed to insert edge");
        }
    }

    graph
}

//
// BENCHMARK 1: SNAPSHOT ACQUISITION LATENCY
//

fn bench_snapshot_acquisition(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_acquisition");

    for size in [100, 1_000, 5_000, 10_000].iter() {
        let graph = create_benchmark_graph(*size);
        warm_cache(&graph).expect("Failed to warm cache");

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let snapshot = black_box(graph.acquire_snapshot().unwrap());
                black_box(snapshot.node_count())
            })
        });
    }

    group.finish();
}

//
// BENCHMARK 2: SNAPSHOT CLONE PERFORMANCE
//

fn bench_snapshot_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_clone");

    let graph = create_benchmark_graph(1_000);
    warm_cache(&graph).expect("Failed to warm cache");

    let snapshot = std::rc::Rc::new(graph.acquire_snapshot().unwrap());

    group.bench_function("arc_clone_1000", |b| {
        b.iter(|| {
            let _clone = black_box(std::rc::Rc::clone(&snapshot));
        })
    });

    group.finish();
}

//
// BENCHMARK 3: SNAPSHOT ITERATION PERFORMANCE
//

fn bench_snapshot_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_iteration");

    for size in [100, 1_000, 10_000].iter() {
        let graph = create_benchmark_graph(*size);
        warm_cache(&graph).expect("Failed to warm cache");

        let snapshot = graph.acquire_snapshot().unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let count = black_box(snapshot.node_count());
                black_box(count)
            })
        });
    }

    group.finish();
}

//
// BENCHMARK 4: SNAPSHOT UPDATE PERFORMANCE
//

fn bench_snapshot_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_update");

    for size in [100, 1_000, 5_000, 10_000].iter() {
        let graph = create_benchmark_graph(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                // Trigger snapshot update via acquire_snapshot
                // which calls update_snapshot internally
                let _snapshot = black_box(graph.acquire_snapshot().unwrap());
            })
        });
    }

    group.finish();
}

//
// BENCHMARK 5: CONCURRENT SNAPSHOT ACQUISITION
//
// NOTE: SqliteGraph is NOT thread-safe (contains RefCell, non-Sync types).
// This benchmark uses SnapshotManager directly which IS thread-safe.
//

fn bench_concurrent_snapshot_acquisition(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_snapshots");

    // Use SnapshotManager directly for concurrent testing
    use sqlitegraph::mvcc::SnapshotManager;
    use std::collections::HashMap;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicU64, Ordering};

    for thread_count in [1, 2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &num_threads| {
                b.iter(|| {
                    // Create test snapshot manager
                    let mut outgoing = HashMap::new();
                    let mut incoming = HashMap::new();
                    for i in 0..1_000 {
                        outgoing.insert(i, vec![]);
                        incoming.insert(i, vec![]);
                    }
                    let manager = Arc::new(SnapshotManager::with_state(&outgoing, &incoming));
                    let barrier = Arc::new(Barrier::new(num_threads));
                    let counter = Arc::new(AtomicU64::new(0));

                    let handles: Vec<_> = (0..num_threads)
                        .map(|_| {
                            let manager = manager.clone();
                            let barrier = barrier.clone();
                            let counter = counter.clone();

                            std::thread::spawn(move || {
                                barrier.wait();
                                for _ in 0..100 {
                                    let snapshot = manager.acquire_snapshot();
                                    if snapshot.node_count() > 0 {
                                        counter.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            })
                        })
                        .collect();

                    for h in handles {
                        h.join().unwrap();
                    }

                    black_box(counter.load(Ordering::Relaxed))
                })
            },
        );
    }

    group.finish();
}

//
// BENCHMARK 6: MEMORY OVERHEAD PER SNAPSHOT
//

fn bench_memory_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_overhead");

    group.throughput(Throughput::Elements(1));

    for size in [100, 1_000, 10_000].iter() {
        let graph = create_benchmark_graph(*size);
        warm_cache(&graph).expect("Failed to warm cache");

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let snapshot = black_box(graph.acquire_snapshot().unwrap());
                let nodes = black_box(snapshot.node_count());
                let edges = black_box(snapshot.edge_count());
                (nodes, edges)
            })
        });
    }

    group.finish();
}

//
// BENCHMARK 7: RAPID SNAPSHOT CREATION/DESTRUCTION
//

fn bench_rapid_snapshot_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_lifecycle");

    let graph = create_benchmark_graph(100);
    warm_cache(&graph).expect("Failed to warm cache");

    group.bench_function("create_drop_1000", |b| {
        b.iter(|| {
            for _ in 0..100 {
                let snapshot = black_box(graph.acquire_snapshot().unwrap());
                black_box(snapshot.node_count());
                // Snapshot dropped here
            }
        })
    });

    group.finish();
}

//
// BENCHMARK 8: SNAPSHOT VS DIRECT ACCESS OVERHEAD
//

fn bench_snapshot_vs_direct_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_vs_direct");

    let graph = create_benchmark_graph(1_000);
    warm_cache(&graph).expect("Failed to warm cache");

    let snapshot = graph.acquire_snapshot().unwrap();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Direct access benchmark
    group.bench_function("direct_access", |b| {
        b.iter(|| {
            for &id in entity_ids.iter().take(100) {
                let _neighbors = black_box(graph.query().outgoing(id).unwrap());
            }
        })
    });

    // Snapshot access benchmark
    group.bench_function("snapshot_access", |b| {
        b.iter(|| {
            for &id in entity_ids.iter().take(100) {
                let _neighbors = black_box(snapshot.get_outgoing(id));
            }
        })
    });

    group.finish();
}

//
// BENCHMARK 9: THROUGHPUT UNDER SUSTAINED LOAD
//

fn bench_snapshot_throughput(c: &mut Criterion) {
    let graph = create_benchmark_graph(1_000);
    warm_cache(&graph).expect("Failed to warm cache");

    let mut group = c.benchmark_group("throughput");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("sustained_acquisition", |b| {
        b.iter(|| {
            let snapshot = black_box(graph.acquire_snapshot().unwrap());
            black_box(snapshot.node_count());
        })
    });

    group.finish();
}

//
// CRITERION SETUP
//

criterion_group! {
    name = mvcc_benchmarks;
    config = Criterion::default()
        .sample_size(100)
        .measurement_time(Duration::from_secs(3))
        .warm_up_time(Duration::from_millis(500));
    targets =
        bench_snapshot_acquisition,
        bench_snapshot_clone,
        bench_snapshot_iteration,
        bench_snapshot_update,
        bench_concurrent_snapshot_acquisition,
        bench_memory_overhead,
        bench_rapid_snapshot_lifecycle,
        bench_snapshot_vs_direct_access,
        bench_snapshot_throughput
}

criterion_main!(mvcc_benchmarks);
