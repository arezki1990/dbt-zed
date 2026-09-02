//! HNSW Configuration Builder
//!
//! This module provides a fluent builder pattern for constructing HnswConfig
//! instances with validation and sensible defaults. The builder ensures
//! that all configuration parameters are valid before creating the final
//! configuration.
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::{HnswConfigBuilder, DistanceMetric};
//!
//! // Single-layer configuration (default, backward compatible)
//! let config = HnswConfigBuilder::new()
//!     .dimension(512)
//!     .m_connections(24)
//!     .ef_construction(300)
//!     .ef_search(80)
//!     .max_layers(20)
//!     .distance_metric(DistanceMetric::Euclidean)
//!     .build()
//!     .unwrap();
//!
//! // Multi-layer configuration for large datasets
//! let multilayer_config = HnswConfigBuilder::new()
//!     .dimension(768)
//!     .m_connections(16)
//!     .ef_construction(200)
//!     .ef_search(50)
//!     .max_layers(16)
//!     .distance_metric(DistanceMetric::Cosine)
//!     .enable_multilayer(true)
//!     .multilayer_deterministic_seed(Some(42))
//!     .build()
//!     .unwrap();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::hnsw::config::HnswConfig;
use crate::hnsw::distance_metric::DistanceMetric;
use crate::hnsw::errors::HnswConfigError;

/// Builder pattern for HnswConfig construction
///
/// Provides a fluent interface for creating HnswConfig instances
/// with validation and sensible defaults.
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::{HnswConfigBuilder, DistanceMetric};
///
/// // Single-layer configuration (default, backward compatible)
/// let config = HnswConfigBuilder::new()
///     .dimension(512)
///     .m_connections(24)
///     .ef_construction(300)
///     .ef_search(80)
///     .max_layers(20)
///     .distance_metric(DistanceMetric::Euclidean)
///     .build()
///     .unwrap();
///
/// // Multi-layer configuration for large datasets
/// let multilayer_config = HnswConfigBuilder::new()
///     .dimension(768)
///     .m_connections(16)
///     .ef_construction(200)
///     .ef_search(50)
///     .max_layers(16)
///     .distance_metric(DistanceMetric::Cosine)
///     .enable_multilayer(true)
///     .multilayer_level_distribution_base(Some(16))
///     .multilayer_deterministic_seed(Some(42))
///     .build()
///     .unwrap();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct HnswConfigBuilder {
    pub config: HnswConfig,
}

