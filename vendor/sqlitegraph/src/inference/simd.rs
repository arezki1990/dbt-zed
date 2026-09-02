//! SIMD-optimized matrix operations for inference.
//!
//! Runtime CPU feature detection with fallback chain:
//!   AVX-512F + FMA (16 f32 SIMD width)
//!   -> AVX2 + FMA (8 f32 SIMD width)
//!   -> scalar fallback
//!
//! All functions operate on row-major f32 slices.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

// ---------------------------------------------------------------------------
// CPU feature detection (detected once at first use)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, PartialEq)]
enum SimdKind {
    Avx512Fma,
    Avx2Fma,
    Scalar,
}

#[cfg(target_arch = "x86_64")]
static SIMD_KIND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

#[cfg(target_arch = "x86_64")]
fn detect_simd() -> SimdKind {
    let cached = SIMD_KIND.load(std::sync::atomic::Ordering::Relaxed);
    if cached != u8::MAX {
        return unsafe { std::mem::transmute::<u8, SimdKind>(cached) };
    }
    let kind = if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("fma") {
        SimdKind::Avx512Fma
    } else if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
        SimdKind::Avx2Fma
    } else {
        SimdKind::Scalar
    };
    SIMD_KIND.store(kind as u8, std::sync::atomic::Ordering::Relaxed);
    kind
}

// ---------------------------------------------------------------------------
// Public API - dispatches to the best available path
// ---------------------------------------------------------------------------

/// Matrix-vector multiply: output[i] = dot(weight[i*in_dim..], input).
/// Weight layout: [out_dim, in_dim] row-major.
pub fn matmul(weight: &[f32], input: &[f32], out_dim: usize, in_dim: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; out_dim];
    matmul_into(weight, input, &mut output, out_dim, in_dim);
    output
}

/// Like `matmul` but writes into a pre-allocated output slice.
pub fn matmul_into(
    weight: &[f32],
    input: &[f32],
    output: &mut [f32],
    out_dim: usize,
    in_dim: usize,
) {
    #[cfg(target_arch = "x86_64")]
    {
        match detect_simd() {
            SimdKind::Avx512Fma => {
                unsafe { matmul_avx512(weight, input, output, out_dim, in_dim) };
                return;
            }
            SimdKind::Avx2Fma => {
                unsafe { matmul_avx2(weight, input, output, out_dim, in_dim) };
                return;
            }
            SimdKind::Scalar => {}
        }
    }
    matmul_scalar(weight, input, output, out_dim, in_dim);
}

