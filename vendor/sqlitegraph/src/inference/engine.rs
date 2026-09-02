//! Graph-Based Inference Engine — dense FFN + HNSW attention.
//!
//! Phase 2: replaces the Phase 1 HNSW sparse FFN with:
//!   - Dense FFN matmul (gate, up, down projections — full activation)
//!   - Graph-based attention via HNSW per-layer per-KV-head indices
//!
//! Architecture per token:
//!   1. Embed token → hidden state
//!   2. For each layer:
//!      a. RMSNorm (attention)
//!      b. Q/K/V projections + RoPE
//!      c. Store K/V in per-layer HNSW indices
//!      d. HNSW search Q → top-K past K vectors (replaces O(n²) attention)
//!      e. Softmax over dot-product similarities → attention weights
//!      f. Weighted sum of V vectors → attention output
//!      g. Output projection + residual
//!      h. RMSNorm (FFN)
//!      i. Dense FFN (SiLU-gated) + residual
//!   3. Final norm → logits → sample
//!
//! The graph IS the memory: no context window limit, no KV cache truncation.
//! HNSW indices grow with each token. Hebbian forgetting (future) prunes old entries.

use crate::hnsw::{HnswIndex, config::HnswConfig, distance_metric::DistanceMetric};
use crate::inference::sampling::sample_token;
use crate::inference::simd;

/// Configuration for the graph inference engine.
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    /// Temperature for sampling (default: 0.8)
    pub temperature: f32,

    /// Top-p for nucleus sampling (default: 0.9)
    pub top_p: f32,

    /// How many past tokens to attend to per head (default: 256).
    /// HNSW returns min(attn_top_k, n_stored) results.
    pub attn_top_k: usize,

    /// RoPE base frequency (default: 1000000.0 for Qwen2)
    pub rope_base: f32,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            temperature: 0.8,
            top_p: 0.9,
            attn_top_k: 256,
            rope_base: 1000000.0,
        }
    }
}

/// Statistics from a generation run.
#[derive(Debug, Clone)]
pub struct InferenceStats {
    /// Number of prompt tokens processed
    pub prompt_tokens_processed: usize,

    /// Number of tokens generated
    pub tokens_generated: usize,

    /// Total wall-clock time in seconds
    pub total_time_s: f64,

    /// Tokens per second (generated tokens only)
    pub tokens_per_sec: f64,

    /// Average time per token in milliseconds
    pub avg_token_time_ms: f64,

    /// Time for first generated token in milliseconds
    pub first_token_ms: f64,

    /// Average time per layer in milliseconds
    pub avg_layer_time_ms: f64,

    /// Average attention time per layer in milliseconds
    pub avg_attn_time_ms: f64,

    /// Average FFN time per layer in milliseconds
    pub avg_ffn_time_ms: f64,
}

/// Per-layer weights and attention state.
struct LayerData {
    /// Pre-attention RMSNorm weights [hidden_dim]
    attn_norm: Vec<f32>,

    /// FFN RMSNorm weights [hidden_dim]
    ffn_norm: Vec<f32>,

    // ── Attention weights (row-major: [out_dim, in_dim]) ──────
    /// Query projection [hidden_dim, hidden_dim]
    wq: Vec<f32>,

    /// Key projection [n_kv_dim, hidden_dim]
    wk: Vec<f32>,

    /// Value projection [n_kv_dim, hidden_dim]
    wv: Vec<f32>,

    /// Output projection [hidden_dim, hidden_dim]
    wo: Vec<f32>,

    // ── Attention biases (optional — Qwen2 GGUF stores them) ──
    /// Query bias [hidden_dim] or empty
    bq: Vec<f32>,

    /// Key bias [n_kv_dim] or empty
    bk: Vec<f32>,

    /// Value bias [n_kv_dim] or empty
    bv: Vec<f32>,

    // ── FFN weights (row-major: [n_neurons, hidden_dim]) ──────
    /// Gate weights [ffn_dim, hidden_dim]
    ffn_gate: Vec<f32>,

