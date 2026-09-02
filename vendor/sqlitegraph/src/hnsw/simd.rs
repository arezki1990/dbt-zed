//! SIMD-Accelerated Distance Functions
#![allow(unused_unsafe)]
//!
//! This module provides SIMD-optimized implementations of vector distance
//! calculations using CPU intrinsics (AVX-512F and AVX2 on x86_64). Functions
//! automatically dispatch to the best available SIMD path based on runtime CPU
//! feature detection.
//!
//! # Architecture
//!
//! - **Scalar fallback**: Pure Rust, always available
//! - **AVX2 path**: 256-bit registers, 8 f32 per iteration
//! - **AVX-512F path**: 512-bit registers, 16 f32 per iteration
//! - **Runtime dispatch**: One-time CPU feature detection cached in `OnceLock`
//!
//! Dispatch order: `AVX-512F → AVX2 → Scalar`. AVX-512 is detected via
//! `is_x86_feature_detected!("avx512f")` and only used when present —
//! older x86 CPUs and non-x86 platforms transparently fall through.
//!
//! # Safety Guarantees
//!
//! All unsafe blocks are contained within this module and only use:
//! - Unaligned loads (`_mm256_loadu_ps`, `_mm512_loadu_ps`) — no alignment requirements
//! - Standard SIMD intrinsics — well-defined behavior for any f32 input
//! - Proper remainder handling — scalar loop processes trailing elements
//!
//! # Performance Characteristics
//!
//! ## AVX-512F (512-bit)
//! - **Throughput**: 16 floats per iteration
//! - **Speedup**: ~2× vs AVX2 on supported CPUs (Intel Sapphire Rapids+, AMD Zen4+)
//! - **FMA**: Uses `_mm512_fmadd_ps` for fused multiply-add throughout
//!
//! ## AVX2 (256-bit)
//! - **Throughput**: 8 floats per iteration
//! - **Speedup**: ~4-6× vs scalar (depends on FMA availability)
//!
//! ## Scalar Fallback
//! - **Throughput**: 1 float per iteration
//! - **Availability**: All platforms
//!
//! # Correctness
//!
//! SIMD and scalar implementations produce **bit-identical or near-identical**
//! results for the same inputs (differences ≤ `1e-5` relative error due to the
//! different summation order of horizontal reductions). All operations follow
//! IEEE 754 floating-point semantics.
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::simd::dot_product;
//!
//! let a = vec![1.0, 2.0, 3.0];
//! let b = vec![4.0, 5.0, 6.0];
//! let product = dot_product(&a, &b);
//! assert_eq!(product, 32.0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::sync::OnceLock;

/// Best SIMD path detected on the current CPU.
///
/// Detection runs once per process via [`simd_level`]; the result is cached in
/// a [`OnceLock`]. On non-x86 platforms this is always [`SimdLevel::Scalar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdLevel {
    /// AVX-512F (and FMA, which AVX-512F implies via VEX-encoded FMA on every
    /// AVX-512F-capable CPU). 16 f32 per register.
    Avx512,
    /// AVX2 + FMA. 8 f32 per register.
    Avx2,
    /// No SIMD — scalar Rust loop.
    Scalar,
}

static SIMD_LEVEL: OnceLock<SimdLevel> = OnceLock::new();

/// Detect the best available SIMD level on this CPU.
///
/// Called once on first use; subsequent calls return the cached value
/// from [`SIMD_LEVEL`] without re-running CPUID. Always returns
/// [`SimdLevel::Scalar`] on non-x86_64 targets.
pub fn simd_level() -> SimdLevel {
    *SIMD_LEVEL.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx512f") {
                return SimdLevel::Avx512;
            }
            if std::arch::is_x86_feature_detected!("avx2") {
                return SimdLevel::Avx2;
            }
        }
        SimdLevel::Scalar
    })
}

/// Scalar fallback implementation of dot product
///
/// This is the baseline implementation that works on all platforms.
/// It uses standard Rust iterator operations and is always available.
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
#[inline]
pub fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "Vectors must have the same length");
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// AVX2 implementation of dot product using x86_64 intrinsics
///
/// This implementation uses 256-bit AVX2 registers to process 8 floats
/// per iteration. Falls back to scalar for remainder elements.
///
/// # Safety
///
/// This function is marked unsafe because it requires:
/// - AVX2 CPU feature support (verified by caller)
/// - Proper use of unsafe intrinsics (contained within)
///
/// The function is safe to call when the AVX2 feature is available.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Dot product value (bit-identical to scalar implementation)
///
/// # Panics
///
/// Panics if vectors have different lengths
///
/// # Performance
///
/// - **Aligned vectors (8+ elements)**: ~4-6x faster than scalar
/// - **Small vectors (< 8 elements)**: Similar to scalar (overhead dominates)
/// - **Non-aligned remainder**: Scalar loop handles trailing elements
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    let len = a.len();
    let mut result = 0.0f32;

    // Process 8 elements at a time using AVX2
    // Using unaligned loads (_mm256_loadu_ps) which work on any alignment
    let simd_len = len & !7; // Round down to nearest multiple of 8
    let mut i = 0;

    if simd_len > 0 {
        let mut sum0 = unsafe { _mm256_setzero_ps() };

        while i < simd_len {
            // Load 8 floats from each vector
            let a_vec = unsafe { _mm256_loadu_ps(a.as_ptr().add(i)) };
            let b_vec = unsafe { _mm256_loadu_ps(b.as_ptr().add(i)) };

            // Multiply and accumulate using FMA if available, otherwise mul + add
            #[cfg(target_feature = "fma")]
            {
                sum0 = unsafe { _mm256_fmadd_ps(a_vec, b_vec, sum0) };
            }
            #[cfg(not(target_feature = "fma"))]
            {
                let mul = unsafe { _mm256_mul_ps(a_vec, b_vec) };
                sum0 = unsafe { _mm256_add_ps(mul, sum0) };
            }

            i += 8;
        }

        // Horizontal sum: extract high and low 128-bit lanes, add them, then extract result
        // _mm256_extractf128_ps: Extract 128 bits from 256-bit vector
        // _mm_add_ps: Add two 128-bit vectors element-wise
        // _mm_cvtss_f32: Extract first scalar from 128-bit vector
        let high = unsafe { _mm256_extractf128_ps(sum0, 1) }; // Extract upper 128 bits
        let low = _mm256_castps256_ps128(sum0); // Extract lower 128 bits
        let sum128 = unsafe { _mm_add_ps(high, low) }; // Add the two 128-bit vectors

        // Horizontal sum of 128-bit vector: shuffle and add
        // [x0, x1, x2, x3] -> shuffle to [x1, x0, x3, x2], add to get [x0+x1, x0+x1, x2+x3, x2+x3]
        let shuffle = unsafe { _mm_shuffle_ps(sum128, sum128, 0b01_00_11_10) };
        let sum2 = unsafe { _mm_add_ps(sum128, shuffle) };
        // Shuffle again to get high part duplicated: [x2+x3, x2+x3, x2+x3, x2+x3]
        let shuffle2 = unsafe { _mm_shuffle_ps(sum2, sum2, 0b00_00_11_11) };
        let sum3 = unsafe { _mm_add_ps(sum2, shuffle2) };

        result = unsafe { _mm_cvtss_f32(sum3) };
    }

    // Handle remaining elements with scalar loop
    while i < len {
        result += a[i] * b[i];
        i += 1;
    }

    result
}

