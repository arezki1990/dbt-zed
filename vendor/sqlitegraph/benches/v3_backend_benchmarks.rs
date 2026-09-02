#![allow(
    clippy::needless_range_loop,
    clippy::redundant_closure_call,
    reason = "test/bench code"
)]
//! V3 Backend Primitive Benchmarks
#![allow(
    clippy::needless_range_loop,
    reason = "bench code uses indexed loops for clarity"
)]

//!
//! Run with: cargo bench --features v3-bench -- v3_backend_benchmarks
//!
//! Benchmarks V3 backend primitives:
//! - PageAllocator: allocate/deallocate cycles, free list reuse
//! - BTreeManager: insert/lookup at various sizes
//! - V3EdgeStore: insert_edge, neighbors, flush
//! - V3Backend: insert_node, insert_edge, get_node, neighbors

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tempfile::TempDir;

use parking_lot::RwLock;
use sqlitegraph::backend::native::v3::btree::BTreeManager;
use sqlitegraph::backend::native::v3::{PageAllocator, PersistentHeaderV3, V3Backend};
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use std::sync::Arc;

mod bench_utils;
use bench_utils::create_v3_bench_context;

// ============================================================================
// A. PAGE ALLOCATOR BENCHMARKS
// ============================================================================

fn bench_allocator_allocate(c: &mut Criterion) {
    let mut group = c.benchmark_group("allocator/allocate");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    for size in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            b.iter(|| {
                let header = PersistentHeaderV3::new_v3();
                let mut allocator = PageAllocator::new(&header);
                for _ in 0..size {
                    black_box(allocator.allocate().unwrap());
                }
            });
        });
    }
    group.finish();
}

fn bench_allocator_allocate_deallocate_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("allocator/reuse");
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("allocate_1000_dealloc_500_realloc_500", |b| {
        b.iter(|| {
            let header = PersistentHeaderV3::new_v3();
            let mut allocator = PageAllocator::new(&header);

            // Allocate 1000 pages
            let mut pages: Vec<u64> = Vec::with_capacity(1000);
            for _ in 0..1000 {
                pages.push(allocator.allocate().unwrap());
            }

            // Free first 500
            for i in 0..500 {
                allocator.deallocate(pages[i]).unwrap();
            }

            // Re-allocate 500 (should reuse from free list)
            for _ in 0..500 {
                let reused = allocator.allocate().unwrap();
                black_box(reused);
            }
        });
    });
    group.finish();
}

// ============================================================================
// B. BTREE MANAGER BENCHMARKS
// ============================================================================

fn bench_btree_insert_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("btree/insert_lookup");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("bench.db");
    std::fs::File::create(&db_path).unwrap();

    for size in [100, 1_000, 10_000] {
        // Insert benchmark
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("insert", size), &size, |b, &size| {
            b.iter(|| {
                let header = PersistentHeaderV3::new_v3();
                let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
                let mut btree = BTreeManager::new(allocator, None, db_path.clone());
                for i in 0..size {
                    black_box(btree.insert(i as i64, (i + 1) as u64).unwrap());
                }
            });
        });

        // Lookup benchmark (pre-populate then measure lookups)
        let header = PersistentHeaderV3::new_v3();
        let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
        let mut btree = BTreeManager::new(allocator, None, db_path.clone());
        for i in 0..size {
            btree.insert(i as i64, (i + 1) as u64).unwrap();
        }

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("lookup", size), &size, |b, &size| {
            b.iter(|| {
                for i in 0..size {
                    black_box(btree.lookup(i as i64).unwrap());
                }
            });
        });
    }
    group.finish();
}

// ============================================================================
// C. V3 BACKEND END-TO-END BENCHMARKS
// ============================================================================

fn bench_v3_insert_nodes(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3/insert_nodes");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);

    for size in [100, 1_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("nodes", size), &size, |b, &size| {
            b.iter_batched(
                || create_v3_bench_context("v3.db"),
                |ctx| {
                    for i in 0..size {
                        black_box(
                            ctx.backend
                                .insert_node(NodeSpec {
                                    kind: "Node".to_string(),
                                    name: format!("node_{}", i),
                                    file_path: None,
                                    data: serde_json::json!({"id": i}),
                                })
                                .unwrap(),
                        );
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_v3_insert_edges(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3/insert_edges");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);

    for (name, nodes) in [("small", 50), ("medium", 200)] {
        group.throughput(Throughput::Elements(nodes as u64));
        group.bench_with_input(BenchmarkId::new("edges", name), &nodes, |b, &nodes| {
            b.iter_batched(
                || {
                    let ctx = create_v3_bench_context("v3.db");
                    let mut node_ids = Vec::with_capacity(nodes);
                    for i in 0..nodes {
                        node_ids.push(
                            ctx.backend
                                .insert_node(NodeSpec {
                                    kind: "Node".to_string(),
                                    name: format!("n_{}", i),
                                    file_path: None,
                                    data: serde_json::json!({}),
                                })
                                .unwrap(),
                        );
                    }
                    (ctx, node_ids)
                },
                |(ctx, node_ids)| {
                    // Create chain edges: 0->1->2->...
                    for i in 0..node_ids.len() - 1 {
                        black_box(
                            ctx.backend
                                .insert_edge(EdgeSpec {
                                    from: node_ids[i],
                                    to: node_ids[i + 1],
                                    edge_type: "NEXT".to_string(),
                                    data: serde_json::json!({}),
                                })
                                .unwrap(),
                        );
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_v3_get_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3/get_neighbors");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Pre-build a star graph: center -> 100 leaves
    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
    let center = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut leaves = Vec::new();
    for i in 0..100 {
        let leaf = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("leaf_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: leaf,
                edge_type: "CONNECTS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        leaves.push(leaf);
    }

    let snapshot = sqlitegraph::snapshot::SnapshotId::current();

    group.bench_function("star_100_outgoing", |b| {
        b.iter(|| {
            black_box(
                backend
                    .neighbors(
                        snapshot,
                        center,
                        NeighborQuery {
                            direction: BackendDirection::Outgoing,
                            edge_type: None,
                        },
                    )
                    .unwrap(),
            );
        });
    });

    group.bench_function("leaf_incoming", |b| {
        b.iter(|| {
            black_box(
                backend
                    .neighbors(
                        snapshot,
                        leaves[0],
                        NeighborQuery {
                            direction: BackendDirection::Incoming,
                            edge_type: None,
                        },
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_allocator_allocate,
    bench_allocator_allocate_deallocate_reuse,
    bench_btree_insert_lookup,
    bench_v3_insert_nodes,
    bench_v3_insert_edges,
    bench_v3_get_neighbors,
);
criterion_main!(benches);
