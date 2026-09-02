//! Algorithm performance benchmarks for graph algorithms.
//!
//! Comprehensive benchmark suite for validating algorithm performance
//! from Phase 8 plans 08-01 (centrality) and 08-02 (community detection).
//!
//! Benchmark categories:
//! - Centrality algorithms (PageRank, Betweenness Centrality)
//! - Community detection (Label Propagation, Louvain)
//! - Performance regression detection (baseline comparison)
//! - Edge case handling (empty graphs, disconnected components)

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rand::Rng;
use rand::SeedableRng;
use sqlitegraph::{SqliteGraph, algo::*};

const SAMPLE_SIZE: usize = 100;
const WARM_UP_TIME: Duration = Duration::from_secs(3);
const MEASURE_TIME: Duration = Duration::from_secs(10);

// ============================================================================
// Graph Generators
// ============================================================================

/// Create a random graph with n nodes and edge_probability
fn create_random_graph(n: usize, edge_probability: f64) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nodes
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

    // Create random edges
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5F3759DF);
    for i in 0..n {
        for j in 0..n {
            if i != j && rng.gen_range(0.0..1.0) < edge_probability {
                graph
                    .insert_edge(&sqlitegraph::GraphEdge {
                        id: 0,
                        from_id: node_ids[i],
                        to_id: node_ids[j],
                        edge_type: "LINK".into(),
                        data: serde_json::json!({}),
                    })
                    .expect("Failed to insert edge");
            }
        }
    }

    graph
}

/// Create a star graph: center node connected to all others
fn create_star_graph(n: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create center
    let center = graph
        .insert_entity(&sqlitegraph::GraphEntity {
            id: 0,
            kind: "Node".into(),
            name: "center".into(),
            file_path: None,
            data: serde_json::json!({"center": true}),
        })
        .expect("Failed to insert center");

    // Create leaves and connect to center
    for i in 0..n - 1 {
        let leaf = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("leaf_{}", i),
                file_path: None,
                data: serde_json::json!({"leaf": i}),
            })
            .expect("Failed to insert leaf");

        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: leaf,
                to_id: center,
                edge_type: "LINK".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");
    }

    graph
}

/// Create a cycle graph: ring topology
fn create_cycle_graph(n: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nodes
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

    // Create cycle edges (bidirectional)
    for i in 0..n {
        let next = (i + 1) % n;
        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: node_ids[i],
                to_id: node_ids[next],
                edge_type: "LINK".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");

        graph
            .insert_edge(&sqlitegraph::GraphEdge {
                id: 0,
                from_id: node_ids[next],
                to_id: node_ids[i],
                edge_type: "LINK".into(),
                data: serde_json::json!({}),
            })
            .expect("Failed to insert edge");
    }

    graph
}

/// Create a barbell graph: two cliques connected by a bridge edge
fn create_barbell_graph(clique_size: usize) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create first clique
    let mut clique1: Vec<i64> = Vec::new();
    for i in 0..clique_size {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("c1_{}", i),
                file_path: None,
                data: serde_json::json!({"clique": 1, "node": i}),
            })
            .expect("Failed to insert node");
        clique1.push(id);
    }

    // Connect clique1 internally (bidirectional)
    for i in 0..clique_size {
        for j in (i + 1)..clique_size {
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: clique1[i],
                    to_id: clique1[j],
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("Failed to insert edge");

            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: clique1[j],
                    to_id: clique1[i],
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("Failed to insert edge");
        }
    }

    // Create second clique
    let mut clique2: Vec<i64> = Vec::new();
    for i in 0..clique_size {
        let id = graph
            .insert_entity(&sqlitegraph::GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: format!("c2_{}", i),
                file_path: None,
                data: serde_json::json!({"clique": 2, "node": i}),
            })
            .expect("Failed to insert node");
        clique2.push(id);
    }

    // Connect clique2 internally (bidirectional)
    for i in 0..clique_size {
        for j in (i + 1)..clique_size {
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: clique2[i],
                    to_id: clique2[j],
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("Failed to insert edge");

            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: clique2[j],
                    to_id: clique2[i],
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("Failed to insert edge");
        }
    }

    // Add bridge edge between cliques (bidirectional)
    graph
        .insert_edge(&sqlitegraph::GraphEdge {
            id: 0,
            from_id: clique1[0],
            to_id: clique2[0],
            edge_type: "BRIDGE".into(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert bridge");

    graph
        .insert_edge(&sqlitegraph::GraphEdge {
            id: 0,
            from_id: clique2[0],
            to_id: clique1[0],
            edge_type: "BRIDGE".into(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert bridge");

    graph
}

// ============================================================================
// PageRank Benchmarks
// ============================================================================

fn bench_pagerank(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("pagerank");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Test different graph sizes
    for &size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("random_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_graph(size, 0.1);
                let _scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
            });
        });

        group.bench_with_input(BenchmarkId::new("cycle_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_cycle_graph(size);
                let _scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
            });
        });

        group.bench_with_input(BenchmarkId::new("star_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_star_graph(size);
                let _scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
            });
        });
    }

    group.finish();
}