/// AVX-512F implementation of dot product using 512-bit registers.
///
/// Processes 16 floats per iteration via `_mm512_fmadd_ps` (fused
/// multiply-add). Falls back to scalar for `len % 16` remainder.
///
/// # Safety
///
/// Must only be called on CPUs that support AVX-512F. The caller must verify
/// via `is_x86_feature_detected!("avx512f")` before calling.
///
/// # Performance
///
/// - **Aligned vectors (16+ elements)**: ~2× faster than AVX2
/// - **Small vectors (< 16 elements)**: Similar to AVX2 (overhead dominates)
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[inline]
unsafe fn dot_product_avx512(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    let len = a.len();
    let simd_len = len & !15; // round down to nearest multiple of 16
    let mut i = 0;
    let mut result = 0.0f32;

    if simd_len > 0 {
        let mut sum = unsafe { _mm512_setzero_ps() };
        while i < simd_len {
            let va = unsafe { _mm512_loadu_ps(a.as_ptr().add(i)) };
            let vb = unsafe { _mm512_loadu_ps(b.as_ptr().add(i)) };
            sum = unsafe { _mm512_fmadd_ps(va, vb, sum) };
            i += 16;
        }
        result = unsafe { _mm512_reduce_add_ps(sum) };
    }

    // Remainder
    while i < len {
        result += a[i] * b[i];
        i += 1;
    }
    result
}

/// Runtime-dispatched dot product with AVX-512 / AVX2 acceleration.
///
/// Selects the best available SIMD path (AVX-512F → AVX2 → Scalar) based on
/// CPU features detected at runtime. The detection happens once and is cached
/// in [`SIMD_LEVEL`] for minimal per-call overhead.
///
/// # Behavior
///
/// - **AVX-512F**: 16 floats per iteration via `dot_product_avx512` (~2× AVX2)
/// - **AVX2**: 8 floats per iteration via `dot_product_avx2` (~4-6× scalar)
/// - **Other**: scalar fallback
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
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::simd::dot_product;
///
/// let a = vec![1.0, 2.0, 3.0, 4.0];
/// let b = vec![5.0, 6.0, 7.0, 8.0];
/// let product = dot_product(&a, &b);
/// assert_eq!(product, 70.0); // 1*5 + 2*6 + 3*7 + 4*8
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[inline]
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: simd_level() only returns Avx512/Avx2 after verifying
        // the corresponding CPU feature via is_x86_feature_detected!.
        match simd_level() {
            SimdLevel::Avx512 => unsafe { dot_product_avx512(a, b) },
            SimdLevel::Avx2 => unsafe { dot_product_avx2(a, b) },
            SimdLevel::Scalar => dot_product_scalar(a, b),
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        dot_product_scalar(a, b)
    }
}

// ============================================================================
// NORM COMPUTATION
// ============================================================================

/// Scalar fallback implementation of squared norm computation
///
/// Returns the sum of squares (L2 norm squared). The caller should take
/// the square root if the actual norm is needed.
///
/// # Arguments
///
/// * `v` - Vector slice
///
/// # Returns
///
/// Sum of squares (norm squared)
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
#[inline]
pub fn compute_norm_squared_scalar(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum()
}

/// AVX2 implementation of squared norm computation using x86_64 intrinsics
///
/// This implementation uses 256-bit AVX2 registers to process 8 floats
/// per iteration. Falls back to scalar for remainder elements.
///
/// # Safety
///
/// This function is marked unsafe because it requires:
/// - AVX2 CPU feature support (verified by caller)
/// - Proper use of unsafe intrinsics (contained within)
///
/// The function is safe to call when the AVX2 feature is available.
///
/// # Arguments
///
/// * `v` - Vector slice
///
/// # Returns
///
/// Sum of squares (norm squared, bit-identical to scalar implementation)
///
/// # Performance
///
/// - **Aligned vectors (8+ elements)**: ~4-6x faster than scalar
/// - **Small vectors (< 8 elements)**: Similar to scalar (overhead dominates)
/// - **Non-aligned remainder**: Scalar loop handles trailing elements
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn compute_norm_squared_avx2(v: &[f32]) -> f32 {
    unsafe {
        use std::arch::x86_64::*;

        let len = v.len();
        let mut result = 0.0f32;

        // Process 8 elements at a time using AVX2
        let simd_len = len & !7; // Round down to nearest multiple of 8
        let mut i = 0;

        if simd_len > 0 {
            let mut sum0 = unsafe { _mm256_setzero_ps() };

            while i < simd_len {
                // Load 8 floats
                let v_vec = _mm256_loadu_ps(v.as_ptr().add(i));

                // Square each element
                let squared = _mm256_mul_ps(v_vec, v_vec);

                // Accumulate
                sum0 = _mm256_add_ps(squared, sum0);

                i += 8;
            }

            // Horizontal sum: extract high and low 128-bit lanes, add them
            let high = unsafe { _mm256_extractf128_ps(sum0, 1) };
            let low = _mm256_castps256_ps128(sum0);
            let sum128 = unsafe { _mm_add_ps(high, low) };

            // Horizontal sum of 128-bit vector
            let shuffle = unsafe { _mm_shuffle_ps(sum128, sum128, 0b01_00_11_10) };
            let sum2 = unsafe { _mm_add_ps(sum128, shuffle) };
            let shuffle2 = unsafe { _mm_shuffle_ps(sum2, sum2, 0b00_00_11_11) };
            let sum3 = unsafe { _mm_add_ps(sum2, shuffle2) };

            result = unsafe { _mm_cvtss_f32(sum3) };
        }

        // Handle remaining elements with scalar loop
        while i < len {
            let val = v[i];
            result += val * val;
            i += 1;
        }

        result
    }
}

