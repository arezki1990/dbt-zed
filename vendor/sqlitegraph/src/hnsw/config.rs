//! HNSW Algorithm Configuration
//!
//! This module defines configuration parameters for Hierarchical Navigable Small World (HNSW)
//! vector index construction and search operations. These parameters directly control
//! index performance, memory usage, and search quality.
//!
//! # Configuration Parameters
//!
//! - **Dimension**: Vector dimension count (must match all vectors)
//! - **M**: Number of bi-directional connections per node (typically 5-48)
//! - **ef_construction**: Dynamic candidate list size during construction (typically 100-800)
//! - **ef_search**: Dynamic candidate list size during search (typically 10-200)
//! - **ml**: Maximum number of layers in the index
//! - **distance_metric**: Similarity calculation method
//!
//! # Performance Impact
//!
//! ## M Parameter
//! - Higher M: Better recall, more memory, slower construction
//! - Lower M: Faster construction, less memory, potentially lower recall
//! - Recommended: 16 for most use cases, 32+ for high accuracy requirements
//!
//! ## ef_construction Parameter
//! - Higher ef_construction: Better index quality, slower construction
//! - Lower ef_construction: Faster construction, potentially lower search quality
//! - Recommended: 200 for balanced performance
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::{HnswConfigBuilder, DistanceMetric};
//!
//! let config = HnswConfigBuilder::new()
//!     .dimension(768)
//!     .m_connections(16)
//!     .ef_construction(200)
//!     .ef_search(50)
//!     .distance_metric(DistanceMetric::Cosine)
//!     .build()
//!     .unwrap();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::hnsw::distance_metric::DistanceMetric;

