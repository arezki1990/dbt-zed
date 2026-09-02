//! Distance Calculation Functions
//!
//! This module provides low-level distance calculation functions for HNSW.
//! These functions are optimized for performance using SIMD instructions (AVX2)
//! when available, with automatic runtime dispatch to scalar fallback on
//! non-AVX CPUs.
//!
//! # Functions
//!
//! - **cosine_similarity**: Cosine similarity between vectors (SIMD-accelerated)
//! - **euclidean_distance**: L2 distance calculation (SIMD-accelerated)
//! - **dot_product**: Raw dot product computation (SIMD-accelerated)
//! - **manhattan_distance**: L1 distance calculation
//!
//! # SIMD Acceleration
//!
//! All distance functions automatically use AVX2 SIMD instructions when available:
//! - **x86_64 with AVX2**: 4-6x speedup for large vectors
//! - **Other platforms**: Scalar fallback with optimized Rust code
//! - **Zero API changes**: Function signatures and results are identical
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::distance_functions::cosine_similarity;
//!
//! let a = vec![1.0, 0.0, 0.0];
//! let b = vec![0.0, 1.0, 0.0];
//! let similarity = cosine_similarity(&a, &b);
//! assert_eq!(similarity, 0.0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

// Use SIMD-accelerated implementations when available
pub use crate::hnsw::simd::cosine_similarity as simd_cosine_similarity;
pub use crate::hnsw::simd::euclidean_distance as simd_euclidean_distance;

/// Compute cosine similarity between two vectors
///
/// Cosine similarity measures the cosine of the angle between two vectors,
/// providing a value between -1 and 1 where 1 indicates identical direction.
///
/// This function uses SIMD-accelerated implementation (AVX2 on x86_64) with
/// automatic runtime dispatch to scalar fallback on non-AVX CPUs.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Cosine similarity value in range [-1, 1]
///
/// # Panics
///
/// Panics if vectors have different lengths, are empty, or contain zero magnitude
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
/// - SIMD Acceleration: 4-6x speedup on AVX2 hardware for large vectors
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::distance_functions::cosine_similarity;
///
/// let a = [1.0, 0.0, 0.0];
/// let b = [1.0, 0.0, 0.0];
/// let similarity = cosine_similarity(&a, &b);
/// assert!((similarity - 1.0).abs() < f32::EPSILON);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // Delegate to SIMD-accelerated implementation
    simd_cosine_similarity(a, b)
}

/// Compute Euclidean distance between two vectors
///
/// Euclidean distance (L2 norm) measures the straight-line distance between
/// two vectors in Euclidean space.
///
/// This function uses SIMD-accelerated implementation (AVX2 on x86_64) with
/// automatic runtime dispatch to scalar fallback on non-AVX CPUs.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Euclidean distance value >= 0
///
/// # Panics
///
/// Panics if vectors have different lengths
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
/// - SIMD Acceleration: ~8x speedup on AVX2 hardware for large vectors
/// - AVX2 processes 8 squared differences per iteration
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::distance_functions::euclidean_distance;
///
/// let a = [1.0, 0.0, 0.0];
/// let b = [0.0, 1.0, 0.0];
/// let distance = euclidean_distance(&a, &b);
/// assert!((distance - 1.41421356).abs() < f32::EPSILON);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    // Delegate to SIMD-accelerated implementation
    simd_euclidean_distance(a, b)
}

/// Compute dot product between two vectors
///
/// Dot product is the sum of element-wise products. For normalized vectors,
/// this is equivalent to cosine similarity multiplied by the magnitudes.
///
/// This function uses SIMD-accelerated implementation (AVX2 on x86_64) with
/// automatic runtime dispatch to scalar fallback on non-AVX CPUs.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Dot product value (can be positive, negative, or zero)
///
/// # Panics
///
/// Panics if vectors have different lengths
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
/// - SIMD Acceleration: 4-6x speedup on AVX2 hardware for large vectors
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::distance_functions::dot_product;
///
/// let a = [1.0, 2.0, 3.0];
/// let b = [4.0, 5.0, 6.0];
/// let product = dot_product(&a, &b);
/// assert_eq!(product, 32.0); // 1*4 + 2*5 + 3*6
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// This is a re-export of the SIMD-accelerated implementation from the
/// `simd` module. See [`crate::hnsw::simd::dot_product`] for details.
pub use crate::hnsw::simd::dot_product;