    /// Up weights [ffn_dim, hidden_dim]
    ffn_up: Vec<f32>,

    /// Down weights [ffn_dim, hidden_dim] (transposed from GGUF [hidden_dim, ffn_dim])
    ffn_down: Vec<f32>,

    // ── Attention state ───────────────────────────────────────
    /// One HNSW index per KV head — stores K vectors for graph attention.
    attn_hnsw: Vec<HnswIndex>,

    /// One flat buffer per KV head — stores V vectors alongside HNSW.
    /// Indexed as [token_position * head_dim .. (token_position + 1) * head_dim].
    attn_v_store: Vec<Vec<f32>>,

    /// Number of tokens stored in this layer's attention indices.
    attn_n_stored: usize,
}

/// The graph-based inference engine.
///
/// Holds all model weights in memory and runs the generation loop
/// entirely in Rust. Dense FFN replaces sparse HNSW FFN.
/// HNSW indices per layer per KV head replace O(n²) attention.
///
/// Usage:
/// ```ignore
/// let mut engine = SparseInferenceEngine::new(config);
/// engine.set_model_info(n_layers, hidden_dim, ffn_dim, vocab_size, n_heads, n_kv_heads);
/// for layer in 0..n_layers {
///     engine.load_layer(layer, attn_norm, ffn_norm, wq, wk, wv, wo, ffn_gate, ffn_up, ffn_down);
/// }
/// engine.set_root_weights(token_embd, output_proj, output_norm);
/// let (tokens, stats) = engine.generate(&prompt_ids, max_tokens);
/// ```
pub struct SparseInferenceEngine {
    config: InferenceConfig,

    // Model architecture
    n_layers: usize,
    hidden_dim: usize,
    ffn_dim: usize,
    vocab_size: usize,
    n_heads: usize,
    n_kv_heads: usize,
    head_dim: usize,

    // Per-layer data
    layers: Vec<LayerData>,

    // Root weights
    /// Token embedding [vocab_size, hidden_dim]
    token_embd: Vec<f32>,
    /// Output projection [vocab_size, hidden_dim]
    output_proj: Vec<f32>,
    /// Output RMSNorm [hidden_dim]
    output_norm: Vec<f32>,
}

impl SparseInferenceEngine {
    /// Create a new inference engine with the given configuration.
    pub fn new(config: InferenceConfig) -> Self {
        Self {
            config,
            n_layers: 0,
            hidden_dim: 0,
            ffn_dim: 0,
            vocab_size: 0,
            n_heads: 0,
            n_kv_heads: 0,
            head_dim: 0,
            layers: Vec::new(),
            token_embd: Vec::new(),
            output_proj: Vec::new(),
            output_norm: Vec::new(),
        }
    }

    /// Set model architecture info.
    pub fn set_model_info(
        &mut self,
        n_layers: usize,
        hidden_dim: usize,
        ffn_dim: usize,
        vocab_size: usize,
        n_heads: usize,
        n_kv_heads: usize,
    ) {
        self.n_layers = n_layers;
        self.hidden_dim = hidden_dim;
        self.ffn_dim = ffn_dim;
        self.vocab_size = vocab_size;
        self.n_heads = n_heads;
        self.n_kv_heads = n_kv_heads;
        self.head_dim = hidden_dim / n_heads;
        self.layers.reserve(n_layers);
    }

