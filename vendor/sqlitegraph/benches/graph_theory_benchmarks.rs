//! Performance benchmarks for Core Graph Theory algorithms (Phase 45).
//!
//! Benchmarks cover:
//! - Weakly Connected Components (WCC)
//! - Strongly Connected Components (SCC) - Tarjan's algorithm
//! - Transitive Closure
//! - Transitive Reduction
//! - Topological Sort
//!
//! Graph sizes:
//! - Small: 10 nodes, ~20 edges
//! - Medium: 100 nodes, ~200 edges
//! - Large: 1000 nodes, ~2000 edges
//!
//! # Graph Fixtures
//!
//! - **Linear chain**: 0->1->2->...->n (DAG, no cycles)
//! - **Diamond DAG**: Multiple paths to sink (DAG, no cycles)
//! - **Random DAG**: Generated with controlled edge probability (DAG, no cycles)
//! - **Graph with cycles**: For SCC and topological sort cycle detection

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use rand::Rng;
use rand::SeedableRng;
use sqlitegraph::{SqliteGraph, algo::*};

const SAMPLE_SIZE: usize = 50;
const WARM_UP_TIME: Duration = Duration::from_secs(2);
const MEASURE_TIME: Duration = Duration::from_secs(5);

// ============================================================================
// Graph Generators
// ============================================================================