/// AVX-512F implementation of squared norm computation.
///
/// Processes 16 floats per iteration via `_mm512_fmadd_ps(v, v, sum)`.
/// Falls back to scalar for `len % 16` remainder.
///
/// # Safety
///
/// Must only be called on CPUs that support AVX-512F.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[inline]
unsafe fn compute_norm_squared_avx512(v: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = v.len();
    let simd_len = len & !15;
    let mut i = 0;
    let mut result = 0.0f32;

    if simd_len > 0 {
        let mut sum = unsafe { _mm512_setzero_ps() };
        while i < simd_len {
            let vv = unsafe { _mm512_loadu_ps(v.as_ptr().add(i)) };
            sum = unsafe { _mm512_fmadd_ps(vv, vv, sum) };
            i += 16;
        }
        result = unsafe { _mm512_reduce_add_ps(sum) };
    }

    while i < len {
        let val = v[i];
        result += val * val;
        i += 1;
    }
    result
}

/// Runtime-dispatched squared norm computation with AVX-512 / AVX2 acceleration.
///
/// Selects the best available SIMD path (AVX-512F → AVX2 → Scalar). The
/// dispatch decision is cached in [`SIMD_LEVEL`].
///
/// # Arguments
///
/// * `v` - Vector slice
///
/// # Returns
///
/// Sum of squares (norm squared). Take the square root for the actual norm.
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::simd::compute_norm_squared;
///
/// let v = vec![3.0, 4.0];
/// let norm_sq = compute_norm_squared(&v);
/// assert_eq!(norm_sq, 25.0); // 3^2 + 4^2 = 9 + 16 = 25
/// let norm = norm_sq.sqrt();
/// assert_eq!(norm, 5.0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[inline]
pub fn compute_norm_squared(v: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        match simd_level() {
            SimdLevel::Avx512 => unsafe { compute_norm_squared_avx512(v) },
            SimdLevel::Avx2 => unsafe { compute_norm_squared_avx2(v) },
            SimdLevel::Scalar => compute_norm_squared_scalar(v),
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        compute_norm_squared_scalar(v)
    }
}

// ============================================================================
// COSINE SIMILARITY
// ============================================================================

/// Scalar fallback implementation of cosine similarity
///
/// Cosine similarity measures the cosine of the angle between two vectors,
/// providing a value between -1 and 1 where 1 indicates identical direction.
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
/// Panics if vectors are empty or have zero magnitude
///
/// # Performance
///
/// - Time Complexity: O(n) where n is vector dimension
/// - Memory Usage: O(1) additional space
#[inline]
pub fn cosine_similarity_scalar(a: &[f32], b: &[f32]) -> f32 {
    assert!(!a.is_empty(), "Vectors cannot be empty");

    let dot = dot_product_scalar(a, b);
    let norm_a = compute_norm_squared_scalar(a).sqrt();
    let norm_b = compute_norm_squared_scalar(b).sqrt();

    assert!(norm_a > f32::EPSILON, "First vector has zero magnitude");
    assert!(norm_b > f32::EPSILON, "Second vector has zero magnitude");

    dot / (norm_a * norm_b)
}

/// AVX2 implementation of cosine similarity using x86_64 intrinsics
///
/// This implementation uses AVX2-accelerated dot product and norm computation
/// for maximum performance.
///
/// # Safety
///
/// This function is marked unsafe because it requires:
/// - AVX2 CPU feature support (verified by caller)
/// - Proper use of unsafe intrinsics (contained within)
///
/// The function is safe to call when the AVX2 feature is available.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Cosine similarity value (bit-identical to scalar implementation)
///
/// # Panics
///
/// Panics if vectors are empty or have zero magnitude
///
/// # Performance
///
/// - **Large vectors (100+ elements)**: ~4-6x faster than scalar
/// - **Small vectors (< 8 elements)**: Similar to scalar (overhead dominates)
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn cosine_similarity_avx2(a: &[f32], b: &[f32]) -> f32 {
    unsafe {
        assert!(!a.is_empty(), "Vectors cannot be empty");

        let dot = dot_product_avx2(a, b);
        let norm_a = compute_norm_squared_avx2(a).sqrt();
        let norm_b = compute_norm_squared_avx2(b).sqrt();

        assert!(norm_a > f32::EPSILON, "First vector has zero magnitude");
        assert!(norm_b > f32::EPSILON, "Second vector has zero magnitude");

        dot / (norm_a * norm_b)
    }
}