    /// Load a single layer's weights.
    ///
    /// # Arguments
    /// * `layer_idx` - Layer index (0-based)
    /// * `attn_norm` - Pre-attention RMSNorm [hidden_dim]
    /// * `ffn_norm` - FFN RMSNorm [hidden_dim]
    /// * `wq` - Query projection [hidden_dim, hidden_dim]
    /// * `wk` - Key projection [n_kv_dim, hidden_dim]
    /// * `wv` - Value projection [n_kv_dim, hidden_dim]
    /// * `wo` - Output projection [hidden_dim, hidden_dim]
    /// * `ffn_gate` - Gate weights [ffn_dim, hidden_dim]
    /// * `ffn_up` - Up weights [ffn_dim, hidden_dim]
    /// * `ffn_down` - Down weights [ffn_dim, hidden_dim]
    /// * `bq` - Query bias [hidden_dim] or empty slice
    /// * `bk` - Key bias [n_kv_dim] or empty slice
    /// * `bv` - Value bias [n_kv_dim] or empty slice
    #[allow(
        clippy::too_many_arguments,
        reason = "transformer layer weights are an inherently wide signature; \
                  a config struct would shuffle the same data without payoff"
    )]
    pub fn load_layer(
        &mut self,
        layer_idx: usize,
        attn_norm: &[f32],
        ffn_norm: &[f32],
        wq: &[f32],
        wk: &[f32],
        wv: &[f32],
        wo: &[f32],
        ffn_gate: &[f32],
        ffn_up: &[f32],
        ffn_down: &[f32],
        bq: &[f32],
        bk: &[f32],
        bv: &[f32],
    ) {
        let head_dim = self.head_dim;

        // Create HNSW indices for graph attention (one per KV head)
        let mut attn_hnsw = Vec::with_capacity(self.n_kv_heads);
        let mut attn_v_store = Vec::with_capacity(self.n_kv_heads);
        for kv_h in 0..self.n_kv_heads {
            let hnsw = HnswIndex::new(
                &format!("attn_L{}_kv{}", layer_idx, kv_h),
                HnswConfig::new(head_dim, 8, 50, DistanceMetric::DotProduct),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to create attention HNSW for layer {} kv {}: {}",
                    layer_idx, kv_h, e
                )
            });
            attn_hnsw.push(hnsw);
            attn_v_store.push(Vec::new());
        }

        let layer = LayerData {
            attn_norm: attn_norm.to_vec(),
            ffn_norm: ffn_norm.to_vec(),
            wq: wq.to_vec(),
            wk: wk.to_vec(),
            wv: wv.to_vec(),
            wo: wo.to_vec(),
            bq: bq.to_vec(),
            bk: bk.to_vec(),
            bv: bv.to_vec(),
            ffn_gate: ffn_gate.to_vec(),
            ffn_up: ffn_up.to_vec(),
            ffn_down: ffn_down.to_vec(),
            attn_hnsw,
            attn_v_store,
            attn_n_stored: 0,
        };

        if layer_idx == self.layers.len() {
            self.layers.push(layer);
        } else if layer_idx < self.layers.len() {
            self.layers[layer_idx] = layer;
        } else {
            panic!(
                "load_layer called out of order: expected layer {}, got {}",
                self.layers.len(),
                layer_idx
            );
        }
    }

    /// Set root model weights.
    pub fn set_root_weights(
        &mut self,
        token_embd: &[f32],
        output_proj: &[f32],
        output_norm: &[f32],
    ) {
        self.token_embd = token_embd.to_vec();
        self.output_proj = output_proj.to_vec();
        self.output_norm = output_norm.to_vec();
    }

    /// Generate tokens using graph-based inference.
    ///
    /// Processes all prompt tokens through all layers (building attention state),
    /// then generates new tokens autoregressively.
    pub fn generate(
        &mut self,
        prompt_tokens: &[u64],
        max_tokens: usize,
    ) -> (Vec<u64>, InferenceStats) {
        let t_start = std::time::Instant::now();

        let hidden_dim = self.hidden_dim;
        let vocab_size = self.vocab_size;
        let n_layers = self.n_layers;

        let mut x = vec![0.0f32; hidden_dim];
        let mut pos = 0usize;

        // Phase 1: Process prompt tokens through all layers.
        // This builds up the per-layer attention state (K/V in HNSW).
        for &token_id in prompt_tokens {
            x = embed_token(&self.token_embd, token_id as usize, hidden_dim);
            for layer_idx in 0..n_layers {
                self.forward_layer(&mut x, layer_idx, pos);
            }
            pos += 1;
        }

        // Phase 2: Generate new tokens autoregressively.
        let mut generated = Vec::with_capacity(max_tokens);
        let mut first_token_ms = 0.0f64;
        let mut total_layer_us: u64 = 0;
        let mut total_attn_us: u64 = 0;
        let mut total_ffn_us: u64 = 0;

        for step in 0..max_tokens {
            let tok_start = std::time::Instant::now();

            // Final norm + logits + sample
            let x_final = rms_norm(&x, &self.output_norm);
            let logits = project_to_logits(&self.output_proj, &x_final, vocab_size, hidden_dim);
            let next_token = sample_token(&logits, self.config.temperature, self.config.top_p);
            generated.push(next_token as u64);

            if step == 0 {
                first_token_ms = tok_start.elapsed().as_secs_f64() * 1000.0;
            }

            // Embed next token and forward through all layers
            x = embed_token(&self.token_embd, next_token, hidden_dim);
            for layer_idx in 0..n_layers {
                let layer_start = std::time::Instant::now();
                let (attn_us, ffn_us) = self.forward_layer_timed(&mut x, layer_idx, pos);
                total_layer_us += layer_start.elapsed().as_micros() as u64;
                total_attn_us += attn_us;
                total_ffn_us += ffn_us;
            }
            pos += 1;
        }

        let total_elapsed = t_start.elapsed().as_secs_f64();
        let _total_gen_tokens = prompt_tokens.len() + generated.len();

        let stats = InferenceStats {
            prompt_tokens_processed: prompt_tokens.len(),
            tokens_generated: generated.len(),
            total_time_s: total_elapsed,
            tokens_per_sec: if total_elapsed > 0.0 {
                generated.len() as f64 / total_elapsed
            } else {
                0.0
            },
            avg_token_time_ms: if max_tokens > 0 {
                total_elapsed * 1000.0 / max_tokens as f64
            } else {
                0.0
            },
            first_token_ms,
            avg_layer_time_ms: if n_layers > 0 && max_tokens > 0 {
                total_layer_us as f64 / 1000.0 / (n_layers as f64 * max_tokens as f64)
            } else {
                0.0
            },
            avg_attn_time_ms: if n_layers > 0 && max_tokens > 0 {
                total_attn_us as f64 / 1000.0 / (n_layers as f64 * max_tokens as f64)
            } else {
                0.0
            },
            avg_ffn_time_ms: if n_layers > 0 && max_tokens > 0 {
                total_ffn_us as f64 / 1000.0 / (n_layers as f64 * max_tokens as f64)
            } else {
                0.0
            },
        };

        (generated, stats)
    }

    /// Forward a single token through one layer (attention + FFN).
    fn forward_layer(&mut self, x: &mut [f32], layer_idx: usize, pos: usize) {
        let (attn_us, ffn_us) = self.forward_layer_timed(x, layer_idx, pos);
        // Discard timing in non-timed path
        let _ = (attn_us, ffn_us);
    }

    /// Forward a single token through one layer, returning timing.
    fn forward_layer_timed(&mut self, x: &mut [f32], layer_idx: usize, pos: usize) -> (u64, u64) {
        let hidden_dim = self.hidden_dim;
        let n_heads = self.n_heads;
        let n_kv_heads = self.n_kv_heads;
        let head_dim = self.head_dim;
        let n_kv_dim = n_kv_heads * head_dim;
        let attn_top_k = self.config.attn_top_k;

        // ════════════════════════════════════════════════════════
        // ATTENTION (graph-based via HNSW)
        // ════════════════════════════════════════════════════════

        let attn_start = std::time::Instant::now();

        // 1. Pre-attention RMSNorm
        let x_attn = rms_norm(x, &self.layers[layer_idx].attn_norm);

        // 2. Q/K/V projections (weight @ input + bias)
        let mut q = simd::matmul(&self.layers[layer_idx].wq, &x_attn, hidden_dim, hidden_dim);
        let mut k_proj = simd::matmul(&self.layers[layer_idx].wk, &x_attn, n_kv_dim, hidden_dim);
        let mut v_proj = simd::matmul(&self.layers[layer_idx].wv, &x_attn, n_kv_dim, hidden_dim);

        // Add attention biases if present
        let layer = &self.layers[layer_idx];
        for (i, slot) in q
            .iter_mut()
            .enumerate()
            .take(layer.bq.len().min(hidden_dim))
        {
            *slot += layer.bq[i];
        }
        for (i, slot) in k_proj
            .iter_mut()
            .enumerate()
            .take(layer.bk.len().min(n_kv_dim))
        {
            *slot += layer.bk[i];
        }
        for (i, slot) in v_proj
            .iter_mut()
            .enumerate()
            .take(layer.bv.len().min(n_kv_dim))
        {
            *slot += layer.bv[i];
        }

        // 3. Apply RoPE to Q (per head) and K (per KV head)
        let mut k = k_proj;
        for h in 0..n_heads {
            apply_rope(
                &mut q[h * head_dim..(h + 1) * head_dim],
                head_dim,
                pos,
                self.config.rope_base,
            );
        }
        for h in 0..n_kv_heads {
            apply_rope(
                &mut k[h * head_dim..(h + 1) * head_dim],
                head_dim,
                pos,
                self.config.rope_base,
            );
        }

        // 4. Store K and V for future attention (per KV head)
        for kv_h in 0..n_kv_heads {
            let k_slice = k[kv_h * head_dim..(kv_h + 1) * head_dim].to_vec();
            let v_slice = &v_proj[kv_h * head_dim..(kv_h + 1) * head_dim];
            self.layers[layer_idx].attn_hnsw[kv_h]
                .insert_vector(&k_slice, None)
                .unwrap_or_else(|e| {
                    panic!(
                        "HNSW insert failed at layer {} kv {}: {}",
                        layer_idx, kv_h, e
                    )
                });
            self.layers[layer_idx].attn_v_store[kv_h].extend_from_slice(v_slice);
        }
        self.layers[layer_idx].attn_n_stored += 1;

        // 5. Graph attention: HNSW search per Q head, weighted sum of V
        let heads_ratio = n_heads / n_kv_heads;
        let mut attn_concat = vec![0.0f32; hidden_dim];

        // Read V store once to avoid repeated borrow conflicts
        // (We need the HNSW search results first, then look up V vectors)
        let v_stores: Vec<&Vec<f32>> = self.layers[layer_idx].attn_v_store.iter().collect();

        for q_head in 0..n_heads {
            let kv_head = q_head / heads_ratio;
            let q_vec = &q[q_head * head_dim..(q_head + 1) * head_dim];

            // HNSW search: find top-K past K vectors most similar to Q
            let results = self.layers[layer_idx].attn_hnsw[kv_head]
                .search(q_vec, attn_top_k)
                .unwrap_or_default();

            if results.is_empty() {
                continue;
            }

            // Convert HNSW dot-product distances to attention scores.
            // HNSW distance = -dot(Q, K), so dot(Q, K) = -distance.
            // Standard attention: score = dot(Q, K) / sqrt(head_dim)
            let inv_sqrt_d = 1.0 / (head_dim as f32).sqrt();
            let mut scores: Vec<f32> = results.iter().map(|(_, dist)| -dist * inv_sqrt_d).collect();

            // Softmax over scores
            softmax(&mut scores);

            // Weighted sum of V vectors
            let v_store = v_stores[kv_head];
            let out_start = q_head * head_dim;
            for (idx, &score) in scores.iter().enumerate() {
                let v_id = results[idx].0 as usize; // 1-based HNSW ID
                let v_start = (v_id - 1) * head_dim;
                for d in 0..head_dim {
                    unsafe {
                        attn_concat[out_start + d] += score * *v_store.get_unchecked(v_start + d);
                    }
                }
            }
        }

        // 6. Attention output projection + residual
        let attn_out = simd::matmul(
            &self.layers[layer_idx].wo,
            &attn_concat,
            hidden_dim,
            hidden_dim,
        );
        for (x_slot, &a_val) in x.iter_mut().zip(attn_out.iter()).take(hidden_dim) {
            *x_slot += a_val;
        }

        let attn_us = attn_start.elapsed().as_micros() as u64;

        // ════════════════════════════════════════════════════════
        // FFN (dense matmul)
        // ════════════════════════════════════════════════════════

        let ffn_start = std::time::Instant::now();

        // 1. FFN RMSNorm
        let x_ffn = rms_norm(x, &self.layers[layer_idx].ffn_norm);

        // 2. Dense FFN: SiLU(gate @ x) * (up @ x) → down^T @ intermediate
        let ffn_out = simd::dense_ffn(
            &self.layers[layer_idx].ffn_gate,
            &self.layers[layer_idx].ffn_up,
            &self.layers[layer_idx].ffn_down,
            &x_ffn,
            hidden_dim,
        );

        // 3. Residual
        for (x_slot, &f_val) in x.iter_mut().zip(ffn_out.iter()).take(hidden_dim) {
            *x_slot += f_val;
        }

        let ffn_us = ffn_start.elapsed().as_micros() as u64;

        (attn_us, ffn_us)
    }
}

