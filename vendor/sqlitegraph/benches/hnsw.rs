//! HNSW (Hierarchical Navigable Small World) vector search performance benchmarks.
//!
//! Benchmarks HNSW vector insertion, search, and memory usage patterns
//! using the criterion benchmarking framework integrated with SQLiteGraph.
//! - Performance impact of different distance metrics
//! - Support for various vector dimensions including OpenAI embeddings (1536)
//! - Scalability analysis across different dataset sizes
//! - SIMD vs scalar performance comparisons for distance functions

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sqlitegraph::hnsw::DistanceMetric;
use sqlitegraph::hnsw::simd::{
    compute_norm_squared, compute_norm_squared_scalar, cosine_similarity, cosine_similarity_scalar,
    dot_product, dot_product_scalar, euclidean_distance, euclidean_distance_scalar,
};

mod bench_utils;
use bench_utils::{MEASURE, WARM_UP};

/// Generate test vectors with specified dimensions and count
///
/// This function generates deterministic test vectors suitable for benchmarking
/// different vector dimensions including:
/// - Small embeddings (64-256 dimensions): Custom models, sentence embeddings
/// - Medium embeddings (512-768 dimensions): BERT, sentence transformers
/// - Large embeddings (1536 dimensions): OpenAI text-embedding-ada-002, text-embedding-3-small
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::with_capacity(count);
    for i in 0..count {
        let mut vector = Vec::with_capacity(dimension);
        for j in 0..dimension {
            // Generate deterministic but varied vectors
            // Uses sine function with position-based seeds for reproducible results
            let value = ((i as f32 * 0.1) + (j as f32 * 0.01)).sin();
            vector.push(value);
        }
        vectors.push(vector);
    }
    vectors
}

/// Create HNSW index with specified configuration
///
/// Creates a standardized HNSW index for benchmarking with configurable dimensions.
/// This function supports all vector dimensions commonly used in production:
/// - 64-256: Small embeddings for efficiency-critical applications
/// - 512-768: Medium embeddings (BERT, sentence transformers)
/// - 1536: Large embeddings (OpenAI text-embedding-ada-002, text-embedding-3-small)
fn create_hnsw_index(
    dimension: usize,
    ef_construction: usize,
    ef_search: usize,
) -> sqlitegraph::hnsw::HnswIndex {
    let config = sqlitegraph::hnsw::hnsw_config()
        .dimension(dimension)
        .m_connections(16)
        .ef_construction(ef_construction)
        .ef_search(ef_search)
        .distance_metric(DistanceMetric::Cosine)
        .build()
        .expect("HNSW configuration should be valid");

    sqlitegraph::hnsw::HnswIndex::new("benchmark_index", config)
        .expect("Failed to create HNSW index")
}

/// Benchmark vector insertion performance
fn hnsw_vector_insertion(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_insertion");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Comprehensive dimension coverage including OpenAI embeddings
    let dimensions = vec![64, 128, 256, 512, 768, 1536];
    let dataset_sizes = vec![100, 500, 1000];

    for &dimension in &dimensions {
        for &dataset_size in &dataset_sizes {
            let bench_id = BenchmarkId::new(
                "insertion",
                format!("dim{}_size{}", dimension, dataset_size),
            );

            group.bench_function(bench_id, |b| {
                b.iter(|| {
                    let mut hnsw = create_hnsw_index(dimension, 200, 50);
                    let vectors = generate_test_vectors(dataset_size, dimension);

                    for vector in &vectors {
                        hnsw.insert_vector(vector, None)
                            .expect("Failed to insert vector");
                    }
                })
            });
        }
    }

    group.finish();
}

/// Benchmark search query performance
fn hnsw_search_performance(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_search");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Comprehensive dimension coverage including OpenAI embeddings
    let dimensions = vec![64, 128, 256, 512, 768, 1536];
    let dataset_sizes = vec![100, 500, 1000];
    let k_values = vec![1, 5, 10];

    for &dimension in &dimensions {
        for &dataset_size in &dataset_sizes {
            for &k in &k_values {
                let bench_id = BenchmarkId::new(
                    "search",
                    format!("dim{}_size{}_k{}", dimension, dataset_size, k),
                );

                group.bench_function(bench_id, |b| {
                    // Setup: Create HNSW index and insert vectors
                    let mut hnsw = create_hnsw_index(dimension, 200, 50);
                    let vectors = generate_test_vectors(dataset_size, dimension);
                    for vector in &vectors {
                        hnsw.insert_vector(vector, None)
                            .expect("Failed to insert vector");
                    }

                    let query = &vectors[0];

                    b.iter(|| hnsw.search(query, k).expect("Failed to search"))
                });
            }
        }
    }

    group.finish();
}

