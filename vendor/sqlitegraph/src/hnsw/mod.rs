//! Hierarchical Navigable Small World (HNSW) Vector Search
//!
//! This module provides high-performance approximate nearest neighbor (ANN) search
//! using the HNSW algorithm. HNSW builds a multi-layer graph structure where
//! higher layers provide "express lanes" for fast navigation, while the bottom
//! layer contains all vectors for precise search.
//!
//! # What is HNSW?
//!
//! HNSW (Hierarchical Navigable Small World) is a graph-based algorithm for
//! efficient approximate nearest neighbor search in high-dimensional vector spaces.
//! It constructs a hierarchical graph where:
//!
//! - **Layer 0** contains all vectors with dense connectivity
//! - **Higher layers** contain exponentially fewer vectors, forming "skip lists"
//! - **Search starts** at the top layer and greedily descends to the bottom
//! - **Result**: O(log N) search complexity with high recall
//!
//! This design provides excellent trade-offs between search speed, accuracy,
//! and memory usage for vector similarity search.
//!
//! # Module Architecture
//!
//! The HNSW implementation is organized into focused modules:
//!
//! - **config**: [`HnswConfig`] - Algorithm configuration and parameters
//! - **builder**: [`HnswConfigBuilder`] - Fluent configuration builder with validation
//! - **index**: [`HnswIndex`] - Main HNSW index implementation with search/insert
//! - **storage**: [`VectorStorage`] trait - Pluggable vector persistence (in-memory/SQLite)
//! - **distance_metric**: SIMD-ready vector distance calculations
//! - **distance_functions**: Low-level distance functions with SIMD optimization
//! - **simd**: SIMD-accelerated distance functions with runtime CPU detection
//! - **batch_filter**: SIMD-accelerated batch ID filtering for multi-tenant operations
//! - **serialization**: SIMD-accelerated varint and delta encoding for HNSW persistence
//! - **errors**: Comprehensive error handling for all HNSW operations
//! - **multilayer**: Multi-layer graph construction and management
//! - **neighborhood**: Neighbor selection and heuristics for graph connectivity
//!
//! # Key Types
//!
//! - [`HnswIndex`] - Main HNSW index with insert, search, and persistence
//! - [`HnswConfig`] - Configuration parameters (dimension, M, ef_construction)
//! - [`VectorStorage`] - Pluggable storage backend for vectors
//! - [`DistanceMetric`] - Supported distance metrics (Cosine, Euclidean, etc.)
//!
//! # Invariants and Guarantees
//!
//! ## Approximate Results
//!
//! HNSW provides **approximate** nearest neighbor search, not exact results:
//!
//! - **Recall depends** on `ef_construction` and `ef_search` parameters
//! - **Higher ef = better recall** but slower search
//! - **Typical recall**: 95%+ for well-tuned parameters
//! - **No guarantee** of finding exact nearest neighbor
//!
//! ## Determinism
//!
//! - **Same input + same config → same results** (deterministic)
//! - **Random seed** controls layer assignment (via `multilayer_deterministic_seed`)
//! - **Insert order affects** graph structure (but not correctness)
//!
//! ## Thread Safety
//!
//! **NOT thread-safe for concurrent writes.** `HnswIndex` uses interior mutability
//! and is not `Sync`. Do not share across threads for insert operations.
//!
//! For concurrent search:
//! - Read-only search operations can access shared data
//! - Use separate indexes per thread for writes
//! - Or wrap in `Mutex`/`RwLock` for explicit synchronization
//!
//! ## Vector Dimension Consistency
//!
//! All vectors in an index must have the **same dimension**:
//!
//! - **Configured at** index creation time via `HnswConfig::dimension`
//! - **Enforced on** insert - returns error for mismatched dimensions
//! - **Cannot change** after index creation (recreate index to change)
//!
//! # Performance Characteristics
//!
//! ## Search Performance
//! - **Time Complexity**: O(log N) average case
//! - **Space Complexity**: O(N × M) where M = max_connections per node
//! - **Accuracy**: 95%+ recall for typical workloads with proper tuning
//!
//! ## Insert Performance
//! - **Time Complexity**: O(log N) average case
//! - **Amortized**: Faster than rebuilding entire index
//! - **Dynamic**: No full rebuild required for new vectors
//!
//! ## Memory Usage
//! - **Base overhead**: 2-3x vector data size
//! - **Graph edges**: O(N × M) where M is configurable (default 16)
//! - **Multi-layer**: Additional ~15% overhead for higher layers
//!
//! # Configuration Parameters
//!
//! Key parameters in [`HnswConfig`]:
//!
//! ## `dimension` (Required)
//! - Vector dimensionality (e.g., 768 for sentence embeddings)
//! - Must match all vectors inserted into the index
//! - Cannot be changed after index creation
//!
//! ## `m` (max_connections, default: 16)
//! - **Max connections** per node in the graph
//! - **Higher M**: Better recall, more memory, slower inserts
//! - **Lower M**: Faster inserts, less memory, lower recall
//! - **Recommended**: 12-32 depending on dataset size
//!
//! ## `ef_construction` (default: 200)
//! - **Candidate list size** during index construction
//! - **Higher ef**: Better graph quality, higher recall, slower builds
//! - **Must be ≥ M**: Required for valid index construction
//! - **Recommended**: 2× M for good quality
//!
//! ## `ef_search` (default: 50)
//! - **Candidate list size** during search operations
//! - **Higher ef**: Better recall, slower search
//! - **Can adjust** at runtime without rebuilding index
//! - **Recommended**: Same as ef_construction or slightly lower
//!
//! # Persistence Behavior
//!
//! HNSW indexes persist across database sessions:
//!
//! ## Metadata Persistence
//! - Index name and configuration saved to `hnsw_indexes` table
//! - Automatically restored on `SqliteGraph::open()`
//! - Survives database restarts and reconnections
//!
//! ## Vector Persistence
//! - Vectors persisted to `hnsw_vectors` table (file-based databases)
//! - In-memory databases use in-memory storage (no persistence)
//! - Graph structure rebuilt from persisted vectors on load
//!
//! ## Limitations
//! - **Graph edges NOT persisted** - rebuilt from vectors on load
//! - **Rebuild cost**: O(N log N) on index open
//! - **Workaround**: Keep index in-memory for hot restarts
//!
//! # Usage Examples
//!
//! ## Create Index and Insert Vectors
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, hnsw::{HnswConfig, DistanceMetric}};
//!
//! let graph = SqliteGraph::open("vectors.db")?;
//!
//! let config = HnswConfig::builder()
//!     .dimension(768)
//!     .m_connections(16)
//!     .ef_construction(200)
//!     .ef_search(50)
//!     .distance_metric(DistanceMetric::Cosine)
//!     .build()?;
//!
//! let mut index = graph.create_hnsw_index("docs", config)?;
//!
//! // Insert vectors
//! for (id, vector) in vectors {
//!     index.insert(id, &vector)?;
//! }
//! ```
//!
//! ## Search for Nearest Neighbors
//!
//! ```rust,ignore
//! # use sqlitegraph::hnsw::HnswIndex;
//! # let mut index: HnswIndex = unsafe { std::mem::zeroed() };
//! # let query_vector: Vec<f32> = vec![];
//! let results = index.search(&query_vector, 10)?;
//!
//! for (vector_id, distance) in results {
//!     println!("ID: {}, Distance: {}", vector_id, distance);
//! }
//! ```
//!
//! # Distance Metrics
//!
//! Supported distance metrics via [`DistanceMetric`]:
//!
//! - **Cosine**: Ideal for normalized vectors and text embeddings
//!   - Range: [0, 2] where 0 = identical
//!   - Use case: Semantic similarity, document embeddings
//!
//! - **Euclidean**: L2 distance, general-purpose similarity
//!   - Range: [0, ∞) where 0 = identical
//!   - Use case: Image embeddings, feature vectors
//!
//! - **Dot Product**: Fast approximate cosine for normalized vectors
//!   - Range: [-1, 1] where 1 = identical (negated for min-heap)
//!   - Use case: Pre-normalized embeddings, fast approximate search
//!
//! - **Manhattan**: L1 distance, robust to outliers
//!   - Range: [0, ∞) where 0 = identical
//!   - Use case: Sparse vectors, robust similarity search
//!
//! # Error Handling
//!
//! All HNSW operations return `Result` types with comprehensive errors:
//!
//! ```rust,ignore
//! use sqlitegraph::hnsw::{HnswConfig, HnswConfigError};
//!
//! match HnswConfig::builder().dimension(0).build() {
//!     Ok(config) => println!("Valid config"),
//!     Err(HnswConfigError::InvalidDimension) => {
//!         println!("Vector dimension must be > 0");
//!     }
//!     Err(e) => println!("Other error: {}", e),
//! }
//! ```

