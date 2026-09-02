//! Memory profiling benchmarks for V3 backend.
//!
//! Run with: cargo bench --features memory_profiling -- memory_profiling
//!
//! Measures actual memory usage during graph operations:
//! - Memory per 1000 nodes during insertion (1K, 10K, 100K)
//! - Memory growth during BFS traversal (1000, 10000 nodes)

#[cfg(feature = "memory_profiling")]
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;

mod bench_utils;
#[cfg(feature = "memory_profiling")]
use bench_utils::{MEASURE, WARM_UP, create_v3_bench_context};

// ============================================================================
// MEMORY PROFILING UTILITIES
// ============================================================================

/// Read current RSS (Resident Set Size) in bytes from /proc/self/status
///
/// Returns the VmRSS value which represents the actual physical memory used.
/// Returns 0 on unsupported platforms (non-Linux).
#[cfg(feature = "memory_profiling")]
fn get_rss_bytes() -> usize {
    use std::fs::File;
    use std::io::BufRead;

    if let Ok(file) = File::open("/proc/self/status") {
        let reader = std::io::BufReader::new(file);
        for line in reader.lines().flatten() {
            if line.starts_with("VmRSS:") {
                // Format: "VmRSS:     12345 kB"
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<usize>() {
                        return kb * 1024; // Convert KB to bytes
                    }
                }
            }
        }
    }
    0 // Unsupported platform or parse failure
}

// ============================================================================
// MEMORY PER 1000 NODES BENCHMARK
// ============================================================================

#[cfg(feature = "memory_profiling")]
fn bench_memory_per_1000_nodes(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("memory_per_1000_nodes");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);
    group.sample_size(10);

    for thousand_nodes in [1, 10, 100] {
        let size = thousand_nodes * 1000;
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(
            BenchmarkId::new("insertion", thousand_nodes),
            &size,
            |b, &size| {
                b.iter_batched(
                    || create_v3_bench_context("v3.db"),
                    |ctx| {
                        let backend = &ctx.backend;
                        let rss_before = get_rss_bytes();

                        // Insert nodes
                        for i in 0..size {
                            black_box(
                                backend
                                    .insert_node(NodeSpec {
                                        kind: "Node".to_string(),
                                        name: format!("node_{}", i),
                                        file_path: None,
                                        data: serde_json::json!({"id": i}),
                                    })
                                    .unwrap(),
                            );
                        }

                        let rss_after = get_rss_bytes();

                        // Calculate memory per 1000 nodes
                        let memory_delta = rss_after.saturating_sub(rss_before);
                        let per_1000 = memory_delta / (size / 1000).max(1);

                        // Report as metric (bytes per 1000 nodes)
                        black_box(per_1000);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

// ============================================================================
// MEMORY DURING TRAVERSAL BENCHMARK
// ============================================================================

#[cfg(feature = "memory_profiling")]
fn bench_memory_during_traversal(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("memory_during_traversal");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);
    group.sample_size(10);

    for size in [1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(
            BenchmarkId::new("bfs_traversal", size),
            &size,
            |b, &size| {
                b.iter_batched(
                    || {
                        let ctx = create_v3_bench_context("v3.db");
                        let backend = &ctx.backend;

                        // Create a chain graph for traversal
                        let mut node_ids = Vec::with_capacity(size);
                        for i in 0..size {
                            let node_id = backend
                                .insert_node(NodeSpec {
                                    kind: "Node".to_string(),
                                    name: format!("node_{}", i),
                                    file_path: None,
                                    data: serde_json::json!({"id": i}),
                                })
                                .unwrap();
                            node_ids.push(node_id);

                            // Create chain edges
                            if i > 0 {
                                backend
                                    .insert_edge(EdgeSpec {
                                        from: node_ids[i - 1],
                                        to: node_id,
                                        edge_type: "chain".to_string(),
                                        data: serde_json::json!({"order": i}),
                                    })
                                    .unwrap();
                            }
                        }

                        (ctx, node_ids)
                    },
                    |(ctx, node_ids)| {
                        let backend = &ctx.backend;
                        let snapshot = SnapshotId::current();
                        let rss_before = get_rss_bytes();

                        // BFS traversal
                        let start_node = node_ids[0];
                        let mut visited = std::collections::HashSet::new();
                        let mut queue = vec![start_node];
                        visited.insert(start_node);

                        while let Some(node_id) = queue.pop() {
                            let neighbors = backend
                                .neighbors(
                                    snapshot,
                                    node_id,
                                    NeighborQuery {
                                        direction: BackendDirection::Outgoing,
                                        edge_type: None,
                                    },
                                )
                                .unwrap();

                            for &neighbor in &neighbors {
                                if visited.insert(neighbor) {
                                    queue.push(neighbor);
                                }
                            }
                        }

                        let rss_after = get_rss_bytes();
                        let memory_growth = rss_after.saturating_sub(rss_before);

                        // Report memory growth during traversal
                        black_box(memory_growth);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

// ============================================================================
// CRITERION MAIN
// ============================================================================

#[cfg(feature = "memory_profiling")]
criterion_group!(
    benches,
    bench_memory_per_1000_nodes,
    bench_memory_during_traversal
);
#[cfg(feature = "memory_profiling")]
criterion_main!(benches);

// ============================================================================
// STUB MAIN WHEN FEATURE NOT ENABLED
// ============================================================================

#[cfg(not(feature = "memory_profiling"))]
fn main() {
    eprintln!("ERROR: Memory profiling benchmarks require the 'memory_profiling' feature.");
    eprintln!("Run with: cargo bench --features memory_profiling -- memory_profiling");
    std::process::exit(1);
}