// ============================================================================
// Betweenness Centrality Benchmarks
// ============================================================================

fn bench_betweenness_centrality(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("betweenness_centrality");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Betweenness is O(VE) - keep sizes smaller
    for &size in [10, 100, 500].iter() {
        group.bench_with_input(BenchmarkId::new("random_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_graph(size, 0.1);
                let _centrality = betweenness_centrality(&graph).expect("betweenness failed");
            });
        });

        group.bench_with_input(BenchmarkId::new("cycle_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_cycle_graph(size);
                let _centrality = betweenness_centrality(&graph).expect("betweenness failed");
            });
        });

        group.bench_with_input(BenchmarkId::new("star_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_star_graph(size);
                let _centrality = betweenness_centrality(&graph).expect("betweenness failed");
            });
        });
    }

    group.finish();
}

// ============================================================================
// Label Propagation Benchmarks
// ============================================================================

fn bench_label_propagation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("label_propagation");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Test different graph sizes
    for &size in [10, 100, 1000, 5000].iter() {
        group.bench_with_input(BenchmarkId::new("random_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_graph(size, 0.1);
                let _communities = label_propagation(&graph, 10).expect("label propagation failed");
            });
        });

        group.bench_with_input(BenchmarkId::new("cycle_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_cycle_graph(size);
                let _communities = label_propagation(&graph, 10).expect("label propagation failed");
            });
        });
    }

    group.finish();
}

// ============================================================================
// Louvain Method Benchmarks
// ============================================================================

fn bench_louvain(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("louvain");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Test different graph sizes
    for &size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("random_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_graph(size, 0.1);
                let _communities = louvain_communities(&graph, 10).expect("louvain failed");
            });
        });

        group.bench_with_input(
            BenchmarkId::new("barbell_graph", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let clique_size = size / 2;
                    let graph = create_barbell_graph(clique_size);
                    let _communities = louvain_communities(&graph, 10).expect("louvain failed");
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Connected Components Benchmarks (baseline)
// ============================================================================

fn bench_connected_components(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("connected_components");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for &size in [10, 100, 1000, 5000].iter() {
        group.bench_with_input(BenchmarkId::new("random_graph", size), &size, |b, &size| {
            b.iter(|| {
                let graph = create_random_graph(size, 0.05);
                let _components =
                    connected_components(&graph).expect("connected components failed");
            });
        });
    }

    group.finish();
}

// ============================================================================
// Edge Case Benchmarks
// ============================================================================

fn bench_empty_graph(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("edge_cases");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("pagerank_empty", |b| {
        b.iter(|| {
            let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
            let _scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
        });
    });

    group.bench_function("betweenness_empty", |b| {
        b.iter(|| {
            let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
            let _centrality = betweenness_centrality(&graph).expect("betweenness failed");
        });
    });

    group.bench_function("label_prop_empty", |b| {
        b.iter(|| {
            let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
            let _communities = label_propagation(&graph, 10).expect("label propagation failed");
        });
    });

    group.bench_function("louvain_empty", |b| {
        b.iter(|| {
            let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
            let _communities = louvain_communities(&graph, 10).expect("louvain failed");
        });
    });

    group.finish();
}

fn bench_disconnected_components(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("edge_cases");
    group.sample_size(SAMPLE_SIZE);
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    group.bench_function("pagerank_disconnected", |b| {
        b.iter(|| {
            let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

            // Create two disconnected triangles
            let a = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "A".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");
            let b = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "B".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");
            let c = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "C".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");

            // Triangle 1
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: a,
                    to_id: b,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: b,
                    to_id: c,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: c,
                    to_id: a,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");

            // Triangle 2
            let d = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "D".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");
            let e = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "E".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");
            let f = graph
                .insert_entity(&sqlitegraph::GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: "F".into(),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .expect("insert");

            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: d,
                    to_id: e,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: e,
                    to_id: f,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");
            graph
                .insert_edge(&sqlitegraph::GraphEdge {
                    id: 0,
                    from_id: f,
                    to_id: d,
                    edge_type: "LINK".into(),
                    data: serde_json::json!({}),
                })
                .expect("insert");

            let _scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    // Centrality algorithms
    bench_pagerank,
    bench_betweenness_centrality,
    // Community detection
    bench_label_propagation,
    bench_louvain,
    // Baseline
    bench_connected_components,
    // Edge cases
    bench_empty_graph,
    bench_disconnected_components,
);

criterion_main!(benches);