/// Create a linear chain DAG: 0 -> 1 -> 2 -> ... -> n-1
fn create_linear_chain(n: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let mut node_ids = Vec::new();
    for i in 0..n {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create chain edges (only forward to maintain DAG property)
    for i in 0..n.saturating_sub(1) {
        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: node_ids[i],
                to_id: node_ids[i + 1],
                edge_type: "NEXT".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");
    }

    graph
}

/// Create a diamond DAG: 0 -> 1 -> 3, 0 -> 2 -> 3
fn create_diamond_dag() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let mut node_ids = Vec::new();
    for i in 0..4 {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create diamond: 0 -> 1 -> 3, 0 -> 2 -> 3
    let edges = vec![(0, 1), (1, 3), (0, 2), (2, 3)];
    for (from_idx, to_idx) in edges {
        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: node_ids[from_idx],
                to_id: node_ids[to_idx],
                edge_type: "EDGE".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");
    }

    graph
}

/// Create a random DAG (no cycles)
fn create_random_dag(n: usize, edge_probability: f64) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let mut node_ids = Vec::new();
    for i in 0..n {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create random edges, but only from lower to higher indices (guarantees DAG)
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
    for i in 0..n {
        for j in (i + 1)..n {
            if rng.gen_range(0.0..1.0) < edge_probability {
                graph
                    .insert_edge(&sqlitegraph::GraphEdge {
                        id: 0,
                        from_id: node_ids[i],
                        to_id: node_ids[j],
                        edge_type: "EDGE".into(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }
        }
    }

    graph
}

/// Create a graph with a cycle: 0 -> 1 -> 2 -> 0
fn create_cycle_graph(n: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let mut node_ids = Vec::new();
    for i in 0..n {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create cycle edges
    for i in 0..n {
        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: node_ids[i],
                to_id: node_ids[(i + 1) % n],
                edge_type: "CYCLE".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");
    }

    graph
}

/// Create a bidirectional random graph (undirected connectivity)
fn create_bidirectional_random(n: usize, edge_probability: f64) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let mut node_ids = Vec::new();
    for i in 0..n {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create bidirectional random edges
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
    for i in 0..n {
        for j in (i + 1)..n {
            if rng.gen_range(0.0..1.0) < edge_probability {
                // Add edge in both directions
                graph
                    .insert_edge(&sqlitegraph::GraphEdge {
                        id: 0,
                        from_id: node_ids[i],
                        to_id: node_ids[j],
                        edge_type: "EDGE".into(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");

                graph
                    .insert_edge(&sqlitegraph::GraphEdge {
                        id: 0,
                        from_id: node_ids[j],
                        to_id: node_ids[i],
                        edge_type: "EDGE".into(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }
        }
    }

    graph
}

// ============================================================================
// Weakly Connected Components (WCC) Benchmarks
// ============================================================================

fn bench_wcc(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wcc");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("linear", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_linear_chain(size);
                let _components =
                    black_box(weakly_connected_components(&graph).expect("WCC failed"));
            });
        });

        group.bench_with_input(
            BenchmarkId::new("bidirectional_random", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let graph = create_bidirectional_random(size, 0.1);
                    let _components =
                        black_box(weakly_connected_components(&graph).expect("WCC failed"));
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Strongly Connected Components (SCC) Benchmarks
// ============================================================================

fn bench_scc(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("scc");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("linear", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_linear_chain(size);
                let _scc = black_box(strongly_connected_components(&graph).expect("SCC failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("diamond", size), &size, |b, &_size| {
            b.iter(|| {
                let graph = create_diamond_dag();
                let _scc = black_box(strongly_connected_components(&graph).expect("SCC failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("random_dag", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_dag(size, 0.1);
                let _scc = black_box(strongly_connected_components(&graph).expect("SCC failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("cycle", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_cycle_graph(size);
                let _scc = black_box(strongly_connected_components(&graph).expect("SCC failed"));
            });
        });
    }

    group.finish();
}

// ============================================================================
// Transitive Closure Benchmarks
// ============================================================================

fn bench_transitive_closure(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("transitive_closure");
    group.sample_size(20); // Smaller sample for O(V²) operation
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::new("linear", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_linear_chain(size);
                let _closure =
                    black_box(transitive_closure(&graph, None).expect("Transitive closure failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("random_dag", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_dag(size, 0.1);
                let _closure =
                    black_box(transitive_closure(&graph, None).expect("Transitive closure failed"));
            });
        });

        // Bounded computation (depth 2)
        group.bench_with_input(
            BenchmarkId::new("random_dag_depth_2", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let graph = create_random_dag(size, 0.1);
                    let bounds = TransitiveClosureBounds {
                        max_depth: Some(2),
                        max_sources: None,
                        max_pairs: None,
                    };
                    let _closure = black_box(
                        transitive_closure(&graph, Some(bounds))
                            .expect("Transitive closure failed"),
                    );
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Transitive Reduction Benchmarks
// ============================================================================

fn bench_transitive_reduction(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("transitive_reduction");
    group.sample_size(30);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::new("linear", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_linear_chain(size);
                let _reduced =
                    black_box(transitive_reduction(&graph).expect("Transitive reduction failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("random_dag", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_dag(size, 0.1);
                let _reduced =
                    black_box(transitive_reduction(&graph).expect("Transitive reduction failed"));
            });
        });
    }

    group.finish();
}

// ============================================================================
// Topological Sort Benchmarks
// ============================================================================

fn bench_topological_sort(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("topological_sort");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("linear", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_linear_chain(size);
                let _ordering =
                    black_box(topological_sort(&graph).expect("Topological sort failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("diamond", size), &size, |b, &_size| {
            b.iter(|| {
                let graph = create_diamond_dag();
                let _ordering =
                    black_box(topological_sort(&graph).expect("Topological sort failed"));
            });
        });

        group.bench_with_input(BenchmarkId::new("random_dag", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_dag(size, 0.1);
                let _ordering =
                    black_box(topological_sort(&graph).expect("Topological sort failed"));
            });
        });

        // Cycle detection benchmark
        group.bench_with_input(
            BenchmarkId::new("cycle_detection", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let graph = create_cycle_graph(size);
                    let _result = black_box(topological_sort(&graph));
                    // Should return CycleDetected error
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark Groups Registration
// ============================================================================

criterion_group!(wcc_benches, bench_wcc,);

criterion_group!(scc_benches, bench_scc,);

criterion_group!(transitive_closure_benches, bench_transitive_closure,);

criterion_group!(transitive_reduction_benches, bench_transitive_reduction,);

criterion_group!(topo_sort_benches, bench_topological_sort,);

criterion_main!(
    wcc_benches,
    scc_benches,
    transitive_closure_benches,
    transitive_reduction_benches,
    topo_sort_benches,
);