// ════════════════════════════════════════════════════════════════
// Standalone helper functions (no &self, avoids borrow conflicts)
// ════════════════════════════════════════════════════════════════

/// RMSNorm: x * w / sqrt(mean(x²) + eps)
#[inline]
fn rms_norm(x: &[f32], weight: &[f32]) -> Vec<f32> {
    let dim = x.len();
    let eps = 1e-6f32;
    let sum_sq: f32 = x.iter().map(|&v| v * v).sum();
    let inv_rms = 1.0 / ((sum_sq / dim as f32 + eps).sqrt());
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        out.push(x[i] * inv_rms * weight[i]);
    }
    out
}

/// Apply Rotary Position Embeddings (RoPE) to a single head vector.
///
/// Uses neox/half-rotated pairing (Qwen2 style):
///   (v[0], v[half]), (v[1], v[half+1]), ...
/// Each pair is rotated by position-dependent angle.
fn apply_rope(vec: &mut [f32], head_dim: usize, pos: usize, rope_base: f32) {
    let half = head_dim / 2;
    for i in 0..half {
        let freq = rope_base.powf(-2.0 * i as f32 / head_dim as f32);
        let angle = pos as f32 * freq;
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        let v0 = vec[i];
        let v1 = vec[i + half];
        vec[i] = v0 * cos_a - v1 * sin_a;
        vec[i + half] = v0 * sin_a + v1 * cos_a;
    }
}

/// Numerically stable softmax in-place.
fn softmax(scores: &mut [f32]) {
    if scores.is_empty() {
        return;
    }
    let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for s in scores.iter_mut() {
        *s = (*s - max).exp();
        sum += *s;
    }
    if sum > 0.0 {
        for s in scores.iter_mut() {
            *s /= sum;
        }
    }
}

/// Embed a token ID to a hidden state vector.
fn embed_token(token_embd: &[f32], token_id: usize, hidden_dim: usize) -> Vec<f32> {
    if token_id * hidden_dim + hidden_dim > token_embd.len() {
        // Out of vocab: zero vector
        return vec![0.0f32; hidden_dim];
    }
    let start = token_id * hidden_dim;
    token_embd[start..start + hidden_dim].to_vec()
}

/// Project hidden state to vocabulary logits — delegates to SIMD module.
/// output_proj is [vocab_size, hidden_dim], so logits[i] = dot(row_i, x).
fn project_to_logits(
    output_proj: &[f32],
    x: &[f32],
    vocab_size: usize,
    hidden_dim: usize,
) -> Vec<f32> {
    simd::matmul(output_proj, x, vocab_size, hidden_dim)
}