/// Benchmark different distance metrics performance
fn hnsw_distance_metrics(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_metrics");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Test with multiple dimensions including OpenAI embeddings
    let test_dimensions = vec![512, 768, 1536];
    let dataset_size = 1000;
    let k = 10;

    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];

    for &dimension in &test_dimensions {
        for metric in &metrics {
            let bench_id = BenchmarkId::new("metrics", format!("dim{}_{:?}", dimension, metric));

            group.bench_function(bench_id, |b| {
                b.iter(|| {
                    let config = sqlitegraph::hnsw::hnsw_config()
                        .dimension(dimension)
                        .m_connections(16)
                        .ef_construction(200)
                        .ef_search(50)
                        .distance_metric(*metric)
                        .build()
                        .expect("HNSW configuration should be valid");

                    let mut hnsw = sqlitegraph::hnsw::HnswIndex::new("benchmark_metrics", config)
                        .expect("Failed to create HNSW index");

                    let vectors = generate_test_vectors(dataset_size, dimension);
                    for vector in &vectors {
                        hnsw.insert_vector(vector, None)
                            .expect("Failed to insert vector");
                    }

                    let query = &vectors[0];
                    hnsw.search(query, k).expect("Failed to search")
                })
            });
        }
    }

    group.finish();
}

/// Simple end-to-end benchmark: insert + search operations
fn hnsw_end_to_end_performance(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_e2e");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    // Comprehensive dimension coverage including OpenAI embeddings
    let dimensions = vec![64, 128, 256, 512, 768, 1536];
    let dataset_sizes = vec![100, 500, 1000];

    for &dimension in &dimensions {
        for &dataset_size in &dataset_sizes {
            let bench_id =
                BenchmarkId::new("e2e", format!("dim{}_size{}", dimension, dataset_size));

            group.bench_function(bench_id, |b| {
                b.iter(|| {
                    let mut hnsw = create_hnsw_index(dimension, 200, 50);
                    let vectors = generate_test_vectors(dataset_size, dimension);

                    // Insert vectors
                    for vector in &vectors {
                        hnsw.insert_vector(vector, None)
                            .expect("Failed to insert vector");
                    }

                    // Perform multiple searches
                    let query = &vectors[0];
                    for _ in 0..10 {
                        hnsw.search(query, 10).expect("Failed to search");
                    }

                    hnsw
                })
            });
        }
    }

    group.finish();
}

/// Benchmark OpenAI embedding performance specifically
///
/// This benchmark focuses on OpenAI text-embedding-ada-002 and text-embedding-3-small
/// which use 1536 dimensions. It provides realistic performance expectations for
/// production workloads using OpenAI embeddings.
fn hnsw_openai_embeddings(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("hnsw_openai");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let openai_dimension = 1536;
    let realistic_dataset_sizes = vec![1000, 5000, 10000];
    let k_values = vec![5, 10, 20]; // Typical values for semantic search

    for &dataset_size in &realistic_dataset_sizes {
        for &k in &k_values {
            let bench_id = BenchmarkId::new("openai_1536", format!("size{}_k{}", dataset_size, k));

            group.bench_function(bench_id, |b| {
                // Setup: Create HNSW index optimized for OpenAI embeddings
                let mut hnsw = create_hnsw_index(openai_dimension, 200, 50);
                let vectors = generate_test_vectors(dataset_size, openai_dimension);

                // Insert all vectors
                for vector in &vectors {
                    hnsw.insert_vector(vector, None)
                        .expect("Failed to insert vector");
                }

                let query = &vectors[0];

                b.iter(|| hnsw.search(query, k).expect("Failed to search"))
            });
        }
    }

    group.finish();
}

// ============================================================================
// SIMD vs Scalar Benchmarks
// ============================================================================

/// Generate benchmark vectors for SIMD performance comparison
///
/// Creates deterministic vectors with varied values to exercise SIMD paths.
/// This function is optimized for SIMD vs scalar performance comparisons.
fn benchmark_vectors(dim: usize) -> (Vec<f32>, Vec<f32>) {
    let a: Vec<f32> = (0..dim).map(|i| i as f32 * 0.1).collect();
    let b: Vec<f32> = (dim..dim * 2).map(|i| i as f32 * 0.1).collect();
    (a, b)
}

/// Benchmark dot product: scalar vs SIMD (multiple vector sizes)
///
/// Compares scalar fallback implementation against AVX2-accelerated version
/// for different vector dimensions. Expected speedup: 4-6x for large vectors.
fn simd_dot_product_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("simd_dot_product");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let vector_sizes = vec![128, 768, 1536]; // Common embedding dimensions

    for &size in &vector_sizes {
        // Scalar implementation
        group.bench_with_input(BenchmarkId::new("scalar", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            b.iter(|| dot_product_scalar(&a, &b_vec));
        });

        // SIMD implementation (runtime dispatch)
        group.bench_with_input(BenchmarkId::new("simd", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            b.iter(|| dot_product(&a, &b_vec));
        });
    }

    group.finish();
}

/// Benchmark Euclidean distance: scalar vs SIMD
///
/// Compares scalar fallback against AVX2 implementation for L2 distance.
/// Expected speedup: 4-6x for large vectors.
fn simd_euclidean_distance_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("simd_euclidean_distance");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let vector_sizes = vec![128, 768, 1536];

    for &size in &vector_sizes {
        group.bench_with_input(BenchmarkId::new("scalar", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            b.iter(|| euclidean_distance_scalar(&a, &b_vec));
        });

        group.bench_with_input(BenchmarkId::new("simd", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            b.iter(|| euclidean_distance(&a, &b_vec));
        });
    }

    group.finish();
}