impl HnswConfigBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: HnswConfig::default(),
        }
    }

    /// Set vector dimension count
    ///
    /// # Arguments
    /// * `dimension` - Number of dimensions in vectors (1-16384)
    ///
    /// # Panics
    ///
    /// Panics if dimension is 0 or exceeds practical limits (>16384)
    pub fn dimension(mut self, dimension: usize) -> Self {
        assert!(dimension > 0, "Dimension must be greater than 0");
        assert!(
            dimension <= 16384,
            "Dimension exceeds practical limit of 16384"
        );
        self.config.dimension = dimension;
        self
    }

    /// Set number of connections per node (M parameter)
    ///
    /// # Arguments
    /// * `m` - Number of connections (5-48 typical)
    ///
    /// # Panics
    ///
    /// Panics if m is 0 or exceeds practical limits (>48)
    pub fn m_connections(mut self, m: usize) -> Self {
        assert!(m > 0, "M must be greater than 0");
        assert!(m <= 48, "M exceeds practical limit of 48");
        self.config.m = m;
        self
    }

    /// Set construction ef parameter
    ///
    /// # Arguments
    /// * `ef` - Construction ef value (100-800 typical)
    ///
    /// # Panics
    ///
    /// Panics if ef is less than m (must be >= M for proper construction)
    pub fn ef_construction(mut self, ef: usize) -> Self {
        assert!(ef >= self.config.m, "ef_construction must be >= M");
        assert!(ef <= 800, "ef_construction exceeds practical limit of 800");
        self.config.ef_construction = ef;
        self
    }

    /// Set search ef parameter
    ///
    /// # Arguments
    /// * `ef` - Search ef value (10-200 typical)
    ///
    /// # Panics
    ///
    /// Panics if ef is 0 or exceeds practical limits (>200)
    pub fn ef_search(mut self, ef: usize) -> Self {
        assert!(ef > 0, "ef_search must be greater than 0");
        assert!(ef <= 200, "ef_search exceeds practical limit of 200");
        self.config.ef_search = ef;
        self
    }

    /// Set maximum number of layers
    ///
    /// # Arguments
    /// * `ml` - Maximum layer count (8-32 typical)
    ///
    /// # Panics
    ///
    /// Panics if ml is 0 or exceeds practical limits (>32)
    pub fn max_layers(mut self, ml: u8) -> Self {
        assert!(ml > 0, "ml must be greater than 0");
        assert!(ml <= 32, "ml exceeds practical limit of 32");
        self.config.ml = ml;
        self
    }

    /// Set distance metric
    ///
    /// # Arguments
    /// * `metric` - Distance metric to use
    pub fn distance_metric(mut self, metric: DistanceMetric) -> Self {
        self.config.distance_metric = metric;
        self
    }

    /// Enable multi-layer HNSW functionality
    ///
    /// When enabled, uses proper multi-layer HNSW with exponential distribution.
    /// When disabled, uses single-layer mode for backward compatibility.
    ///
    /// # Arguments
    /// * `enable` - Whether to enable multi-layer functionality
    pub fn enable_multilayer(mut self, enable: bool) -> Self {
        self.config.enable_multilayer = enable;
        self
    }

    /// Set base value for exponential level distribution in multi-layer mode
    ///
    /// Higher values create flatter layer distributions (more vectors in higher layers).
    /// When None, uses the m parameter value as default.
    ///
    /// # Arguments
    /// * `base` - Base value for level distribution, or None to use m
    pub fn multilayer_level_distribution_base(mut self, base: Option<usize>) -> Self {
        self.config.multilayer_level_distribution_base = base;
        self
    }

    /// Set deterministic seed for multi-layer operations
    ///
    /// When provided, ensures reproducible level assignments across runs.
    /// When None, uses non-deterministic behavior (default for production).
    ///
    /// # Arguments
    /// * `seed` - Seed value, or None for non-deterministic behavior
    pub fn multilayer_deterministic_seed(mut self, seed: Option<u64>) -> Self {
        self.config.multilayer_deterministic_seed = seed;
        self
    }

    /// Build the HnswConfig with validation
    ///
    /// # Returns
    ///
    /// Returns `Ok(HnswConfig)` if all parameters are valid,
    /// or `Err(HnswConfigError)` if validation fails.
    ///
    /// # Errors
    ///
    /// - `InvalidDimension` if dimension is 0
    /// - `InvalidMParameter` if m is 0
    /// - `InvalidEfConstruction` if ef_construction < m
    /// - `InvalidEfSearch` if ef_search is 0
    /// - `InvalidMaxLayers` if ml is 0
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sqlitegraph::hnsw::{HnswConfigBuilder, DistanceMetric};
    ///
    /// let config = HnswConfigBuilder::new()
    ///     .dimension(256)
    ///     .distance_metric(DistanceMetric::Cosine)
    ///     .build()
    ///     .unwrap();
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(self) -> Result<HnswConfig, HnswConfigError> {
        // Final validation
        if self.config.dimension == 0 {
            return Err(HnswConfigError::InvalidDimension);
        }
        if self.config.m == 0 {
            return Err(HnswConfigError::InvalidMParameter);
        }
        if self.config.ef_construction < self.config.m {
            return Err(HnswConfigError::InvalidEfConstruction);
        }
        if self.config.ef_search == 0 {
            return Err(HnswConfigError::InvalidEfSearch);
        }
        if self.config.ml == 0 {
            return Err(HnswConfigError::InvalidMaxLayers);
        }

        Ok(self.config)
    }
}