/// Compute Manhattan distance between two vectors
///
/// Manhattan distance (L1 norm) measures the sum of absolute differences
/// between corresponding elements of two vectors. It's more robust to outliers
/// than Euclidean distance.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Manhattan distance value >= 0
///
/// # Panics
///
/// Panics if vectors have different lengths
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
/// - Future: SIMD optimization planned
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::distance_functions::manhattan_distance;
///
/// let a = [1.0, 2.0, 3.0];
/// let b = [4.0, 0.0, 6.0];
/// let distance = manhattan_distance(&a, &b);
/// assert_eq!(distance, 8.0); // |1-4| + |2-0| + |3-6| = 3 + 2 + 3 = 8
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0, 3.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = [1.0, 0.0];
        let b = [-1.0, 0.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity + 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_euclidean_distance_identical() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0, 3.0];
        let distance = euclidean_distance(&a, &b);
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn test_euclidean_distance_unit() {
        let a = [0.0, 0.0];
        let b = [1.0, 0.0];
        let distance = euclidean_distance(&a, &b);
        assert_eq!(distance, 1.0);
    }

    #[test]
    fn test_dot_product_basic() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let product = dot_product(&a, &b);
        assert_eq!(product, 32.0); // 1*4 + 2*5 + 3*6
    }

    #[test]
    fn test_manhattan_distance_basic() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 0.0, 6.0];
        let distance = manhattan_distance(&a, &b);
        assert_eq!(distance, 8.0); // |1-4| + |2-0| + |3-6| = 3 + 2 + 3 = 8
    }

    #[test]
    #[should_panic(expected = "Vectors must have the same length")]
    fn test_different_lengths_panic() {
        let a = [1.0, 2.0];
        let b = [1.0, 2.0, 3.0];
        cosine_similarity(&a, &b);
    }

    #[test]
    #[should_panic(expected = "Vectors cannot be empty")]
    fn test_empty_vectors_panic() {
        let a: [f32; 0] = [];
        let b: [f32; 0] = [];
        cosine_similarity(&a, &b);
    }

    #[test]
    #[should_panic(expected = "First vector has zero magnitude")]
    fn test_zero_magnitude_panic() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        cosine_similarity(&a, &b);
    }

    #[test]
    fn test_all_metrics_identical_vectors() {
        let a = [1.0, 0.0];
        let b = [1.0, 0.0];

        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - 1.0).abs() < f32::EPSILON);

        let euclidean_dist = euclidean_distance(&a, &b);
        assert_eq!(euclidean_dist, 0.0);

        let dot = dot_product(&a, &b);
        assert_eq!(dot, 1.0);

        let manhattan_dist = manhattan_distance(&a, &b);
        assert_eq!(manhattan_dist, 0.0);
    }

    #[test]
    fn test_high_dimensional_vectors() {
        let a: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let b: Vec<f32> = (101..=200).map(|i| i as f32).collect();

        // Should not panic and should return reasonable values
        let _cosine = cosine_similarity(&a, &b);
        let _euclidean = euclidean_distance(&a, &b);
        let _dot = dot_product(&a, &b);
        let _manhattan = manhattan_distance(&a, &b);
    }

    #[test]
    fn test_negative_values() {
        let a = [-1.0, 2.0, -3.0];
        let b = [4.0, -5.0, 6.0];

        let cosine = cosine_similarity(&a, &b);
        assert!((cosine + 0.974631).abs() < 0.001); // -32 / sqrt(14*77)

        let euclidean = euclidean_distance(&a, &b);
        assert!((euclidean - 12.4499).abs() < 0.001);

        let dot = dot_product(&a, &b);
        assert_eq!(dot, -32.0);

        let manhattan = manhattan_distance(&a, &b);
        assert_eq!(manhattan, 21.0);
    }
}