/// Benchmark cosine similarity: scalar vs SIMD
///
/// Compares scalar fallback against AVX2 implementation for cosine similarity.
/// Expected speedup: 4-6x for large vectors (includes norm computation).
fn simd_cosine_similarity_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("simd_cosine_similarity");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let vector_sizes = vec![128, 768, 1536];

    for &size in &vector_sizes {
        group.bench_with_input(BenchmarkId::new("scalar", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            // Normalize vectors for cosine similarity
            let norm_a = compute_norm_squared_scalar(&a).sqrt();
            let norm_b = compute_norm_squared_scalar(&b_vec).sqrt();
            let a_norm: Vec<f32> = a.iter().map(|x| x / norm_a).collect();
            let b_norm: Vec<f32> = b_vec.iter().map(|x| x / norm_b).collect();
            b.iter(|| cosine_similarity_scalar(&a_norm, &b_norm));
        });

        group.bench_with_input(BenchmarkId::new("simd", size), &size, |b, &size| {
            let (a, b_vec) = benchmark_vectors(size);
            // Normalize vectors for cosine similarity
            let norm_a = compute_norm_squared(&a).sqrt();
            let norm_b = compute_norm_squared(&b_vec).sqrt();
            let a_norm: Vec<f32> = a.iter().map(|x| x / norm_a).collect();
            let b_norm: Vec<f32> = b_vec.iter().map(|x| x / norm_b).collect();
            b.iter(|| cosine_similarity(&a_norm, &b_norm));
        });
    }

    group.finish();
}

/// Benchmark norm computation: scalar vs SIMD
///
/// Compares scalar against AVX2 for L2 norm squared computation.
/// This is a building block for cosine similarity.
fn simd_norm_squared_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("simd_norm_squared");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let vector_sizes = vec![128, 768, 1536];

    for &size in &vector_sizes {
        group.bench_with_input(BenchmarkId::new("scalar", size), &size, |b, &size| {
            let (a, _) = benchmark_vectors(size);
            b.iter(|| compute_norm_squared_scalar(&a));
        });

        group.bench_with_input(BenchmarkId::new("simd", size), &size, |b, &size| {
            let (a, _) = benchmark_vectors(size);
            b.iter(|| compute_norm_squared(&a));
        });
    }

    group.finish();
}

/// Benchmark batch filter operations: scalar vs SIMD
///
/// Compares HashSet-based scalar filtering against AVX2 implementation.
/// Expected speedup: 2-3x for large batches.
fn simd_batch_filter_benchmarks(criterion: &mut Criterion) {
    use sqlitegraph::hnsw::batch_filter::{filter_allowed_scalar, filter_batch};

    let mut group = criterion.benchmark_group("simd_batch_filter");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let batch_sizes = vec![100, 1000, 10000];

    for &size in &batch_sizes {
        let ids: Vec<u64> = (0..size).collect();
        let allowed: Vec<u64> = (0..size / 2).collect();

        group.bench_with_input(
            BenchmarkId::new("scalar", size),
            &(ids.clone(), allowed.clone()),
            |b, (ids, allowed)| {
                b.iter(|| filter_allowed_scalar(ids, allowed));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("simd", size),
            &(ids.clone(), allowed.clone()),
            |b, (ids, allowed)| {
                b.iter(|| filter_batch(ids, allowed, true));
            },
        );
    }

    group.finish();
}

/// Benchmark delta encoding: scalar vs SIMD
///
/// Compares scalar loop against AVX2 parallel delta computation.
/// Expected speedup: 3-5x for large arrays (> 100 elements).
fn simd_delta_encode_benchmarks(criterion: &mut Criterion) {
    use sqlitegraph::hnsw::serialization::{delta_encode, delta_encode_scalar};

    let mut group = criterion.benchmark_group("simd_delta_encode");
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    let array_sizes = vec![100, 1000, 10000];

    for &size in &array_sizes {
        let values: Vec<u32> = (0..size).map(|i| (i * 10) as u32).collect();

        group.bench_with_input(BenchmarkId::new("scalar", size), &values, |b, values| {
            b.iter(|| delta_encode_scalar(values));
        });

        group.bench_with_input(BenchmarkId::new("simd", size), &values, |b, values| {
            b.iter(|| delta_encode(values));
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    hnsw_vector_insertion,
    hnsw_search_performance,
    hnsw_distance_metrics,
    hnsw_end_to_end_performance,
    hnsw_openai_embeddings,
    simd_dot_product_benchmarks,
    simd_euclidean_distance_benchmarks,
    simd_cosine_similarity_benchmarks,
    simd_norm_squared_benchmarks,
    simd_batch_filter_benchmarks,
    simd_delta_encode_benchmarks
);

criterion_main!(benches);