impl Default for HnswConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let config = HnswConfigBuilder::new()
            .dimension(512)
            .m_connections(24)
            .ef_construction(300)
            .ef_search(80)
            .max_layers(20)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();

        assert_eq!(config.dimension, 512);
        assert_eq!(config.m, 24);
        assert_eq!(config.ef_construction, 300);
        assert_eq!(config.ef_search, 80);
        assert_eq!(config.ml, 20);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean);
    }

    #[test]
    #[should_panic(expected = "Dimension must be greater than 0")]
    fn test_builder_validation_dimension_zero() {
        HnswConfigBuilder::new().dimension(0);
    }

    #[test]
    #[should_panic(expected = "M must be greater than 0")]
    fn test_builder_validation_m_zero() {
        HnswConfigBuilder::new().m_connections(0);
    }

    #[test]
    #[should_panic(expected = "ef_construction must be >= M")]
    fn test_builder_validation_ef_construction_less_than_m() {
        HnswConfigBuilder::new()
            .m_connections(20)
            .ef_construction(10);
    }

    #[test]
    #[should_panic(expected = "ef_search must be greater than 0")]
    fn test_builder_validation_ef_search_zero() {
        HnswConfigBuilder::new().ef_search(0);
    }

    #[test]
    #[should_panic(expected = "ml must be greater than 0")]
    fn test_builder_validation_ml_zero() {
        HnswConfigBuilder::new().max_layers(0);
    }

    #[test]
    fn test_builder_all_distance_metrics() {
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ];

        for metric in metrics {
            let config = HnswConfigBuilder::new()
                .distance_metric(metric)
                .build()
                .unwrap();
            assert_eq!(config.distance_metric, metric);
        }
    }

    #[test]
    fn test_builder_multilayer_methods() {
        // Test enabling multi-layer functionality
        let config = HnswConfigBuilder::new()
            .dimension(256)
            .enable_multilayer(true)
            .build()
            .unwrap();

        assert!(config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, None);
        assert_eq!(config.multilayer_deterministic_seed, None);
    }

    #[test]
    fn test_builder_multilayer_level_distribution_base() {
        let config = HnswConfigBuilder::new()
            .dimension(512)
            .m_connections(16)
            .multilayer_level_distribution_base(Some(20))
            .build()
            .unwrap();

        assert_eq!(config.multilayer_level_distribution_base, Some(20));
    }

    #[test]
    fn test_builder_multilayer_deterministic_seed() {
        let config = HnswConfigBuilder::new()
            .dimension(384)
            .multilayer_deterministic_seed(Some(12345))
            .build()
            .unwrap();

        assert_eq!(config.multilayer_deterministic_seed, Some(12345));
    }

    #[test]
    fn test_builder_multilayer_full_configuration() {
        let config = HnswConfigBuilder::new()
            .dimension(768)
            .m_connections(24)
            .ef_construction(400)
            .ef_search(100)
            .max_layers(20)
            .distance_metric(DistanceMetric::Cosine)
            .enable_multilayer(true)
            .multilayer_level_distribution_base(Some(24))
            .multilayer_deterministic_seed(Some(42))
            .build()
            .unwrap();

        assert_eq!(config.dimension, 768);
        assert_eq!(config.m, 24);
        assert_eq!(config.ef_construction, 400);
        assert_eq!(config.ef_search, 100);
        assert_eq!(config.ml, 20);
        assert_eq!(config.distance_metric, DistanceMetric::Cosine);
        assert!(config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, Some(24));
        assert_eq!(config.multilayer_deterministic_seed, Some(42));
    }

    #[test]
    fn test_builder_defaults_multilayer_disabled() {
        // Default configuration should have multi-layer disabled for backward compatibility
        let config = HnswConfigBuilder::new().dimension(512).build().unwrap();

        assert!(!config.enable_multilayer);
        assert_eq!(config.multilayer_level_distribution_base, None);
        assert_eq!(config.multilayer_deterministic_seed, None);
    }

    #[test]
    fn test_builder_multilayer_vs_single_layer() {
        let single_layer = HnswConfigBuilder::new()
            .dimension(512)
            .m_connections(16)
            .ef_construction(200)
            .ef_search(50)
            .max_layers(16)
            .distance_metric(DistanceMetric::Euclidean)
            .enable_multilayer(false)
            .build()
            .unwrap();

        let multi_layer = HnswConfigBuilder::new()
            .dimension(512)
            .m_connections(16)
            .ef_construction(200)
            .ef_search(50)
            .max_layers(16)
            .distance_metric(DistanceMetric::Euclidean)
            .enable_multilayer(true)
            .multilayer_level_distribution_base(Some(16))
            .multilayer_deterministic_seed(Some(42))
            .build()
            .unwrap();

        // Should be different due to multi-layer settings
        assert_ne!(single_layer, multi_layer);
        assert!(!single_layer.enable_multilayer);
        assert!(multi_layer.enable_multilayer);
        assert_eq!(single_layer.multilayer_deterministic_seed, None);
        assert_eq!(multi_layer.multilayer_deterministic_seed, Some(42));
    }

    #[test]
    fn test_builder_multilayer_level_distribution_base_none() {
        // Test explicit None value
        let config = HnswConfigBuilder::new()
            .dimension(256)
            .m_connections(12)
            .multilayer_level_distribution_base(None)
            .build()
            .unwrap();

        assert_eq!(config.multilayer_level_distribution_base, None);
    }
}
