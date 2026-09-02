//! Graph-Based Inference Engine
//!
//! Replaces dense transformer forward pass with dense FFN + HNSW graph attention.
//!
//! Architecture:
//!   Dense FFN: SiLU-gated matrix multiplication (boring, fast, correct)
//!   Attention: HNSW index per layer per KV head stores K/V vectors.
//!     Query searches for top-K similar past keys via dot product.
//!     Softmax over similarities -> weighted sum of values.
//!     No O(n^2) cost, no context window limit.
//!
//! The graph IS the memory: no context window, no KV cache truncation.
//! HNSW indices grow with each token. Hebbian forgetting (future) prunes old entries.

mod engine;
pub mod sampling;
pub mod simd;

pub use engine::{InferenceConfig, InferenceStats, SparseInferenceEngine};
pub use sampling::sample_token;