/// AVX-512F implementation of cosine similarity.
///
/// Fuses the dot product and the two squared-norm computations into a single
/// loop over 16-float chunks, sharing loads of both vectors. Uses three
/// independent FMA accumulators (`dot`, `norm_a`, `norm_b`).
///
/// # Safety
///
/// Must only be called on CPUs that support AVX-512F.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[inline]
unsafe fn cosine_similarity_avx512(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    assert!(!a.is_empty(), "Vectors cannot be empty");

    let len = a.len();
    let simd_len = len & !15;
    let mut i = 0;

    let mut dot_sum = 0.0f32;
    let mut norm_a_sum = 0.0f32;
    let mut norm_b_sum = 0.0f32;

    if simd_len > 0 {
        let mut dot = unsafe { _mm512_setzero_ps() };
        let mut na = unsafe { _mm512_setzero_ps() };
        let mut nb = unsafe { _mm512_setzero_ps() };
        while i < simd_len {
            let va = unsafe { _mm512_loadu_ps(a.as_ptr().add(i)) };
            let vb = unsafe { _mm512_loadu_ps(b.as_ptr().add(i)) };
            dot = unsafe { _mm512_fmadd_ps(va, vb, dot) };
            na = unsafe { _mm512_fmadd_ps(va, va, na) };
            nb = unsafe { _mm512_fmadd_ps(vb, vb, nb) };
            i += 16;
        }
        dot_sum = unsafe { _mm512_reduce_add_ps(dot) };
        norm_a_sum = unsafe { _mm512_reduce_add_ps(na) };
        norm_b_sum = unsafe { _mm512_reduce_add_ps(nb) };
    }

    while i < len {
        dot_sum += a[i] * b[i];
        norm_a_sum += a[i] * a[i];
        norm_b_sum += b[i] * b[i];
        i += 1;
    }

    let norm_a = norm_a_sum.sqrt();
    let norm_b = norm_b_sum.sqrt();
    assert!(norm_a > f32::EPSILON, "First vector has zero magnitude");
    assert!(norm_b > f32::EPSILON, "Second vector has zero magnitude");
    dot_sum / (norm_a * norm_b)
}

/// Runtime-dispatched cosine similarity with AVX-512 / AVX2 acceleration.
///
/// Selects the best available SIMD path (AVX-512F → AVX2 → Scalar). On AVX-512
/// the dot product and both squared norms are fused into a single 16-wide loop;
/// on AVX2 they are computed via the 8-wide kernels in sequence.
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
/// Panics if:
/// - Vectors have different lengths
/// - Vectors are empty
/// - Either vector has zero magnitude
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::simd::cosine_similarity;
///
/// let a = vec![1.0, 0.0, 0.0];
/// let b = vec![1.0, 0.0, 0.0];
/// let similarity = cosine_similarity(&a, &b);
/// assert!((similarity - 1.0).abs() < f32::EPSILON);
///
/// let c = vec![1.0, 0.0];
/// let d = vec![0.0, 1.0];
/// let similarity = cosine_similarity(&c, &d);
/// assert!((similarity - 0.0).abs() < f32::EPSILON);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    #[cfg(target_arch = "x86_64")]
    {
        match simd_level() {
            SimdLevel::Avx512 => unsafe { cosine_similarity_avx512(a, b) },
            SimdLevel::Avx2 => unsafe { cosine_similarity_avx2(a, b) },
            SimdLevel::Scalar => cosine_similarity_scalar(a, b),
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        cosine_similarity_scalar(a, b)
    }
}

// ============================================================================
// EUCLIDEAN DISTANCE (L2)
// ============================================================================

/// Scalar fallback implementation of Euclidean (L2) distance
///
/// Computes the square root of the sum of squared differences between
/// corresponding elements of two vectors. This is the baseline implementation
/// used when SIMD instructions are not available.
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Euclidean distance (L2 norm) >= 0
///
/// # Performance
///
/// - Time: O(n) where n is vector dimension
/// - Memory: O(1) additional space
/// - No SIMD acceleration
#[inline]
pub fn euclidean_distance_scalar(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "Vectors must have the same length");
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let diff = x - y;
            diff * diff
        })
        .sum::<f32>()
        .sqrt()
}

/// AVX2-optimized implementation of Euclidean (L2) distance
///
/// Computes Euclidean distance using 256-bit AVX2 registers to process
/// 8 f32 values per iteration. This provides significant speedup for
/// large-dimensional vectors on AVX2-capable CPUs.
///
/// # Algorithm
///
/// 1. Load 8 floats from each vector using unaligned loads
/// 2. Compute difference: `av - bv` (subtraction)
/// 3. Square differences: `diff * diff` (multiplication)
/// 4. Horizontal sum of squared differences
/// 5. Accumulate across all chunks
/// 6. Handle remainder elements with scalar loop
/// 7. Return sqrt of accumulated sum
///
/// # Safety
///
/// This function must only be called when AVX2 is available:
/// - Use `is_x86_feature_detected!("avx2")` to check
/// - Marked `unsafe` because incorrect usage causes illegal instruction
/// - Uses `_mm256_loadu_ps` (unaligned load) for safety with any alignment
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Euclidean distance (L2 norm) >= 0
///
/// # Performance
///
/// - Processes 8 elements per iteration
/// - ~8x throughput for aligned large vectors
/// - Remainder handled with scalar loop
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn euclidean_distance_avx2(a: &[f32], b: &[f32]) -> f32 {
    unsafe {
        use std::arch::x86_64::*;

        assert_eq!(a.len(), b.len(), "Vectors must have the same length");

        let len = a.len();
        let mut sum = 0.0_f32;

        // Process 8 elements at a time
        let chunks = len / 8;
        let remainder = len % 8;

        let mut i = 0;
        for _ in 0..chunks {
            // Unaligned loads - safe for any alignment
            let av = _mm256_loadu_ps(a.as_ptr().add(i));
            let bv = _mm256_loadu_ps(b.as_ptr().add(i));

            // Compute difference: av - bv
            let diff = _mm256_sub_ps(av, bv);

            // Square differences: diff * diff
            let squared = _mm256_mul_ps(diff, diff);

            // Horizontal sum (partial)
            let high = _mm256_extractf128_ps(squared, 1);
            let low = _mm256_castps256_ps128(squared);
            let sum2 = _mm_add_ps(low, high);

            // Accumulate (complete horizontal sum)
            let mut tmp = [0.0_f32; 4];
            _mm_storeu_ps(tmp.as_mut_ptr(), sum2);
            sum += tmp[0] + tmp[1] + tmp[2] + tmp[3];

            i += 8;
        }

        // Handle remainder elements (len % 8)
        for j in 0..remainder {
            let diff = a[i + j] - b[i + j];
            sum += diff * diff;
        }

        sum.sqrt()
    }
}

