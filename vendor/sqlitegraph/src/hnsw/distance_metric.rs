//! Distance Metrics for Vector Similarity
//!
//! This module defines various distance metrics supported by the HNSW index for
//! calculating vector similarity. Different distance metrics are appropriate for
//! different types of vector data and use cases. The choice affects both
//! search quality and performance.
//!
//! # Supported Metrics
//!
//! - **Cosine Similarity**: Ideal for normalized vectors and text embeddings
//! - **Euclidean Distance**: L2 distance, suitable for general-purpose similarity
//! - **Dot Product**: Fast approximate cosine similarity for normalized vectors
//! - **Manhattan Distance**: L1 distance, robust to outliers
//!
//! # Performance Characteristics
//!
//! | Metric         | SIMD Support | Typical Use Case           | Normalization Required |
//! |----------------|-------------|----------------------------|-----------------------|
//! | Cosine         | Yes         | Text embeddings           | Recommended           |
//! | Euclidean      | Yes         | General similarity        | Optional              |
//! | DotProduct     | Yes         | Fast approximate cosine   | Required              |
//! | Manhattan      | Yes         | Robust similarity         | Optional              |
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::distance_metric::{DistanceMetric, compute_distance};
//!
//! let a = vec![1.0, 0.0, 0.0];
//! let b = vec![0.0, 1.0, 0.0];
//!
//! let distance = compute_distance(DistanceMetric::Cosine, &a, &b);
//! assert!(distance > 0.0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

/// Distance metrics supported by HNSW index
///
/// Different distance metrics are appropriate for different types of vector data
/// and use cases. The choice affects both search quality and performance.
///
/// # Variants
///
/// * `Cosine` - Cosine similarity, ideal for normalized vectors and text embeddings
/// * `Euclidean` - L2 distance, suitable for general-purpose similarity
/// * `DotProduct` - Raw dot product, fast for normalized vectors
/// * `Manhattan` - L1 distance, robust to outliers
///
/// # Performance Characteristics
///
/// | Metric         | SIMD Support | Typical Use Case           | Normalization Required |
/// |----------------|-------------|----------------------------|-----------------------|
/// | Cosine         | Yes         | Text embeddings           | Recommended           |
/// | Euclidean      | Yes         | General similarity        | Optional              |
/// | DotProduct     | Yes         | Fast approximate cosine   | Required              |
/// | Manhattan      | Yes         | Robust similarity         | Optional              |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DistanceMetric {
    /// Cosine similarity (1 - normalized dot product)
    /// Range: [0, 2] where 0 = identical, 2 = opposite
    #[default]
    Cosine,

    /// Euclidean (L2) distance
    /// Range: [0, ∞) where 0 = identical
    Euclidean,

    /// Dot product similarity
    /// Range: (-∞, ∞), higher values indicate greater similarity
    DotProduct,

    /// Manhattan (L1) distance
    /// Range: [0, ∞) where 0 = identical
    Manhattan,
}

impl DistanceMetric {
    /// Get the string representation of this distance metric
    pub fn as_str(&self) -> &str {
        match self {
            DistanceMetric::Cosine => "cosine",
            DistanceMetric::Euclidean => "euclidean",
            DistanceMetric::DotProduct => "dot_product",
            DistanceMetric::Manhattan => "manhattan",
        }
    }
}

impl fmt::Display for DistanceMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DistanceMetric::Cosine => write!(f, "cosine"),
            DistanceMetric::Euclidean => write!(f, "euclidean"),
            DistanceMetric::DotProduct => write!(f, "dot_product"),
            DistanceMetric::Manhattan => write!(f, "manhattan"),
        }
    }
}

/// Generic distance computation based on metric type
///
/// This function computes distance between two vectors based on the specified
/// distance metric. For similarity metrics (like cosine), it converts to
/// distance by taking the complement.
///
/// # Arguments
///
/// * `metric` - Distance metric to use
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Distance value where lower values indicate greater similarity
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::distance_metric::{DistanceMetric, compute_distance};
///
/// let a = [1.0, 0.0, 0.0];
/// let b = [0.0, 1.0, 0.0];
///
/// let cosine_dist = compute_distance(DistanceMetric::Cosine, &a, &b);
/// let euclidean_dist = compute_distance(DistanceMetric::Euclidean, &a, &b);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn compute_distance(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        DistanceMetric::Cosine => {
            // Convert cosine similarity to distance: (1 - similarity) / 2
            // This gives range [0, 1] where 0 = identical
            let similarity = cosine_similarity(a, b);
            (1.0 - similarity) / 2.0
        }
        DistanceMetric::Euclidean => euclidean_distance(a, b),
        DistanceMetric::DotProduct => {
            // Convert dot product to distance: -dot_product
            // This assumes higher dot products indicate greater similarity
            -dot_product(a, b)
        }
        DistanceMetric::Manhattan => manhattan_distance(a, b),
    }
}

// Re-export distance functions for backward compatibility
pub use crate::hnsw::distance_functions::{
    cosine_similarity, dot_product, euclidean_distance, manhattan_distance,
};