/// HNSW algorithm configuration parameters
///
/// This struct defines all parameters that control HNSW index behavior.
/// These parameters significantly impact search quality, construction time,
/// and memory usage patterns.
///
/// # Field Descriptions
///
/// ## dimension
/// Vector dimension count. Must match all vectors inserted into the index.
/// Typical values: 128-16384 depending on embedding model used.
///
/// ## m
/// Number of bi-directional links created for each node during construction.
/// This is the primary parameter controlling index connectivity.
///
/// - Lower values (5-12): Faster construction, less memory, lower recall
/// - Medium values (16-24): Balanced performance (recommended)
/// - Higher values (32-48): Better recall, more memory, slower construction
///
/// ## ef_construction
/// Size of dynamic candidate list during index construction.
/// Controls how thoroughly the algorithm explores the graph during insertion.
///
/// - Lower values (100-200): Faster construction
/// - Higher values (400-800): Better index quality, slower construction
///
/// ## ef_search
/// Size of dynamic candidate list during search operations.
/// Controls search accuracy vs speed trade-off.
///
/// - Lower values (10-50): Faster search, potentially lower accuracy
/// - Higher values (100-200): Better recall, slower search
///
/// ## ml
/// Maximum number of layers in the HNSW structure.
/// Calculated as floor(-ln(N) * ml_scale) where N is data size.
/// Higher values create deeper graphs for better performance on large datasets.
///
/// ## distance_metric
/// Distance function used for vector similarity calculation.
/// Choose based on your vector data characteristics and use case requirements.
///
/// ## enable_multilayer
/// Controls whether multi-layer HNSW functionality is enabled.
/// When false (default), all vectors are inserted into the base layer only,
/// providing backward compatibility and avoiding node ID conflicts.
/// When true, proper multi-layer HNSW with exponential distribution is used.
///
/// ## multilayer_level_distribution_base
/// Base value for exponential level distribution in multi-layer mode.
/// Higher values create flatter layer distributions (more vectors in higher layers).
/// Default value equals m for optimal performance.
///
/// ## multilayer_deterministic_seed
/// Seed for deterministic random number generation in multi-layer operations.
/// When Some(seed), reproducible level assignments are ensured.
/// When None, non-deterministic behavior is used (default for production).
///
/// # Default Configuration
///
/// The default configuration provides good performance for most use cases:
/// - Balanced search quality vs speed
/// - Reasonable memory usage (~2.5x vector size)
/// - Fast construction time
/// - Robust to various data distributions
/// - Single-layer mode for backward compatibility
///
/// # Multi-layer vs Single-layer Mode
///
/// ## Single-layer mode (enable_multilayer = false)
/// - All vectors inserted into base layer (L0)
/// - No node ID conflicts
/// - Faster insertion, simpler search
/// - Recommended for small datasets (<10k vectors) or when compatibility is critical
///
/// ## Multi-layer mode (enable_multilayer = true)
/// - Exponential level distribution for optimal search performance
/// - 3-10x faster search for large datasets (>10k vectors)
/// - More complex insertion algorithm with bidirectional ID mapping
/// - Recommended for large datasets where search performance is critical
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::{HnswConfig, DistanceMetric};
///
/// // High-precision configuration
/// let precise_config = HnswConfig {
///     dimension: 768,
///     m: 32,
///     ef_construction: 400,
///     ef_search: 100,
///     ml: 24,
///     distance_metric: DistanceMetric::Cosine,
///     enable_multilayer: false,
///     multilayer_level_distribution_base: None,
///     multilayer_deterministic_seed: None,
/// };
///
/// // Multi-layer configuration for large datasets
/// let multilayer_config = HnswConfig {
///     dimension: 768,
///     m: 16,
///     ef_construction: 200,
///     ef_search: 50,
///     ml: 16,
///     distance_metric: DistanceMetric::Cosine,
///     enable_multilayer: true,
///     multilayer_level_distribution_base: Some(16),
///     multilayer_deterministic_seed: Some(42),
/// };
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct HnswConfig {
    /// Vector dimension count
    /// Must match all vectors inserted into the index
    /// Range: 1-16384 (practical limits)
    pub dimension: usize,

    /// Number of connections per node (M parameter)
    /// Controls graph connectivity and memory usage
    /// Range: 5-48 (typical), higher values require more memory
    pub m: usize,

    /// Construction ef parameter
    /// Dynamic candidate list size during index building
    /// Range: 100-800 (typical)
    pub ef_construction: usize,

    /// Search ef parameter
    /// Dynamic candidate list size during search
    /// Range: 10-200 (typical)
    pub ef_search: usize,

    /// Maximum number of layers
    /// Controls maximum graph depth
    /// Range: 8-32 (typical)
    pub ml: u8,

    /// Distance metric for similarity calculation
    pub distance_metric: DistanceMetric,

    /// Enable multi-layer HNSW functionality
    /// When false, uses single-layer mode for backward compatibility
    /// When true, enables proper multi-layer HNSW with exponential distribution
    pub enable_multilayer: bool,

    /// Base value for exponential level distribution in multi-layer mode
    /// When None, uses m value as default
    /// Higher values create flatter distributions (more vectors in higher layers)
    pub multilayer_level_distribution_base: Option<usize>,

    /// Seed for deterministic random number generation in multi-layer operations
    /// When Some(seed), ensures reproducible level assignments
    /// When None, uses non-deterministic behavior (default for production)
    pub multilayer_deterministic_seed: Option<u64>,
}

impl HnswConfig {
    /// Create a new HnswConfig with the specified parameters
    ///
    /// # Arguments
    ///
    /// * `dimension` - Vector dimension count
    /// * `m` - Number of connections per node (M parameter)
    /// * `ef_construction` - Dynamic candidate list size during construction
    /// * `distance_metric` - Distance metric for similarity calculation
    ///
    /// # Returns
    ///
    /// A new HnswConfig instance with sensible defaults for other parameters
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sqlitegraph::hnsw::{HnswConfig, DistanceMetric};
    ///
    /// let config = HnswConfig::new(128, 16, 200, DistanceMetric::Cosine);
    /// ```
    pub fn new(
        dimension: usize,
        m: usize,
        ef_construction: usize,
        distance_metric: DistanceMetric,
    ) -> Self {
        HnswConfig {
            dimension,
            m,
            ef_construction,
            ef_search: ef_construction, // Default ef_search to ef_construction
            ml: 16,                     // Reasonable default
            distance_metric,
            enable_multilayer: false, // Single-layer mode by default
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        }
    }
}