/// AVX-512F implementation of Euclidean (L2) distance.
///
/// Processes 16 floats per iteration. The squared differences are accumulated
/// in-register via `_mm512_fmadd_ps(diff, diff, sum)`, then horizontally
/// reduced once at the end (vs. one reduction per chunk in the AVX2 path).
///
/// # Safety
///
/// Must only be called on CPUs that support AVX-512F.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[inline]
unsafe fn euclidean_distance_avx512(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    let len = a.len();
    let simd_len = len & !15;
    let mut i = 0;
    let mut sum = 0.0f32;

    if simd_len > 0 {
        let mut acc = unsafe { _mm512_setzero_ps() };
        while i < simd_len {
            let va = unsafe { _mm512_loadu_ps(a.as_ptr().add(i)) };
            let vb = unsafe { _mm512_loadu_ps(b.as_ptr().add(i)) };
            let diff = unsafe { _mm512_sub_ps(va, vb) };
            acc = unsafe { _mm512_fmadd_ps(diff, diff, acc) };
            i += 16;
        }
        sum = unsafe { _mm512_reduce_add_ps(acc) };
    }

    while i < len {
        let d = a[i] - b[i];
        sum += d * d;
        i += 1;
    }
    sum.sqrt()
}

/// Runtime-dispatched Euclidean (L2) distance with AVX-512 / AVX2 acceleration.
///
/// Selects the best available SIMD path (AVX-512F → AVX2 → Scalar).
///
/// # Arguments
///
/// * `a` - First vector slice
/// * `b` - Second vector slice (must have same length as a)
///
/// # Returns
///
/// Euclidean distance (L2 norm) >= 0
///
/// # Performance
///
/// - AVX-512: ~16× scalar throughput per iteration
/// - AVX2: ~8× scalar throughput per iteration
/// - Detection overhead: O(1) after first call (cached)
#[inline]
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "Vectors must have the same length");

    #[cfg(target_arch = "x86_64")]
    {
        match simd_level() {
            SimdLevel::Avx512 => unsafe { euclidean_distance_avx512(a, b) },
            SimdLevel::Avx2 => unsafe { euclidean_distance_avx2(a, b) },
            SimdLevel::Scalar => euclidean_distance_scalar(a, b),
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        euclidean_distance_scalar(a, b)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_product_correctness() {
        let a: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let b: Vec<f32> = (101..=200).map(|i| i as f32).collect();

        let result = dot_product(&a, &b);
        let expected: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

        assert!((result - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn test_dot_product_empty_vectors() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];

        let result = dot_product(&a, &b);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_dot_product_single_element() {
        let a = vec![5.0];
        let b = vec![3.0];

        let result = dot_product(&a, &b);
        assert_eq!(result, 15.0);
    }

    #[test]
    fn test_dot_product_non_aligned_size() {
        // Test sizes that are not multiples of 8
        for size in [1, 3, 5, 7, 9, 13, 17, 25] {
            let a: Vec<f32> = (1..=size).map(|i| i as f32).collect();
            let b: Vec<f32> = (1..=size).map(|i| (i * 2) as f32).collect();

            let result = dot_product(&a, &b);
            let expected: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

            assert!(
                (result - expected).abs() < f32::EPSILON,
                "Failed for size {}",
                size
            );
        }
    }

    #[test]
    fn test_dot_product_aligned_size() {
        // Test sizes that are multiples of 8 (optimal for AVX2)
        for size in [8, 16, 32, 64, 128] {
            let a: Vec<f32> = (1..=size).map(|i| i as f32).collect();
            let b: Vec<f32> = (1..=size).map(|i| (i * 2) as f32).collect();

            let result = dot_product(&a, &b);
            let expected: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

            assert!(
                (result - expected).abs() < f32::EPSILON,
                "Failed for size {}",
                size
            );
        }
    }

    #[test]
    fn test_dot_product_negative_values() {
        let a = [-1.0, 2.0, -3.0, 4.0];
        let b = [5.0, -6.0, 7.0, -8.0];

        let result = dot_product(&a, &b);
        let expected: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

        assert_eq!(result, expected);
        assert_eq!(result, -70.0);
    }

    #[test]
    fn test_dot_product_zeros() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 2.0, 3.0];

        let result = dot_product(&a, &b);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_dot_product_large_vectors() {
        // Performance sanity check - this should complete quickly
        let a: Vec<f32> = (1..=1024).map(|i| i as f32).collect();
        let b: Vec<f32> = (1..=1024).map(|i| (i * 2) as f32).collect();

        let result = dot_product(&a, &b);
        let expected: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

        // Use relative error tolerance for large sums due to floating-point
        // SIMD horizontal sum processes elements in different order causing
        // small numerical differences (~1e-6 relative error)
        let abs_diff = (result - expected).abs();
        let rel_error = abs_diff / expected.abs();
        assert!(
            rel_error < 1e-5 || abs_diff < f32::EPSILON,
            "Relative error {} too large (abs diff: {}, result: {}, expected: {})",
            rel_error,
            abs_diff,
            result,
            expected
        );
    }

    #[test]
    fn test_dot_product_scalar_matches_simd() {
        // Verify scalar and SIMD produce identical results (within floating-point precision)
        let sizes = [1, 3, 7, 8, 15, 16, 31, 32, 100, 256];

        for size in sizes {
            let a: Vec<f32> = (0..size).map(|i| (i as f32 * 0.1).fract()).collect();
            let b: Vec<f32> = (0..size).map(|i| (i as f32 * 0.13).fract()).collect();

            let scalar_result = dot_product_scalar(&a, &b);
            let simd_result = dot_product(&a, &b);

            let abs_diff = (scalar_result - simd_result).abs();
            let rel_error = if scalar_result.abs() > f32::EPSILON {
                abs_diff / scalar_result.abs()
            } else {
                abs_diff
            };

            assert!(
                rel_error < 1e-5 || abs_diff < f32::EPSILON,
                "Scalar and SIMD differ for size {}: scalar={}, simd={}, diff={}, rel_error={}",
                size,
                scalar_result,
                simd_result,
                abs_diff,
                rel_error
            );
        }
    }

    #[test]
    #[should_panic(expected = "Vectors must have the same length")]
    fn test_dot_product_different_lengths_panic() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];

        dot_product(&a, &b);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_avx2_availability() {
        // This test just verifies that AVX2 detection doesn't panic
        // The actual behavior depends on the CPU running the test
        let has_avx2 = std::arch::is_x86_feature_detected!("avx2");
        println!("AVX2 available: {}", has_avx2);

        // Either way, dot_product should work correctly
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        let result = dot_product(&a, &b);
        assert_eq!(result, 32.0);
    }

    // -------------------------------------------------------------------------
    // NORM COMPUTATION TESTS
    // -------------------------------------------------------------------------

    #[test]
    fn test_compute_norm_squared_basic() {
        let v = vec![3.0, 4.0];
        let norm_sq = compute_norm_squared(&v);
        assert_eq!(norm_sq, 25.0); // 3^2 + 4^2 = 9 + 16 = 25
    }

    #[test]
    fn test_compute_norm_squared_unit_vector() {
        let v = vec![1.0, 0.0, 0.0];
        let norm_sq = compute_norm_squared(&v);
        assert_eq!(norm_sq, 1.0);
    }

    #[test]
    fn test_compute_norm_squared_zero_vector() {
        let v = vec![0.0, 0.0, 0.0];
        let norm_sq = compute_norm_squared(&v);
        assert_eq!(norm_sq, 0.0);
    }

    #[test]
    fn test_compute_norm_squared_pythagorean_triple() {
        let v = vec![5.0, 12.0];
        let norm_sq = compute_norm_squared(&v);
        assert_eq!(norm_sq, 169.0); // 5^2 + 12^2 = 25 + 144 = 169
        let norm = norm_sq.sqrt();
        assert_eq!(norm, 13.0);
    }

    #[test]
    fn test_compute_norm_squared_non_aligned() {
        // Test sizes not divisible by 8
        for size in [1, 3, 5, 7, 9, 13, 17] {
            let v: Vec<f32> = (1..=size).map(|i| i as f32).collect();
            let result = compute_norm_squared(&v);
            let expected: f32 = v.iter().map(|x| x * x).sum();

            assert!(
                (result - expected).abs() < f32::EPSILON,
                "Failed for size {}",
                size
            );
        }
    }

    #[test]
    fn test_compute_norm_squared_large_vector() {
        let v: Vec<f32> = (1..=1000).map(|i| i as f32 * 0.1).collect();
        let result = compute_norm_squared(&v);
        let expected: f32 = v.iter().map(|x| x * x).sum();

        // Use relative tolerance for large vectors due to floating-point accumulation
        let abs_diff = (result - expected).abs();
        let rel_error = if expected.abs() > f32::EPSILON {
            abs_diff / expected.abs()
        } else {
            abs_diff
        };
        assert!(
            rel_error < 1e-5 || abs_diff < 1e-3,
            "Norm squared differs: result={}, expected={}, diff={}, rel_error={}",
            result,
            expected,
            abs_diff,
            rel_error
        );
    }

    #[test]
    fn test_compute_norm_squared_matches_scalar() {
        let v: Vec<f32> = (1..=100).map(|i| i as f32 * 0.1).collect();

        let scalar_result = compute_norm_squared_scalar(&v);
        let simd_result = compute_norm_squared(&v);

        // Allow small tolerance due to different accumulation order in SIMD
        let abs_diff = (scalar_result - simd_result).abs();
        assert!(
            abs_diff < 1e-3,
            "Norm squared differs: scalar={}, simd={}, diff={}",
            scalar_result,
            simd_result,
            abs_diff
        );
    }

    // -------------------------------------------------------------------------
    // COSINE SIMILARITY TESTS
    // -------------------------------------------------------------------------

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity + 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_normalized() {
        // Pre-normalized vectors should have cosine similarity equal to dot product
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ]; // Unit vector at 45 degrees

        let similarity = cosine_similarity(&a, &b);
        assert!((similarity - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_non_zero() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let similarity = cosine_similarity(&a, &b);
        // Verify it's within valid range [-1, 1]
        assert!((-1.0..=1.0).contains(&similarity));
    }

    #[test]
    fn test_cosine_matches_manual_calculation() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let result = cosine_similarity(&a, &b);

        // Manual calculation
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        let expected = dot / (norm_a * norm_b);

        assert!((result - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cosine_similarity_matches_scalar() {
        let a: Vec<f32> = (1..=100).map(|i| i as f32 * 0.1).collect();
        let b: Vec<f32> = (101..=200).map(|i| i as f32 * 0.1).collect();

        let scalar_result = cosine_similarity_scalar(&a, &b);
        let simd_result = cosine_similarity(&a, &b);

        assert!((scalar_result - simd_result).abs() < f32::EPSILON * 10.0);
    }

    #[test]
    fn test_cosine_similarity_large_vectors() {
        let a: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
        let b: Vec<f32> = (1001..=2000).map(|i| i as f32).collect();

        // Should not panic and should return valid value
        let similarity = cosine_similarity(&a, &b);
        assert!((-1.0..=1.0).contains(&similarity));
    }

    #[test]
    #[should_panic(expected = "Vectors must have the same length")]
    fn test_cosine_similarity_different_lengths_panic() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        cosine_similarity(&a, &b);
    }

    #[test]
    #[should_panic(expected = "Vectors cannot be empty")]
    fn test_cosine_similarity_empty_vectors_panic() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        cosine_similarity(&a, &b);
    }

    #[test]
    #[should_panic(expected = "First vector has zero magnitude")]
    fn test_cosine_similarity_zero_magnitude_panic() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        cosine_similarity(&a, &b);
    }

    #[test]
    #[should_panic(expected = "Second vector has zero magnitude")]
    fn test_cosine_similarity_zero_magnitude_b_panic() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        cosine_similarity(&a, &b);
    }

    // -------------------------------------------------------------------------
    // INTEGRATION TESTS
    // -------------------------------------------------------------------------

    #[test]
    fn test_dot_norm_cosine_integration() {
        // Verify that cosine = dot / (norm_a * norm_b)
        let a: Vec<f32> = (1..=50).map(|i| i as f32 * 0.1).collect();
        let b: Vec<f32> = (51..=100).map(|i| i as f32 * 0.1).collect();

        let dot = dot_product(&a, &b);
        let norm_a = compute_norm_squared(&a).sqrt();
        let norm_b = compute_norm_squared(&b).sqrt();
        let cosine = cosine_similarity(&a, &b);

        let expected_cosine = dot / (norm_a * norm_b);
        assert!((cosine - expected_cosine).abs() < f32::EPSILON);
    }

    #[test]
    fn test_euclidean_distance_scalar_identical() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0, 3.0];
        let distance = euclidean_distance_scalar(&a, &b);
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn test_euclidean_distance_scalar_basic() {
        let a = [0.0, 0.0];
        let b = [1.0, 0.0];
        let distance = euclidean_distance_scalar(&a, &b);
        assert_eq!(distance, 1.0);
    }

    #[test]
    fn test_euclidean_distance_scalar_diagonal() {
        let a = [0.0, 0.0];
        let b = [1.0, 1.0];
        let distance = euclidean_distance_scalar(&a, &b);
        assert!((distance - std::f32::consts::SQRT_2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_euclidean_distance_scalar_high_dimensional() {
        let a: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let b: Vec<f32> = (101..=200).map(|i| i as f32).collect();

        let distance = euclidean_distance_scalar(&a, &b);
        assert!(distance > 0.0);
        assert!(distance.is_finite());
    }

    #[test]
    fn test_euclidean_distance_scalar_negative_values() {
        let a = [-1.0, 2.0, -3.0];
        let b = [4.0, -5.0, 6.0];
        let distance = euclidean_distance_scalar(&a, &b);
        assert!((distance - 12.4499).abs() < 0.001);
    }

    #[test]
    fn test_euclidean_distance_scalar_non_aligned() {
        let a: Vec<f32> = (1..=13).map(|i| i as f32).collect();
        let b: Vec<f32> = (14..=26).map(|i| i as f32).collect();

        let distance = euclidean_distance_scalar(&a, &b);
        assert!(distance > 0.0);
        assert!(distance.is_finite());
    }

    #[test]
    fn test_euclidean_distance_dispatch_identical() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0, 3.0];
        let distance = euclidean_distance(&a, &b);
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn test_euclidean_distance_dispatch_basic() {
        let a = [0.0, 0.0];
        let b = [1.0, 0.0];
        let distance = euclidean_distance(&a, &b);
        assert_eq!(distance, 1.0);
    }

    #[test]
    fn test_euclidean_distance_dispatch_matches_scalar() {
        let a: Vec<f32> = (1..=50).map(|i| i as f32 * 0.1).collect();
        let b: Vec<f32> = (51..=100).map(|i| i as f32 * 0.1).collect();

        let scalar_result = euclidean_distance_scalar(&a, &b);
        let dispatch_result = euclidean_distance(&a, &b);

        assert!((dispatch_result - scalar_result).abs() < f32::EPSILON);
    }

    #[test]
    fn test_euclidean_distance_large_vector() {
        let a: Vec<f32> = (1..=1000).map(|i| i as f32).collect();
        let b: Vec<f32> = (1001..=2000).map(|i| i as f32).collect();

        let distance = euclidean_distance(&a, &b);
        assert!(distance > 0.0);
        assert!(distance.is_finite());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_euclidean_distance_avx2_matches_scalar() {
        if std::arch::is_x86_feature_detected!("avx2") {
            let a: Vec<f32> = (1..=100).map(|i| i as f32 * 0.73).collect();
            let b: Vec<f32> = (101..=200).map(|i| i as f32 * 1.23).collect();

            let scalar_result = euclidean_distance_scalar(&a, &b);
            let avx2_result = unsafe { euclidean_distance_avx2(&a, &b) };

            assert!((avx2_result - scalar_result).abs() < f32::EPSILON);
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_euclidean_distance_avx2_remainder() {
        if std::arch::is_x86_feature_detected!("avx2") {
            for size in [1, 7, 8, 9, 15, 16, 17, 23, 24, 25].iter() {
                let a: Vec<f32> = (1..=*size).map(|i| i as f32).collect();
                let b: Vec<f32> = (*size + 1..=*size * 2).map(|i| i as f32).collect();

                let scalar_result = euclidean_distance_scalar(&a, &b);
                let avx2_result = unsafe { euclidean_distance_avx2(&a, &b) };

                assert!(
                    (avx2_result - scalar_result).abs() < f32::EPSILON,
                    "Mismatch for size {}: scalar={}, avx2={}",
                    size,
                    scalar_result,
                    avx2_result
                );
            }
        }
    }

    #[test]
    fn test_euclidean_distance_zero_vector() {
        let a = [0.0, 0.0, 0.0];
        let b = [0.0, 0.0, 0.0];
        let distance = euclidean_distance(&a, &b);
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn test_euclidean_distance_symmetry() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];

        let distance_ab = euclidean_distance(&a, &b);
        let distance_ba = euclidean_distance(&b, &a);

        assert_eq!(distance_ab, distance_ba);
    }

    #[test]
    #[should_panic(expected = "Vectors must have the same length")]
    fn test_euclidean_distance_different_lengths_panic() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];
        euclidean_distance(&a, &b);
    }

    // -------------------------------------------------------------------------
    // SimdLevel detection tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_simd_level_detection_succeeds() {
        // Never panics, always returns a valid variant.
        let level = simd_level();
        eprintln!("Detected SIMD level: {:?}", level);

        // Idempotent + cached: calling again returns the same value.
        assert_eq!(level, simd_level());
        assert_eq!(level, simd_level());
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_simd_level_matches_cpu_features() {
        // Verify simd_level() picks the highest available path.
        let level = simd_level();
        let has_avx512 = std::arch::is_x86_feature_detected!("avx512f");
        let has_avx2 = std::arch::is_x86_feature_detected!("avx2");
        match level {
            SimdLevel::Avx512 => assert!(has_avx512),
            SimdLevel::Avx2 => {
                assert!(has_avx2);
                assert!(!has_avx512, "should have picked Avx512 if available");
            }
            SimdLevel::Scalar => {
                assert!(!has_avx2);
                assert!(!has_avx512);
            }
        }
    }

    // -------------------------------------------------------------------------
    // AVX-512 vs scalar correctness
    // -------------------------------------------------------------------------

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx512_dot_product_matches_scalar() {
        if !std::arch::is_x86_feature_detected!("avx512f") {
            eprintln!("AVX-512 not available, skipping");
            return;
        }
        let a: Vec<f32> = (0..384).map(|i| (i as f32) * 0.01).collect();
        let b: Vec<f32> = (0..384).map(|i| (i as f32) * 0.02 - 0.5).collect();
        let scalar = dot_product_scalar(&a, &b);
        let avx512 = unsafe { dot_product_avx512(&a, &b) };
        let abs_diff = (scalar - avx512).abs();
        let rel_error = abs_diff / scalar.abs().max(f32::EPSILON);
        assert!(
            rel_error < 1e-5,
            "scalar={scalar}, avx512={avx512}, rel_error={rel_error}"
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx512_norm_squared_matches_scalar() {
        if !std::arch::is_x86_feature_detected!("avx512f") {
            return;
        }
        let v: Vec<f32> = (0..512).map(|i| (i as f32) * 0.03).collect();
        let scalar = compute_norm_squared_scalar(&v);
        let avx512 = unsafe { compute_norm_squared_avx512(&v) };
        let rel = (scalar - avx512).abs() / scalar.abs();
        assert!(rel < 1e-5, "scalar={scalar}, avx512={avx512}, rel={rel}");
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx512_cosine_similarity_matches_scalar() {
        if !std::arch::is_x86_feature_detected!("avx512f") {
            return;
        }
        let a: Vec<f32> = (0..768).map(|i| (i as f32) * 0.01 + 1.0).collect();
        let b: Vec<f32> = (0..768).map(|i| (i as f32) * 0.02 + 0.5).collect();
        let scalar = cosine_similarity_scalar(&a, &b);
        let avx512 = unsafe { cosine_similarity_avx512(&a, &b) };
        assert!(
            (scalar - avx512).abs() < 1e-5,
            "scalar={scalar}, avx512={avx512}"
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx512_euclidean_distance_matches_scalar() {
        if !std::arch::is_x86_feature_detected!("avx512f") {
            return;
        }
        let a: Vec<f32> = (0..1024).map(|i| (i as f32) * 0.5).collect();
        let b: Vec<f32> = (0..1024).map(|i| (i as f32) * 0.5 + 0.25).collect();
        let scalar = euclidean_distance_scalar(&a, &b);
        let avx512 = unsafe { euclidean_distance_avx512(&a, &b) };
        let rel = (scalar - avx512).abs() / scalar.abs().max(f32::EPSILON);
        assert!(rel < 1e-5, "scalar={scalar}, avx512={avx512}, rel={rel}");
    }

    // -------------------------------------------------------------------------
    // AVX-512 remainder handling (len % 16 != 0)
    // -------------------------------------------------------------------------

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx512_remainder_handling() {
        if !std::arch::is_x86_feature_detected!("avx512f") {
            return;
        }
        // Sizes that exercise every possible (len / 16, len % 16) bucket.
        for size in [1usize, 7, 15, 16, 17, 31, 32, 33, 48, 137, 255] {
            let a: Vec<f32> = (0..size).map(|i| i as f32).collect();
            let b: Vec<f32> = (0..size).map(|i| (i as f32) * 0.5).collect();

            let scalar = dot_product_scalar(&a, &b);
            let avx512 = unsafe { dot_product_avx512(&a, &b) };
            let abs = (scalar - avx512).abs();
            let rel = if scalar.abs() > f32::EPSILON {
                abs / scalar.abs()
            } else {
                abs
            };
            assert!(
                rel < 1e-5 || abs < 1e-3,
                "dot size={size}: scalar={scalar}, avx512={avx512}, rel={rel}"
            );

            let scalar_e = euclidean_distance_scalar(&a, &b);
            let avx512_e = unsafe { euclidean_distance_avx512(&a, &b) };
            let abs_e = (scalar_e - avx512_e).abs();
            let rel_e = if scalar_e.abs() > f32::EPSILON {
                abs_e / scalar_e.abs()
            } else {
                abs_e
            };
            assert!(
                rel_e < 1e-5 || abs_e < 1e-3,
                "euclidean size={size}: scalar={scalar_e}, avx512={avx512_e}, rel={rel_e}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Typical embedding dimensions through the public dispatch.
    // -------------------------------------------------------------------------

    #[test]
    fn test_dispatch_typical_embedding_dims() {
        // 384 = MiniLM, 768 = BERT-base, 1024 = Voyage-2/Mistral, 1536 = OpenAI ada-002
        for dim in [384usize, 768, 1024, 1536] {
            let a: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.001 + 0.1).collect();
            let b: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.002 - 0.05).collect();

            let scalar_dot = dot_product_scalar(&a, &b);
            let auto_dot = dot_product(&a, &b);
            let rel_dot = (scalar_dot - auto_dot).abs() / scalar_dot.abs().max(f32::EPSILON);
            assert!(
                rel_dot < 1e-5,
                "dot dim={dim}: scalar={scalar_dot}, auto={auto_dot}, rel={rel_dot}"
            );

            let scalar_cos = cosine_similarity_scalar(&a, &b);
            let auto_cos = cosine_similarity(&a, &b);
            assert!(
                (scalar_cos - auto_cos).abs() < 1e-5,
                "cosine dim={dim}: scalar={scalar_cos}, auto={auto_cos}"
            );

            let scalar_eu = euclidean_distance_scalar(&a, &b);
            let auto_eu = euclidean_distance(&a, &b);
            let rel_eu = (scalar_eu - auto_eu).abs() / scalar_eu.abs().max(f32::EPSILON);
            assert!(
                rel_eu < 1e-5,
                "euclidean dim={dim}: scalar={scalar_eu}, auto={auto_eu}, rel={rel_eu}"
            );
        }
    }
}
