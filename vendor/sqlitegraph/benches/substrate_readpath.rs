//! Phase 0.0 — Substrate read-path profile (sqlite-backend, the path consumers run on).
//!
//! PURPOSE: settle whether Thread A (tiny-pointer hash-probe optimization) has a
//! home on the real consumer read path. The consumer path is:
//!
//!   SqliteGraphBackend::neighbors(SnapshotId::current(), node, NeighborQuery)
//!     -> query_neighbors -> graph.fetch_outgoing -> AdjacencyCache [hit: clone | miss: SQL]
//!
//! On a cache HIT the cost is: parking_lot::RwLock read + ahash probe + Vec<i64>
//! CLONE (O(degree)). The hash probe is the ONLY term a tiny-pointer table would
//! shrink. This bench decomposes that hit cost (probe vs clone vs lock) and
//! contrasts hit vs SQL-miss, across a degree axis, so the numbers decide.
//!
//! Scope: bench-only, Phase 0. Not referenced from src/. Committing nothing.

use std::hint::black_box;

use ahash::AHashMap;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use parking_lot::RwLock;
use serde_json::json;
use sqlitegraph::{
    BackendDirection, GraphBackend, GraphEdge, GraphEntity, NeighborQuery, SnapshotId,
    SqliteGraphBackend, cache::AdjacencyCache,
};

const PROBE_BATCH: usize = 1024; // keys touched per timed iter for ns-scale ops

// ----------------------------------------------------------------------------
// Graph fixtures
// ----------------------------------------------------------------------------

fn make_entity(kind: &str, name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({}),
    }
}

fn make_edge(from: i64, to: i64, edge_type: &str) -> GraphEdge {
    GraphEdge {
        id: 0,
        from_id: from,
        to_id: to,
        edge_type: edge_type.to_string(),
        data: json!({}),
    }
}

/// Build a synthetic code-graph-shaped graph: a few high-degree hubs plus many
/// low-degree leaves. Mirrors real symbol graphs (a few `new`/`path`/`emit`
/// hubs with dozens of edges; most symbols have 1-2).
///
/// Returns (backend, hub_ids, leaf_ids).
fn build_hub_leaf_graph(
    n_hubs: usize,
    hub_degree: usize,
    n_leaves: usize,
) -> (SqliteGraphBackend, Vec<i64>, Vec<i64>) {
    let backend = SqliteGraphBackend::in_memory().expect("open in-memory sqlite backend");
    let g = backend.graph();

    let mut hubs = Vec::with_capacity(n_hubs);
    for i in 0..n_hubs {
        hubs.push(
            g.insert_entity(&make_entity("hub", &format!("hub_{i}")))
                .expect("insert hub"),
        );
    }
    let mut leaves = Vec::with_capacity(n_leaves);
    for i in 0..n_leaves {
        leaves.push(
            g.insert_entity(&make_entity("leaf", &format!("leaf_{i}")))
                .expect("insert leaf"),
        );
    }

    // hub -> many leaves (high out-degree on hubs)
    for (hi, &h) in hubs.iter().enumerate() {
        for d in 0..hub_degree {
            let li = (hi * hub_degree + d) % leaves.len();
            g.insert_edge(&make_edge(h, leaves[li], "calls"))
                .expect("insert hub->leaf");
        }
    }
    // leaf -> one hub (low out-degree on leaves)
    for (i, &l) in leaves.iter().enumerate() {
        let h = hubs[i % hubs.len()];
        g.insert_edge(&make_edge(l, h, "calls"))
            .expect("insert leaf->hub");
    }

    (backend, hubs, leaves)
}

// ----------------------------------------------------------------------------
// Group 1 — hit-path decomposition (the Thread-A question)
//   probe_only       : AHashMap::get(&k)               -> the term tiny pointers shrink
//   probe_and_clone  : AHashMap::get(&k).cloned()      -> probe + Vec clone
//   rwlock_probe_clone: RwLock<AHashMap>::read().get().cloned() -> what AdjacencyCache::get does
//   adjcache_get     : AdjacencyCache::get(k)          -> the real structure, real path
// Degree axis = Vec length {10, 100, 1000}: if clone grows with degree and probe
// stays flat, the probe is not the bottleneck.
// ----------------------------------------------------------------------------

fn fill_probe_map(degree: usize) -> (AHashMap<i64, Vec<i64>>, Vec<i64>) {
    let mut m = AHashMap::with_capacity(PROBE_BATCH + 8);
    let mut keys = Vec::with_capacity(PROBE_BATCH);
    let v: Vec<i64> = (0..degree as i64).collect();
    for i in 0..PROBE_BATCH as i64 {
        m.insert(i, v.clone());
        keys.push(i);
    }
    (m, keys)
}