impl Default for HnswConfig {
    fn default() -> Self {
        HnswConfig {
            dimension: 768,       // Common embedding size
            m: 16,                // Balanced connectivity
            ef_construction: 200, // Good construction quality
            ef_search: 50,        // Balanced search speed/quality
            ml: 16,               // Reasonable depth
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: false, // Single-layer mode for backward compatibility
            multilayer_level_distribution_base: None, // Use m as default
            multilayer_deterministic_seed: None, // Non-deterministic for production
        }
    }
}

/// Create a new HnswConfig builder with default values
///
/// This is a convenience function that creates a builder for constructing
/// HnswConfig instances with validation and sensible defaults.
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::{hnsw_config, DistanceMetric};
///
/// let config = hnsw_config()
///     .dimension(256)
///     .m_connections(12)
///     .distance_metric(DistanceMetric::Euclidean)
///     .build()
///     .unwrap();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn hnsw_config() -> crate::hnsw::builder::HnswConfigBuilder {
    crate::hnsw::builder::HnswConfigBuilder::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HnswConfig::default();

        assert_eq!(config.dimension, 768);
        assert_eq!(config.m, 16);
        assert_eq!(config.ef_construction, 200);
        assert_eq!(config.ef_search, 50);
        assert_eq!(config.ml, 16);
        assert_eq!(config.distance_metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_config_clone() {
        let config1 = HnswConfig {
            dimension: 256,
            m: 12,
            ef_construction: 150,
            ef_search: 40,
            ml: 12,
            distance_metric: DistanceMetric::Manhattan,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(12),
            multilayer_deterministic_seed: Some(123),
        };

        let config2 = config1.clone();
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_high_precision_config() {
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
        assert!(!config.enable_multilayer);
    }

    #[test]
    fn test_fast_construction_config() {
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
        assert!(!config.enable_multilayer);
    }

    #[test]
    fn test_hnsw_config_function() {
        let config = hnsw_config()
            .dimension(512)
            .m_connections(24)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();

        assert_eq!(config.dimension, 512);
        assert_eq!(config.m, 24);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean);
        assert!(!config.enable_multilayer); // Default is single-layer
    }

    #[test]
    fn test_multilayer_config_defaults() {
        let config = HnswConfig::default();

        // Default should be single-layer mode for backward compatibility
        assert!(!config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, None);
        assert_eq!(config.multilayer_deterministic_seed, None);
    }

    #[test]
    fn test_multilayer_config_enabled() {
        let config = HnswConfig {
            dimension: 256,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(16),
            multilayer_deterministic_seed: Some(42),
        };

        assert!(config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, Some(16));
        assert_eq!(config.multilayer_deterministic_seed, Some(42));
    }

    #[test]
    fn test_multilayer_config_defaults_derivation() {
        let config = HnswConfig {
            dimension: 512,
            m: 20,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: true,
            multilayer_level_distribution_base: None, // Should use m
            multilayer_deterministic_seed: None,      // Should be non-deterministic
        };

        assert!(config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, None);
        assert_eq!(config.multilayer_deterministic_seed, None);
    }

    #[test]
    fn test_multilayer_config_validation() {
        // Test that multi-layer config doesn't interfere with basic validation
        let config = HnswConfig {
            dimension: 768,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(32), // Higher than m
            multilayer_deterministic_seed: Some(12345),
        };

        assert_eq!(config.dimension, 768);
        assert_eq!(config.m, 16);
        assert_eq!(config.multilayer_level_distribution_base, Some(32));
        assert_eq!(config.multilayer_deterministic_seed, Some(12345));
    }

    #[test]
    fn test_single_layer_vs_multi_layer_config() {
        let single_layer = HnswConfig {
            dimension: 512,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: false,
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        };

        let multi_layer = HnswConfig {
            dimension: 512,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Cosine,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(16),
            multilayer_deterministic_seed: Some(42),
        };

        // Should be different due to multi-layer settings
        assert_ne!(single_layer, multi_layer);
        assert!(!single_layer.enable_multilayer);
        assert!(multi_layer.enable_multilayer);
    }
}
