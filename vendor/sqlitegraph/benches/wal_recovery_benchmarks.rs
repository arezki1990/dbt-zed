//! WAL Recovery performance benchmarks.
//!
//! Compares sequential vs parallel WAL recovery performance
//! to validate parallelization improvements.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sqlitegraph::{GraphConfig, NodeSpec, open_graph};
use std::time::Duration;
use tempfile::TempDir;

mod bench_utils;
use bench_utils::create_benchmark_temp_dir;

const WARM_UP_TIME: Duration = Duration::from_secs(2);
const MEASURE_TIME: Duration = Duration::from_secs(5);

/// Benchmark sequential WAL recovery
fn bench_sequential_recovery(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wal_recovery_sequential");
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for transaction_count in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(transaction_count),
            transaction_count,
            |b, &tx_count| {
                b.iter_batched(
                    || setup_recovery_benchmark(tx_count, 1),
                    |(db_path, _temp_dir)| {
                        // Measure recovery time (sequential with parallelism=1)
                        let config = GraphConfig::native().with_parallel_recovery(1); // Sequential

                        let _graph =
                            open_graph(&db_path, &config).expect("Failed to recover graph");
                        drop(_graph);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark parallel WAL recovery
fn bench_parallel_recovery(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wal_recovery_parallel");
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for transaction_count in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(transaction_count),
            transaction_count,
            |b, &tx_count| {
                b.iter_batched(
                    || setup_recovery_benchmark(tx_count, 4),
                    |(db_path, _temp_dir)| {
                        // Measure recovery time (parallel with parallelism=4)
                        let config = GraphConfig::native().with_parallel_recovery(4); // Parallel

                        let _graph =
                            open_graph(&db_path, &config).expect("Failed to recover graph");
                        drop(_graph);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark different parallelism degrees
fn bench_parallelism_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wal_recovery_scaling");
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    // Fixed transaction count, varying parallelism
    let transaction_count = 100;

    for parallelism in [1, 2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::new("parallelism", parallelism),
            parallelism,
            |b, &parallelism| {
                b.iter_batched(
                    || setup_recovery_benchmark(transaction_count, parallelism),
                    |(db_path, _temp_dir)| {
                        // Measure recovery time with specified parallelism
                        let config = GraphConfig::native().with_parallel_recovery(parallelism);

                        let _graph =
                            open_graph(&db_path, &config).expect("Failed to recover graph");
                        drop(_graph);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark throughput (transactions per second)
fn bench_throughput(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("wal_recovery_throughput");
    group.warm_up_time(WARM_UP_TIME);
    group.measurement_time(MEASURE_TIME);

    for (tx_count, parallelism) in [(100, 1), (100, 4), (500, 4)] {
        group.throughput(Throughput::Elements(tx_count as u64));

        group.bench_with_input(
            BenchmarkId::new(format!("{}_parallel_{}", tx_count, parallelism), tx_count),
            &(tx_count, parallelism),
            |b, &(tx_count, parallelism)| {
                b.iter_batched(
                    || setup_recovery_benchmark(tx_count, parallelism),
                    |(db_path, _temp_dir)| {
                        let config = GraphConfig::native().with_parallel_recovery(parallelism);

                        let _graph =
                            open_graph(&db_path, &config).expect("Failed to recover graph");
                        drop(_graph);
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Setup function for WAL recovery benchmarks
///
/// Creates a graph with the specified number of transactions,
/// then closes it to force WAL generation. Returns the database
/// path and temp directory for cleanup.
fn setup_recovery_benchmark(
    transaction_count: usize,
    _parallelism: usize,
) -> (std::path::PathBuf, TempDir) {
    let temp_dir = create_benchmark_temp_dir();
    let db_path = temp_dir.path().join("benchmark.db");

    // Create graph and generate WAL by inserting nodes
    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    // Insert nodes to generate WAL records (each insert is a transaction)
    for i in 0..transaction_count {
        let _ = graph.insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "benchmark": true}),
        });
    }

    // Force WAL flush by dropping graph
    drop(graph);

    (db_path, temp_dir)
}

criterion_group!(
    benches,
    bench_sequential_recovery,
    bench_parallel_recovery,
    bench_parallelism_scaling,
    bench_throughput
);
criterion_main!(benches);