// Re-export public API
pub use builder::HnswConfigBuilder;
pub use config::{HnswConfig, hnsw_config};
pub use distance_metric::{DistanceMetric, compute_distance};
pub use errors::{
    HnswConfigError, HnswError, HnswIndexError, HnswMultiLayerError, HnswStorageError,
};
pub use index::{HnswIndex, HnswIndexStats};
pub use storage::{
    InMemoryVectorStorage, VectorBatch, VectorRecord, VectorStorage, VectorStorageStats,
};

// Multi-layer HNSW components
pub use multilayer::{LayerMappings, LevelDistributor, MultiLayerNodeManager};

// Module organization
pub mod batch_filter;
pub mod builder;
pub mod config;
pub mod distance_functions;
pub mod distance_metric;
pub mod errors;
pub mod index;
pub mod layer;
pub mod multilayer;
pub mod neighborhood;
pub mod serialization;
pub mod simd;
pub mod storage;
#[cfg(feature = "native-v3")]
pub mod v3_storage;

// Re-export batch_filter public API
pub use batch_filter::{filter_allowed_scalar, filter_batch, filter_denied_scalar};

// Re-export serialization public API
pub use serialization::{decode_varint_scalar, delta_decode, delta_encode, encode_varint_scalar};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hnsw_module_imports() {
        // Test that all modules can be imported successfully
        let _config = HnswConfig::default();
        let _builder = HnswConfigBuilder::new();
        let _metric = DistanceMetric::Cosine;

        // Test default configuration
        assert_eq!(_config.dimension, 768);
        assert_eq!(_config.distance_metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_hnsw_config_builder() {
        let config = HnswConfigBuilder::new()
            .dimension(256)
            .m_connections(12)
            .ef_construction(150)
            .ef_search(40)
            .max_layers(12)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();

        assert_eq!(config.dimension, 256);
        assert_eq!(config.m, 12);
        assert_eq!(config.ef_construction, 150);
        assert_eq!(config.ef_search, 40);
        assert_eq!(config.ml, 12);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean);
    }

    #[test]
    fn test_distance_metrics() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];

        let cosine_dist = compute_distance(DistanceMetric::Cosine, &a, &b);
        let euclidean_dist = compute_distance(DistanceMetric::Euclidean, &a, &b);
        let manhattan_dist = compute_distance(DistanceMetric::Manhattan, &a, &b);
        let dot_dist = compute_distance(DistanceMetric::DotProduct, &a, &b);

        assert!((cosine_dist - 0.5).abs() < f32::EPSILON); // (1 - 0) / 2
        assert!((euclidean_dist - std::f32::consts::SQRT_2).abs() < f32::EPSILON);
        assert_eq!(manhattan_dist, 2.0);
        assert_eq!(dot_dist, 0.0); // -dot_product
    }

    #[test]
    fn test_error_handling() {
        // Test that the build() method catches validation errors
        let result = HnswConfigBuilder::new().build(); // Default has dimension 768, so this should pass
        assert!(result.is_ok());

        // Test dimension 0 should be caught by build() method
        let mut builder = HnswConfigBuilder::new();
        builder.config.dimension = 0; // Direct field access for testing
        let result = builder.build();
        assert!(matches!(result, Err(HnswConfigError::InvalidDimension)));
    }

    #[test]
    fn test_hnsw_config_function() {
        let config = hnsw_config()
            .dimension(512)
            .m_connections(24)
            .distance_metric(DistanceMetric::Manhattan)
            .build()
            .unwrap();

        assert_eq!(config.dimension, 512);
        assert_eq!(config.m, 24);
        assert_eq!(config.distance_metric, DistanceMetric::Manhattan);
    }

    #[test]
    fn test_default_configuration() {
        let config = HnswConfig::default();

        // Verify reasonable defaults
        assert_eq!(config.dimension, 768); // Common embedding size
        assert_eq!(config.m, 16); // Balanced connectivity
        assert_eq!(config.ef_construction, 200); // Good construction quality
        assert_eq!(config.ef_search, 50); // Balanced search speed/quality
        assert_eq!(config.ml, 16); // Reasonable depth
        assert_eq!(config.distance_metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_high_precision_configuration() {
        let config = HnswConfig {
            dimension: 1536,
            m: 32,
            ef_construction: 400,
            ef_search: 100,
            ml: 24,
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: false,
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        };

        assert_eq!(config.dimension, 1536);
        assert_eq!(config.m, 32);
        assert!(config.ef_construction >= config.m);
        assert_eq!(config.distance_metric, DistanceMetric::Cosine);
        assert!(!config.enable_multilayer);
    }

    #[test]
    fn test_fast_construction_configuration() {
        let config = HnswConfig {
            dimension: 384,
            m: 8,
            ef_construction: 100,
            ef_search: 20,
            ml: 12,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: false,
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        };

        assert_eq!(config.dimension, 384);
        assert_eq!(config.m, 8);
        assert_eq!(config.ef_construction, 100);
        assert_eq!(config.ef_search, 20);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean);
        assert!(!config.enable_multilayer);
    }

    #[test]
    fn test_batch_filter_module() {
        let ids = vec![1, 2, 3, 4, 5];
        let allowed = vec![2, 3, 4];
        let filtered = crate::hnsw::batch_filter::filter_allowed_scalar(&ids, &allowed);
        assert_eq!(filtered, vec![2, 3, 4]);
    }

    #[test]
    fn test_serialization_module() {
        use crate::hnsw::serialization::{decode_varint_scalar, encode_varint_scalar};

        let value = 300u32;
        let mut buffer = Vec::new();
        encode_varint_scalar(&mut buffer, value).unwrap();

        let decoded = decode_varint_scalar(buffer.as_slice()).unwrap();
        assert_eq!(decoded, value);
    }

    // -------------------------------------------------------------------------
    // SIMD CORRECTNESS TESTS
    // -------------------------------------------------------------------------

    #[test]
    fn test_simd_matches_scalar_dot_product() {
        use crate::hnsw::simd::{dot_product, dot_product_scalar};

        let test_cases = vec![
            (vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]),
            (
                (0..100).map(|i| i as f32).collect(),
                (100..200).map(|i| i as f32).collect(),
            ),
            (
                (0..1000).map(|i| i as f32).collect(),
                (1000..2000).map(|i| i as f32).collect(),
            ),
        ];

        for (a, b) in test_cases {
            let scalar = dot_product_scalar(&a, &b);
            let simd = dot_product(&a, &b);

            // Use relative error tolerance for large vectors due to floating-point
            // accumulation order differences in SIMD horizontal sum
            let abs_diff = (scalar - simd).abs();
            let rel_error = if scalar.abs() > f32::EPSILON {
                abs_diff / scalar.abs()
            } else {
                abs_diff
            };

            assert!(
                rel_error < 1e-5 || abs_diff < f32::EPSILON * 100.0,
                "Dot product differs for size {}: scalar={}, simd={}, diff={}, rel_error={}",
                a.len(),
                scalar,
                simd,
                abs_diff,
                rel_error
            );
        }
    }

    #[test]
    fn test_simd_matches_scalar_euclidean() {
        use crate::hnsw::simd::{euclidean_distance, euclidean_distance_scalar};

        let a: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let b: Vec<f32> = (100..200).map(|i| i as f32).collect();

        let scalar = euclidean_distance_scalar(&a, &b);
        let simd = euclidean_distance(&a, &b);

        // Use relative tolerance for floating-point precision
        let abs_diff = (scalar - simd).abs();
        let rel_error = if scalar.abs() > f32::EPSILON {
            abs_diff / scalar.abs()
        } else {
            abs_diff
        };

        assert!(
            rel_error < 1e-5 || abs_diff < f32::EPSILON * 10.0,
            "Euclidean: scalar={}, simd={}, diff={}, rel_error={}",
            scalar,
            simd,
            abs_diff,
            rel_error
        );
    }

    #[test]
    fn test_simd_matches_scalar_cosine() {
        use crate::hnsw::simd::{cosine_similarity, cosine_similarity_scalar};

        // Avoid zero values which cause NaN in recip
        let a: Vec<f32> = (1..100).map(|i| (i as f32).recip()).collect();
        let b: Vec<f32> = (101..200).map(|i| (i as f32).recip()).collect();

        let scalar = cosine_similarity_scalar(&a, &b);
        let simd = cosine_similarity(&a, &b);

        // Allow more tolerance for sqrt operations in cosine
        let abs_diff = (scalar - simd).abs();
        let rel_error = if scalar.abs() > f32::EPSILON {
            abs_diff / scalar.abs()
        } else {
            abs_diff
        };

        assert!(
            rel_error < 1e-4 || abs_diff < f32::EPSILON * 100.0,
            "Cosine: scalar={}, simd={}, diff={}, rel_error={}",
            scalar,
            simd,
            abs_diff,
            rel_error
        );
    }

    #[test]
    fn test_simd_matches_scalar_norm_squared() {
        use crate::hnsw::simd::{compute_norm_squared, compute_norm_squared_scalar};

        let v: Vec<f32> = (1..=500).map(|i| i as f32 * 0.1).collect();

        let scalar = compute_norm_squared_scalar(&v);
        let simd = compute_norm_squared(&v);

        // Use relative tolerance for large sums
        let abs_diff = (scalar - simd).abs();
        let rel_error = if scalar.abs() > f32::EPSILON {
            abs_diff / scalar.abs()
        } else {
            abs_diff
        };

        assert!(
            rel_error < 1e-5 || abs_diff < 0.01,
            "Norm squared: scalar={}, simd={}, diff={}, rel_error={}",
            scalar,
            simd,
            abs_diff,
            rel_error
        );
    }

    #[test]
    fn test_simd_correctness_edge_cases() {
        use crate::hnsw::simd::{dot_product, euclidean_distance};

        // Test with small vectors
        let a = vec![1.0, 2.0];
        let b = vec![3.0, 4.0];

        let dot = dot_product(&a, &b);
        assert!((dot - 11.0).abs() < f32::EPSILON);

        let dist = euclidean_distance(&a, &b);
        assert!((dist - 2.828_427).abs() < 0.0001);

        // Test with non-aligned sizes
        let c: Vec<f32> = (1..=17).map(|i| i as f32).collect();
        let d: Vec<f32> = (18..=34).map(|i| i as f32).collect();

        let dot2 = dot_product(&c, &d);
        let expected: f32 = c.iter().zip(d.iter()).map(|(x, y)| x * y).sum();
        assert!((dot2 - expected).abs() < f32::EPSILON * 10.0);
    }

    #[test]
    fn test_batch_filter_correctness() {
        use crate::hnsw::batch_filter::{filter_allowed_scalar, filter_batch};

        let ids: Vec<u64> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let allowed: Vec<u64> = vec![2, 4, 6, 8, 10];

        let scalar_result = filter_allowed_scalar(&ids, &allowed);
        let simd_result = filter_batch(&ids, &allowed, true);

        assert_eq!(scalar_result, simd_result);
        assert_eq!(simd_result, vec![2, 4, 6, 8, 10]);
    }

    #[test]
    fn test_delta_encode_correctness() {
        use crate::hnsw::serialization::{delta_decode, delta_encode, delta_encode_scalar};

        let values: Vec<u32> = vec![100, 105, 110, 115, 120, 125];

        let scalar_deltas = delta_encode_scalar(&values);
        let simd_deltas = delta_encode(&values);

        assert_eq!(scalar_deltas, simd_deltas);
        assert_eq!(simd_deltas, vec![100, 5, 5, 5, 5, 5]);

        // Verify round-trip
        let restored = delta_decode(&simd_deltas);
        assert_eq!(restored, values);
    }

    #[test]
    fn test_varint_encoding_round_trip() {
        use crate::hnsw::serialization::{decode_varint_scalar, encode_varint_scalar};

        let test_values = vec![0u32, 1, 127, 128, 300, 16383, 16384, u32::MAX];

        for value in test_values {
            let mut buffer = Vec::new();
            encode_varint_scalar(&mut buffer, value).unwrap();

            let decoded = decode_varint_scalar(buffer.as_slice()).unwrap();
            assert_eq!(decoded, value, "Failed to round-trip value {}", value);
        }
    }
}

// V3 VectorStorage implementation tests
#[cfg(all(test, feature = "native-v3"))]
mod v3_storage_tests;
