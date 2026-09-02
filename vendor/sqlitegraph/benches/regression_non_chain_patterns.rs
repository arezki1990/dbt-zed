//! Non-chain pattern regression benchmark.
//!
//! Validates that Star, Random, and Tree graph traversals stay within 10% of v1.6 baseline.
//! This ensures the observe_with_cluster() optimization doesn't regress non-chain patterns.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rand::Rng;
use rand::SeedableRng;
use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, open_graph, snapshot::SnapshotId};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP, create_benchmark_temp_dir};

/// Create a star graph (1 center, N neighbors)
fn create_star_graph(size: usize) -> (tempfile::TempDir, std::path::PathBuf, i64) {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(size + 1);

    // Create nodes
    for i in 0..=size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create star edges (center to all others)
    let center = node_ids[0];
    for (i, node_id) in node_ids.iter().enumerate().skip(1).take(size) {
        graph
            .insert_edge(EdgeSpec {
                from: center,
                to: *node_id,
                edge_type: "star".to_string(),
                data: serde_json::json!({"spoke": i}),
            })
            .expect("Failed to insert edge");
    }

    (temp_dir, db_path, center)
}

/// Create a random graph
fn create_random_graph(
    size: usize,
    edge_count: usize,
) -> (tempfile::TempDir, std::path::PathBuf, i64) {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(size);

    // Create nodes
    for i in 0..size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create random edges
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xA17C);
    for _ in 0..edge_count {
        let from_idx = rng.gen_range(0..size);
        let mut to_idx = rng.gen_range(0..size);
        while to_idx == from_idx {
            to_idx = rng.gen_range(0..size);
        }

        let _ = graph.insert_edge(EdgeSpec {
            from: node_ids[from_idx],
            to: node_ids[to_idx],
            edge_type: "random".to_string(),
            data: serde_json::json!({"random_id": rng.r#gen::<u64>()}),
        });
    }

    (temp_dir, db_path, node_ids[0])
}

/// Create a tree graph (branching factor 3)
fn create_tree_graph(size: usize) -> (tempfile::TempDir, std::path::PathBuf, i64) {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(size);

    // Create nodes
    for i in 0..size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create tree edges (branching factor 3)
    let mut parent_idx = 0;
    let mut child_idx = 1;
    while child_idx < size && parent_idx < size {
        // Add up to 3 children per parent
        for _ in 0..3 {
            if child_idx >= size {
                break;
            }
            let _ = graph.insert_edge(EdgeSpec {
                from: node_ids[parent_idx],
                to: node_ids[child_idx],
                edge_type: "tree".to_string(),
                data: serde_json::json!({"parent": parent_idx, "child": child_idx}),
            });
            child_idx += 1;
        }
        parent_idx += 1;
    }

    (temp_dir, db_path, node_ids[0])
}

/// Benchmark BFS on star graphs
///
/// Star graphs have high degree at center, testing degree handling.
/// High degree (>1) should trigger immediate fallback from chain optimization.
fn bench_star_graph(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("non_chain_star");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500, 1000] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("star_bfs", size), &size, |b, &_size| {
            b.iter(|| {
                let (_temp_dir, db_path, start_node) = create_star_graph(size);

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to open graph");

                let _result = graph.bfs(SnapshotId::current(), start_node, size as u32);

                std::mem::forget(_temp_dir);
            });
        });
    }

    group.finish();
}

/// Benchmark BFS on random graphs
///
/// Random graphs have mixed degrees, testing general traversal.
/// Random patterns should not match chain detection.
fn bench_random_graph(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("non_chain_random");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500, 1000] {
        let edge_count = size * 2; // 2x edges for connectivity

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("random_bfs", size), &size, |b, &_size| {
            b.iter(|| {
                let (_temp_dir, db_path, start_node) = create_random_graph(size, edge_count);

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to open graph");

                let _result = graph.bfs(SnapshotId::current(), start_node, size as u32);

                std::mem::forget(_temp_dir);
            });
        });
    }

    group.finish();
}

/// Benchmark BFS on tree graphs
///
/// Tree graphs have branching but no cycles, testing fallback behavior.
/// Trees should trigger fallback when branching >1 is detected.
fn bench_tree_graph(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("non_chain_tree");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &size in &[100, 500, 1000] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("tree_bfs", size), &size, |b, &_size| {
            b.iter(|| {
                let (_temp_dir, db_path, start_node) = create_tree_graph(size);

                let graph =
                    open_graph(&db_path, &GraphConfig::native()).expect("Failed to open graph");

                let _result = graph.bfs(SnapshotId::current(), start_node, size as u32);

                std::mem::forget(_temp_dir);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_star_graph,
    bench_random_graph,
    bench_tree_graph
);
criterion_main!(benches);