fn bench_hit_decomposition(c: &mut Criterion) {
    let mut group = c.benchmark_group("readpath_decompose");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(2));
    group.throughput(Throughput::Elements(PROBE_BATCH as u64));

    for degree in [10usize, 100, 1000] {
        let (map, keys) = fill_probe_map(degree);
        let map_rwlock = RwLock::new(map.clone());
        let adjcache = AdjacencyCache::new();
        for &k in &keys {
            adjcache.insert(k, map[&k].clone());
        }

        group.bench_with_input(BenchmarkId::new("probe_only", degree), &degree, |b, _| {
            b.iter(|| {
                let mut acc = 0i64;
                for &k in &keys {
                    if let Some(v) = map.get(&k) {
                        acc = acc.wrapping_add(v.len() as i64);
                    }
                }
                black_box(acc);
            });
        });

        group.bench_with_input(
            BenchmarkId::new("probe_and_clone", degree),
            &degree,
            |b, _| {
                b.iter(|| {
                    let mut acc = 0i64;
                    for &k in &keys {
                        if let Some(v) = map.get(&k).cloned() {
                            acc = acc.wrapping_add(v.len() as i64);
                        }
                    }
                    black_box(acc);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("rwlock_probe_clone", degree),
            &degree,
            |b, _| {
                b.iter(|| {
                    let mut acc = 0i64;
                    for &k in &keys {
                        if let Some(v) = map_rwlock.read().get(&k).cloned() {
                            acc = acc.wrapping_add(v.len() as i64);
                        }
                    }
                    black_box(acc);
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("adjcache_get", degree), &degree, |b, _| {
            b.iter(|| {
                let mut acc = 0i64;
                for &k in &keys {
                    if let Some(v) = adjcache.get(k) {
                        acc = acc.wrapping_add(v.len() as i64);
                    }
                }
                black_box(acc);
            });
        });
    }

    group.finish();
}

// ----------------------------------------------------------------------------
// Group 2 — the REAL consumer path: SqliteGraphBackend::neighbors
//   neighbors_hit_hub  : warm cache, high-degree node (expensive clone)
//   neighbors_hit_leaf : warm cache, low-degree node (cheap clone)
//   neighbors_miss_hub : cache entry removed first -> forced SQL SELECT + insert
// ----------------------------------------------------------------------------

fn warm_then_neighbors(backend: &SqliteGraphBackend, node: i64) {
    let q = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    // first call warms the cache (miss); discard result
    let _ = backend
        .neighbors(SnapshotId::current(), node, q.clone())
        .expect("warm neighbors");
}

fn bench_consumer_neighbors(c: &mut Criterion) {
    let mut group = c.benchmark_group("consumer_neighbors");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(2));

    // 4 hubs each with 100 out-edges, 800 leaves with 1 out-edge each.
    let (backend, hubs, leaves) = build_hub_leaf_graph(4, 100, 800);
    let q = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let hub = hubs[0];
    let leaf = leaves[0];

    // Warm caches for the hit benches.
    warm_then_neighbors(&backend, hub);
    warm_then_neighbors(&backend, leaf);

    group.bench_function("neighbors_hit_hub_deg100", |b| {
        b.iter(|| {
            let v = backend
                .neighbors(SnapshotId::current(), black_box(hub), q.clone())
                .expect("neighbors hit hub");
            black_box(v.len());
        });
    });

    group.bench_function("neighbors_hit_leaf_deg1", |b| {
        b.iter(|| {
            let v = backend
                .neighbors(SnapshotId::current(), black_box(leaf), q.clone())
                .expect("neighbors hit leaf");
            black_box(v.len());
        });
    });

    group.bench_function("neighbors_miss_hub_deg100", |b| {
        b.iter(|| {
            // Force a miss: drop the cached entry, then query -> SQL SELECT + insert.
            backend.graph().outgoing_cache_ref().remove(black_box(hub));
            let v = backend
                .neighbors(SnapshotId::current(), black_box(hub), q.clone())
                .expect("neighbors miss hub");
            black_box(v.len());
        });
    });

    group.finish();
}

// ----------------------------------------------------------------------------
// Group 3 — integrated traversal via the consumer path (manual BFS using
// neighbors). Exercises AdjacencyCache hits on a realistic reach set. No
// QueryCache involvement (that cache only stores whole bfs()/shortest_path()).
// ----------------------------------------------------------------------------

fn bfs_via_neighbors(backend: &SqliteGraphBackend, start: i64, max_depth: u32) -> usize {
    use std::collections::{HashMap, VecDeque};
    let q = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let mut visited: HashMap<i64, ()> = HashMap::new();
    let mut frontier: VecDeque<(i64, u32)> = VecDeque::new();
    frontier.push_back((start, 0));
    visited.insert(start, ());
    let mut touched = 0usize;
    while let Some((node, depth)) = frontier.pop_front() {
        touched += 1;
        if depth >= max_depth {
            continue;
        }
        if let Ok(nbrs) = backend.neighbors(SnapshotId::current(), node, q.clone()) {
            for n in nbrs {
                if visited.insert(n, ()).is_none() {
                    frontier.push_back((n, depth + 1));
                }
            }
        }
    }
    touched
}

fn bench_consumer_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("consumer_traversal");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(2));
    group.sample_size(30);

    let (backend, hubs, _leaves) = build_hub_leaf_graph(4, 100, 800);
    let start = hubs[0];

    for depth in [1u32, 2] {
        group.bench_with_input(
            BenchmarkId::new("bfs_neighbors", depth),
            &depth,
            |b, &depth| {
                b.iter(|| {
                    let n = bfs_via_neighbors(&backend, black_box(start), black_box(depth));
                    black_box(n);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_hit_decomposition,
    bench_consumer_neighbors,
    bench_consumer_traversal,
);
criterion_main!(benches);