/// Fused SiLU-gated FFN: computes gate and up dot products together,
/// applies SiLU(gate) * up activation, then down projection.
///
/// ffn_gate/up/down are all [n_neurons, hidden_dim] row-major.
/// x_norm is [hidden_dim]. Returns [hidden_dim].
pub fn dense_ffn(
    ffn_gate: &[f32],
    ffn_up: &[f32],
    ffn_down: &[f32],
    x_norm: &[f32],
    hidden_dim: usize,
) -> Vec<f32> {
    let n = ffn_gate.len() / hidden_dim;
    let mut intermediate = vec![0.0f32; n];

    // Phase 1: compute SiLU(gate) * up for all neurons
    #[cfg(target_arch = "x86_64")]
    {
        match detect_simd() {
            SimdKind::Avx512Fma => {
                unsafe {
                    ffn_gate_up_avx512(ffn_gate, ffn_up, x_norm, &mut intermediate, n, hidden_dim)
                };
            }
            SimdKind::Avx2Fma => {
                unsafe {
                    ffn_gate_up_avx2(ffn_gate, ffn_up, x_norm, &mut intermediate, n, hidden_dim)
                };
            }
            SimdKind::Scalar => {
                ffn_gate_up_scalar(ffn_gate, ffn_up, x_norm, &mut intermediate, n, hidden_dim);
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    ffn_gate_up_scalar(ffn_gate, ffn_up, x_norm, &mut intermediate, n, hidden_dim);

    // Phase 2: down projection (TRANSPOSE matmul)
    // ffn_down is [n, hidden_dim] row-major. We need:
    //   output[i] = sum_j intermediate[j] * ffn_down[j * hidden_dim + i]
    // This is ffn_down^T @ intermediate. Cannot use row-oriented matmul.
    let mut output = vec![0.0f32; hidden_dim];
    #[cfg(target_arch = "x86_64")]
    {
        match detect_simd() {
            SimdKind::Avx512Fma => {
                unsafe {
                    transpose_matvec_avx512(ffn_down, &intermediate, &mut output, n, hidden_dim)
                };
            }
            SimdKind::Avx2Fma => {
                unsafe {
                    transpose_matvec_avx2(ffn_down, &intermediate, &mut output, n, hidden_dim)
                };
            }
            SimdKind::Scalar => {
                transpose_matvec_scalar(ffn_down, &intermediate, &mut output, n, hidden_dim);
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    transpose_matvec_scalar(ffn_down, &intermediate, &mut output, n, hidden_dim);

    output
}

// ---------------------------------------------------------------------------
// AVX-512 FMA implementation (16 f32 per register)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,fma")]
unsafe fn matmul_avx512(
    weight: &[f32],
    input: &[f32],
    output: &mut [f32],
    out_dim: usize,
    in_dim: usize,
) {
    unsafe {
        for (j, out_slot) in output.iter_mut().enumerate().take(out_dim) {
            let row = weight.as_ptr().add(j * in_dim);

            let mut sum0 = _mm512_setzero_ps();
            let mut sum1 = _mm512_setzero_ps();

            let mut i = 0;
            while i + 31 < in_dim {
                let w0 = _mm512_loadu_ps(row.add(i));
                let w1 = _mm512_loadu_ps(row.add(i + 16));
                let i0 = _mm512_loadu_ps(input.as_ptr().add(i));
                let i1 = _mm512_loadu_ps(input.as_ptr().add(i + 16));
                sum0 = _mm512_fmadd_ps(w0, i0, sum0);
                sum1 = _mm512_fmadd_ps(w1, i1, sum1);
                i += 32;
            }
            while i + 15 < in_dim {
                let w = _mm512_loadu_ps(row.add(i));
                let inp = _mm512_loadu_ps(input.as_ptr().add(i));
                sum0 = _mm512_fmadd_ps(w, inp, sum0);
                i += 16;
            }

            let mut scalar_sum = _mm512_reduce_add_ps(_mm512_add_ps(sum0, sum1));

            for k in i..in_dim {
                scalar_sum += *row.add(k) * *input.as_ptr().add(k);
            }

            *out_slot = scalar_sum;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,fma")]
unsafe fn ffn_gate_up_avx512(
    ffn_gate: &[f32],
    ffn_up: &[f32],
    x_norm: &[f32],
    intermediate: &mut [f32],
    n: usize,
    hidden_dim: usize,
) {
    unsafe {
        for (j, inter_slot) in intermediate.iter_mut().enumerate().take(n) {
            let gate_row = ffn_gate.as_ptr().add(j * hidden_dim);
            let up_row = ffn_up.as_ptr().add(j * hidden_dim);

            let mut g0 = _mm512_setzero_ps();
            let mut g1 = _mm512_setzero_ps();
            let mut u0 = _mm512_setzero_ps();
            let mut u1 = _mm512_setzero_ps();

            let mut i = 0;
            while i + 31 < hidden_dim {
                let x0 = _mm512_loadu_ps(x_norm.as_ptr().add(i));
                let x1 = _mm512_loadu_ps(x_norm.as_ptr().add(i + 16));
                g0 = _mm512_fmadd_ps(_mm512_loadu_ps(gate_row.add(i)), x0, g0);
                g1 = _mm512_fmadd_ps(_mm512_loadu_ps(gate_row.add(i + 16)), x1, g1);
                u0 = _mm512_fmadd_ps(_mm512_loadu_ps(up_row.add(i)), x0, u0);
                u1 = _mm512_fmadd_ps(_mm512_loadu_ps(up_row.add(i + 16)), x1, u1);
                i += 32;
            }
            while i + 15 < hidden_dim {
                let x = _mm512_loadu_ps(x_norm.as_ptr().add(i));
                g0 = _mm512_fmadd_ps(_mm512_loadu_ps(gate_row.add(i)), x, g0);
                u0 = _mm512_fmadd_ps(_mm512_loadu_ps(up_row.add(i)), x, u0);
                i += 16;
            }

            let g = _mm512_reduce_add_ps(_mm512_add_ps(g0, g1));
            let u = _mm512_reduce_add_ps(_mm512_add_ps(u0, u1));

            let mut g_scalar = g;
            let mut u_scalar = u;
            for k in i..hidden_dim {
                let xk = *x_norm.as_ptr().add(k);
                g_scalar += *gate_row.add(k) * xk;
                u_scalar += *up_row.add(k) * xk;
            }

            // SiLU(g) * u = g * sigmoid(g) * u
            let silu_g = g_scalar * (1.0 / (1.0 + (-g_scalar).exp()));
            *inter_slot = silu_g * u_scalar;
        }
    }
}

// ---------------------------------------------------------------------------
// AVX2 FMA implementation (8 f32 per register)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn matmul_avx2(
    weight: &[f32],
    input: &[f32],
    output: &mut [f32],
    out_dim: usize,
    in_dim: usize,
) {
    unsafe {
        for (j, out_slot) in output.iter_mut().enumerate().take(out_dim) {
            let row = weight.as_ptr().add(j * in_dim);

            let mut sum0 = _mm256_setzero_ps();
            let mut sum1 = _mm256_setzero_ps();
            let mut sum2 = _mm256_setzero_ps();
            let mut sum3 = _mm256_setzero_ps();

            let mut i = 0;
            while i + 31 < in_dim {
                let x0 = _mm256_loadu_ps(input.as_ptr().add(i));
                let x1 = _mm256_loadu_ps(input.as_ptr().add(i + 8));
                let x2 = _mm256_loadu_ps(input.as_ptr().add(i + 16));
                let x3 = _mm256_loadu_ps(input.as_ptr().add(i + 24));
                sum0 = _mm256_fmadd_ps(_mm256_loadu_ps(row.add(i)), x0, sum0);
                sum1 = _mm256_fmadd_ps(_mm256_loadu_ps(row.add(i + 8)), x1, sum1);
                sum2 = _mm256_fmadd_ps(_mm256_loadu_ps(row.add(i + 16)), x2, sum2);
                sum3 = _mm256_fmadd_ps(_mm256_loadu_ps(row.add(i + 24)), x3, sum3);
                i += 32;
            }
            while i + 7 < in_dim {
                let x = _mm256_loadu_ps(input.as_ptr().add(i));
                sum0 = _mm256_fmadd_ps(_mm256_loadu_ps(row.add(i)), x, sum0);
                i += 8;
            }

            let mut s = hsum256(_mm256_add_ps(
                _mm256_add_ps(sum0, sum1),
                _mm256_add_ps(sum2, sum3),
            ));

            for k in i..in_dim {
                s += *row.add(k) * *input.as_ptr().add(k);
            }
            *out_slot = s;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn ffn_gate_up_avx2(
    ffn_gate: &[f32],
    ffn_up: &[f32],
    x_norm: &[f32],
    intermediate: &mut [f32],
    n: usize,
    hidden_dim: usize,
) {
    unsafe {
        for (j, inter_slot) in intermediate.iter_mut().enumerate().take(n) {
            let gate_row = ffn_gate.as_ptr().add(j * hidden_dim);
            let up_row = ffn_up.as_ptr().add(j * hidden_dim);

            let mut g0 = _mm256_setzero_ps();
            let mut g1 = _mm256_setzero_ps();
            let mut g2 = _mm256_setzero_ps();
            let mut g3 = _mm256_setzero_ps();
            let mut u0 = _mm256_setzero_ps();
            let mut u1 = _mm256_setzero_ps();
            let mut u2 = _mm256_setzero_ps();
            let mut u3 = _mm256_setzero_ps();

            let mut i = 0;
            while i + 31 < hidden_dim {
                let x0 = _mm256_loadu_ps(x_norm.as_ptr().add(i));
                let x1 = _mm256_loadu_ps(x_norm.as_ptr().add(i + 8));
                let x2 = _mm256_loadu_ps(x_norm.as_ptr().add(i + 16));
                let x3 = _mm256_loadu_ps(x_norm.as_ptr().add(i + 24));
                g0 = _mm256_fmadd_ps(_mm256_loadu_ps(gate_row.add(i)), x0, g0);
                g1 = _mm256_fmadd_ps(_mm256_loadu_ps(gate_row.add(i + 8)), x1, g1);
                g2 = _mm256_fmadd_ps(_mm256_loadu_ps(gate_row.add(i + 16)), x2, g2);
                g3 = _mm256_fmadd_ps(_mm256_loadu_ps(gate_row.add(i + 24)), x3, g3);
                u0 = _mm256_fmadd_ps(_mm256_loadu_ps(up_row.add(i)), x0, u0);
                u1 = _mm256_fmadd_ps(_mm256_loadu_ps(up_row.add(i + 8)), x1, u1);
                u2 = _mm256_fmadd_ps(_mm256_loadu_ps(up_row.add(i + 16)), x2, u2);
                u3 = _mm256_fmadd_ps(_mm256_loadu_ps(up_row.add(i + 24)), x3, u3);
                i += 32;
            }
            while i + 7 < hidden_dim {
                let x = _mm256_loadu_ps(x_norm.as_ptr().add(i));
                g0 = _mm256_fmadd_ps(_mm256_loadu_ps(gate_row.add(i)), x, g0);
                u0 = _mm256_fmadd_ps(_mm256_loadu_ps(up_row.add(i)), x, u0);
                i += 8;
            }

            let g = hsum256(_mm256_add_ps(_mm256_add_ps(g0, g1), _mm256_add_ps(g2, g3)));
            let u = hsum256(_mm256_add_ps(_mm256_add_ps(u0, u1), _mm256_add_ps(u2, u3)));

            let mut g_scalar = g;
            let mut u_scalar = u;
            for k in i..hidden_dim {
                let xk = *x_norm.as_ptr().add(k);
                g_scalar += *gate_row.add(k) * xk;
                u_scalar += *up_row.add(k) * xk;
            }

            let silu_g = g_scalar * (1.0 / (1.0 + (-g_scalar).exp()));
            *inter_slot = silu_g * u_scalar;
        }
    }
}

/// Horizontal sum of 8 f32 in a __m256.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn hsum256(v: __m256) -> f32 {
    let hi = _mm256_extractf128_ps(v, 1);
    let lo = _mm256_castps256_ps128(v);
    let sum128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf2 = _mm_movehl_ps(shuf, sums);
    let result = _mm_add_ss(sums, shuf2);
    _mm_cvtss_f32(result)
}

// ---------------------------------------------------------------------------
// Transpose matrix-vector multiply: output = matrix^T @ input
// matrix is [n_rows, n_cols] row-major. input is [n_rows]. output is [n_cols].
// output[col] = sum_{row} matrix[row * n_cols + col] * input[row]
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,fma")]
unsafe fn transpose_matvec_avx512(
    matrix: &[f32],
    input: &[f32],
    output: &mut [f32],
    n_rows: usize,
    n_cols: usize,
) {
    unsafe {
        // Transpose matmul: output[col] = sum_row matrix[row*n_cols+col] * input[row]
        // Strategy: maintain per-column SIMD accumulators (each __m512 holds 16 column sums).
        // For each row, broadcast input[row], FMA with 16 matrix columns at a time.
        let n_simd = n_cols / 16;
        let _remainder = n_cols % 16;

        let mut col_accs: Vec<__m512> = vec![_mm512_setzero_ps(); n_simd];

        for row in 0..n_rows {
            let b = _mm512_set1_ps(*input.get_unchecked(row));
            let row_start = row * n_cols;

            for (c, acc) in col_accs.iter_mut().enumerate().take(n_simd) {
                let w = _mm512_loadu_ps(matrix.as_ptr().add(row_start + c * 16));
                *acc = _mm512_fmadd_ps(w, b, *acc);
            }
        }

        // Store column accumulators — each element is a separate column sum
        for (c, acc) in col_accs.iter().enumerate().take(n_simd) {
            _mm512_storeu_ps(output.as_mut_ptr().add(c * 16), *acc);
        }

        // Scalar tail for remaining columns
        for (ci, out_slot) in output.iter_mut().enumerate().take(n_cols).skip(n_simd * 16) {
            let mut s = 0.0f32;
            for ri in 0..n_rows {
                s += *matrix.get_unchecked(ri * n_cols + ci) * *input.get_unchecked(ri);
            }
            *out_slot = s;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn transpose_matvec_avx2(
    matrix: &[f32],
    input: &[f32],
    output: &mut [f32],
    n_rows: usize,
    n_cols: usize,
) {
    unsafe {
        // Same broadcast-FMA approach as AVX-512 but with 8-float vectors
        let n_simd = n_cols / 8;
        let _remainder = n_cols % 8;

        let mut col_accs: Vec<__m256> = vec![_mm256_setzero_ps(); n_simd];

        for row in 0..n_rows {
            let b = _mm256_set1_ps(*input.get_unchecked(row));
            let row_start = row * n_cols;

            for (c, acc) in col_accs.iter_mut().enumerate().take(n_simd) {
                let w = _mm256_loadu_ps(matrix.as_ptr().add(row_start + c * 8));
                *acc = _mm256_fmadd_ps(w, b, *acc);
            }
        }

        for (c, acc) in col_accs.iter().enumerate().take(n_simd) {
            _mm256_storeu_ps(output.as_mut_ptr().add(c * 8), *acc);
        }

        for (ci, out_slot) in output.iter_mut().enumerate().take(n_cols).skip(n_simd * 8) {
            let mut s = 0.0f32;
            for ri in 0..n_rows {
                s += *matrix.get_unchecked(ri * n_cols + ci) * *input.get_unchecked(ri);
            }
            *out_slot = s;
        }
    }
}

fn transpose_matvec_scalar(
    matrix: &[f32],
    input: &[f32],
    output: &mut [f32],
    n_rows: usize,
    n_cols: usize,
) {
    for ci in 0..n_cols {
        let mut s = 0.0f32;
        for ri in 0..n_rows {
            s += matrix[ri * n_cols + ci] * input[ri];
        }
        output[ci] = s;
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

fn matmul_scalar(weight: &[f32], input: &[f32], output: &mut [f32], out_dim: usize, in_dim: usize) {
    for (j, out_slot) in output.iter_mut().enumerate().take(out_dim) {
        let mut sum = 0.0f32;
        let row_start = j * in_dim;
        for i in 0..in_dim {
            unsafe {
                sum += *weight.get_unchecked(row_start + i) * *input.get_unchecked(i);
            }
        }
        *out_slot = sum;
    }
}

fn ffn_gate_up_scalar(
    ffn_gate: &[f32],
    ffn_up: &[f32],
    x_norm: &[f32],
    intermediate: &mut [f32],
    n: usize,
    hidden_dim: usize,
) {
    for (j, inter_slot) in intermediate.iter_mut().enumerate().take(n) {
        let mut g = 0.0f32;
        let mut u = 0.0f32;
        let base = j * hidden_dim;
        for i in 0..hidden_dim {
            unsafe {
                let xi = *x_norm.get_unchecked(i);
                g += *ffn_gate.get_unchecked(base + i) * xi;
                u += *ffn_up.get_unchecked(base + i) * xi;
            }
        }
        let silu_g = g * (1.0 / (1.0 + (-g).exp()));
        *inter_slot = silu_g * u;
    }
}
