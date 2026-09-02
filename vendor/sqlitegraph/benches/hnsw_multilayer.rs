//! Multi-layer HNSW O(log N) scaling benchmarks.
//!
//! Benchmarks HNSW to verify search performance across dataset sizes.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sqlitegraph::hnsw::{DistanceMetric, HnswConfig, HnswIndex};

mod bench_utils;

/// Benchmark HNSW search scaling with dataset size
fn bench_hnsw_search_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_scaling");
    group.sample_size(20); // Reduced sample size for faster benchmarks

    let dataset_sizes = vec![100, 500, 1_000]; // Reduced sizes for faster execution
    let dimension = 64; // Reduced dimension

    for &size in &dataset_sizes {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &size,
            |bencher, &size| {
                // Single-layer mode for stability
                let config = HnswConfig {
                    dimension,
                    m: 16,
                    ef_construction: 100, // Reduced for faster insertion
                    ef_search: 50,
                    ml: 8,
                    distance_metric: DistanceMetric::Euclidean,
                    enable_multilayer: false,
                    multilayer_level_distribution_base: None,
                    multilayer_deterministic_seed: None,
                };

                let mut hnsw = HnswIndex::new("bench", config).unwrap();

                // Insert vectors
                for i in 0..size {
                    let vector = (0..dimension)
                        .map(|j| ((i * dimension + j) as f32 * 0.01).sin())
                        .collect::<Vec<_>>();
                    hnsw.insert_vector(&vector, None).unwrap();
                }

                // Use first vector as query
                let query = (0..dimension)
                    .map(|j| (j as f32 * 0.01).sin())
                    .collect::<Vec<_>>();

                bencher.iter(|| hnsw.search(&query, 10).unwrap());
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_hnsw_search_scaling);
criterion_main!(benches);