/// Reject a vector the distance kernel cannot handle for `metric`.
///
/// Cosine similarity divides by each vector's L2 norm, so it is undefined for a
/// zero-magnitude vector or any vector containing a non-finite (NaN/Inf)
/// component. The SIMD cosine kernel panics on such inputs ("First vector has
/// zero magnitude", see `simd::cosine_similarity` `# Panics`); a panic is not
/// catchable by caller `Result` handling, so one degenerate vector used to abort
/// an entire batch insert. This validates the vector at the public API boundary
/// instead, returning a typed error the caller can skip.
///
/// The other metrics (`Euclidean` / `Manhattan` / `DotProduct`) never divide by
/// the norm, so the origin vector is valid for them and this returns `Ok`.
pub(crate) fn validate_vector_for_metric(
    metric: DistanceMetric,
    vector: &[f32],
) -> Result<(), crate::hnsw::errors::HnswIndexError> {
    if !matches!(metric, DistanceMetric::Cosine) {
        return Ok(());
    }
    let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return Err(crate::hnsw::errors::HnswIndexError::ZeroMagnitudeVector);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance_metric_display() {
        assert_eq!(DistanceMetric::Cosine.to_string(), "cosine");
        assert_eq!(DistanceMetric::Euclidean.to_string(), "euclidean");
        assert_eq!(DistanceMetric::DotProduct.to_string(), "dot_product");
        assert_eq!(DistanceMetric::Manhattan.to_string(), "manhattan");
    }

    #[test]
    fn test_distance_metric_default() {
        let metric = DistanceMetric::default();
        assert_eq!(metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_distance_metric_equality() {
        assert_eq!(DistanceMetric::Cosine, DistanceMetric::Cosine);
        assert_ne!(DistanceMetric::Cosine, DistanceMetric::Euclidean);
        assert_ne!(DistanceMetric::Euclidean, DistanceMetric::Manhattan);
    }

    #[test]
    fn test_compute_distance_cosine() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        let distance = compute_distance(DistanceMetric::Cosine, &a, &b);
        assert_eq!(distance, 0.5); // (1 - 0) / 2
    }

    #[test]
    fn test_compute_distance_euclidean() {
        let a = [0.0, 0.0];
        let b = [3.0, 4.0];
        let distance = compute_distance(DistanceMetric::Euclidean, &a, &b);
        assert_eq!(distance, 5.0);
    }

    #[test]
    fn test_compute_distance_dot_product() {
        let a = [1.0, 0.0];
        let b = [1.0, 0.0];
        let distance = compute_distance(DistanceMetric::DotProduct, &a, &b);
        assert_eq!(distance, -1.0); // -dot_product
    }

    #[test]
    fn test_compute_distance_manhattan() {
        let a = [1.0, 2.0];
        let b = [4.0, 0.0];
        let distance = compute_distance(DistanceMetric::Manhattan, &a, &b);
        assert_eq!(distance, 5.0); // |1-4| + |2-0|
    }

    #[test]
    fn test_all_metrics_identical_vectors() {
        let a = [1.0, 0.0];
        let b = [1.0, 0.0];

        let cosine_dist = compute_distance(DistanceMetric::Cosine, &a, &b);
        let euclidean_dist = compute_distance(DistanceMetric::Euclidean, &a, &b);
        let dot_dist = compute_distance(DistanceMetric::DotProduct, &a, &b);
        let manhattan_dist = compute_distance(DistanceMetric::Manhattan, &a, &b);

        assert_eq!(cosine_dist, 0.0);
        assert_eq!(euclidean_dist, 0.0);
        assert_eq!(manhattan_dist, 0.0);
        assert_eq!(dot_dist, -1.0);
    }

    // --- validate_vector_for_metric -------------------------------------
    //
    // Focused unit coverage of the degenerate-vector guard. The integration
    // behavior (guard fires at the HNSW insert/search boundary) is covered in
    // index_api_tests; these tests pin the validation logic itself so a change
    // here cannot silently pass the boundary tests.

    #[test]
    fn test_validate_cosine_rejects_zero_vector() {
        let zero = [0.0, 0.0, 0.0];
        assert_eq!(
            validate_vector_for_metric(DistanceMetric::Cosine, &zero),
            Err(crate::hnsw::errors::HnswIndexError::ZeroMagnitudeVector)
        );
    }

    #[test]
    fn test_validate_cosine_rejects_non_finite() {
        // NaN / Inf make the norm non-finite and are rejected by the same check.
        for bad in [
            [f32::NAN, 1.0, 0.0],
            [1.0, f32::INFINITY, 0.0],
            [f32::NEG_INFINITY, 0.0, 0.0],
        ] {
            assert_eq!(
                validate_vector_for_metric(DistanceMetric::Cosine, &bad),
                Err(crate::hnsw::errors::HnswIndexError::ZeroMagnitudeVector),
                "cosine must reject {bad:?}"
            );
        }
    }

    #[test]
    fn test_validate_cosine_accepts_unit_and_tiny_vectors() {
        // A valid unit vector is accepted.
        assert!(validate_vector_for_metric(DistanceMetric::Cosine, &[1.0, 0.0, 0.0]).is_ok());
        // A sub-EPSILON-magnitude vector is rejected (norm <= f32::EPSILON).
        let tiny = [f32::EPSILON, 0.0, 0.0]; // norm == EPSILON, not strictly >
        assert_eq!(
            validate_vector_for_metric(DistanceMetric::Cosine, &tiny),
            Err(crate::hnsw::errors::HnswIndexError::ZeroMagnitudeVector)
        );
        // Just-above-threshold is accepted.
        let ok = [2.0 * f32::EPSILON, 0.0, 0.0];
        assert!(validate_vector_for_metric(DistanceMetric::Cosine, &ok).is_ok());
    }

    #[test]
    fn test_validate_non_cosine_accepts_origin() {
        // Euclidean / Manhattan / DotProduct never divide by the norm, so the
        // origin vector is valid for them.
        let zero = [0.0, 0.0, 0.0];
        for metric in [
            DistanceMetric::Euclidean,
            DistanceMetric::Manhattan,
            DistanceMetric::DotProduct,
        ] {
            assert!(
                validate_vector_for_metric(metric, &zero).is_ok(),
                "{metric:?} should accept a zero-magnitude vector"
            );
        }
    }
}
