//! V3 Algorithm Benchmarks
//!
//! Run with: cargo bench --features v3-bench -- v3_algorithm_benchmarks
//!
//! Benchmarks graph algorithms running against the V3 backend:
//! - BFS/DFS traversal
//! - Shortest path
//! - Topological sort
//! - PageRank
//! - Connected components
//! - k-hop queries

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tempfile::TempDir;

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;

// ============================================================================
// Graph Generators
// ============================================================================

/// Create a linear chain graph: n0 -> n1 -> n2 -> ... -> n(N-1)
fn create_chain(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("chain_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap(),
        );
    }
    for i in 0..n - 1 {
        backend
            .insert_edge(EdgeSpec {
                from: ids[i],
                to: ids[i + 1],
                edge_type: "NEXT".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    ids
}

/// Create a star graph: center -> leaf_0, leaf_1, ..., leaf_(N-1)
fn create_star(backend: &V3Backend, n: usize) -> (i64, Vec<i64>) {
    let center = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut leaves = Vec::with_capacity(n);
    for i in 0..n {
        let leaf = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("leaf_{}", i),
                file_path: None,
                data: serde_json::json!({"idx": i}),
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
    (center, leaves)
}

/// Create a binary tree: root at index 0, children at 2i+1 and 2i+2
fn create_binary_tree(backend: &V3Backend, depth: usize) -> Vec<i64> {
    let n = (1 << (depth + 1)) - 1; // 2^(depth+1) - 1 nodes
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("bt_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap(),
        );
    }
    for i in 0..n {
        let left = 2 * i + 1;
        let right = 2 * i + 2;
        if left < n {
            backend
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[left],
                    edge_type: "LEFT".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
        if right < n {
            backend
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[right],
                    edge_type: "RIGHT".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }
    ids
}

// ============================================================================
// A. BFS / TRAVERSAL BENCHMARKS
// ============================================================================

fn bench_bfs_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3_algo/bfs_traversal");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);

    // BFS via k_hop with large depth
    for (name, nodes) in [("chain_100", 100), ("chain_500", 500)] {
        let temp = TempDir::new().unwrap();
        let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
        let ids = create_chain(&backend, nodes);
        let snapshot = SnapshotId::current();

        group.throughput(Throughput::Elements(nodes as u64));
        group.bench_with_input(BenchmarkId::new("k_hop", name), &nodes, |b, _| {
            b.iter(|| {
                // BFS traversal via k_hop from start
                black_box(
                    backend
                        .k_hop(snapshot, ids[0], 100, BackendDirection::Outgoing)
                        .unwrap(),
                );
            });
        });
    }
    group.finish();
}

// ============================================================================
// B. K-HOP BENCHMARKS
// ============================================================================

fn bench_k_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3_algo/k_hop");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Binary tree of depth 4: 31 nodes
    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
    let ids = create_binary_tree(&backend, 4);
    let snapshot = SnapshotId::current();
    let root = ids[0];

    for depth in [1, 2, 3, 4] {
        group.bench_with_input(
            BenchmarkId::new("binary_tree", depth),
            &depth,
            |b, &depth| {
                b.iter(|| {
                    black_box(
                        backend
                            .k_hop(snapshot, root, depth as u32, BackendDirection::Outgoing)
                            .unwrap(),
                    );
                });
            },
        );
    }
    group.finish();
}

// ============================================================================
// C. NEIGHBOR QUERY BENCHMARKS
// ============================================================================

fn bench_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3_algo/neighbors");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Star graph: center with 100 leaves
    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
    let (center, leaves) = create_star(&backend, 100);
    let snapshot = SnapshotId::current();

    group.bench_function("star_outgoing_center", |b| {
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

    group.bench_function("star_incoming_leaf", |b| {
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

    group.bench_function("star_filtered_type", |b| {
        b.iter(|| {
            black_box(
                backend
                    .neighbors(
                        snapshot,
                        center,
                        NeighborQuery {
                            direction: BackendDirection::Outgoing,
                            edge_type: Some("CONNECTS".to_string()),
                        },
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

// ============================================================================
// D. NODE OPERATIONS BENCHMARKS
// ============================================================================

fn bench_node_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3_algo/node_ops");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Pre-populate
    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();
    let mut ids = Vec::new();
    for i in 0..100 {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("n_{}", i),
                    file_path: None,
                    data: serde_json::json!({"val": i}),
                })
                .unwrap(),
        );
    }
    let snapshot = SnapshotId::current();

    group.bench_function("get_node", |b| {
        b.iter(|| {
            for &id in &ids {
                black_box(backend.get_node(snapshot, id).unwrap());
            }
        });
    });

    group.bench_function("entity_ids", |b| {
        b.iter(|| {
            black_box(backend.entity_ids().unwrap());
        });
    });

    group.bench_function("node_degree", |b| {
        b.iter(|| {
            for &id in &ids {
                black_box(backend.node_degree(snapshot, id).unwrap());
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_bfs_traversal,
    bench_k_hop,
    bench_neighbors,
    bench_node_operations,
);
criterion_main!(benches);
