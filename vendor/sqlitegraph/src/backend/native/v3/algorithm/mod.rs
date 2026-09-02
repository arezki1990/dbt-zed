//! Parallel graph algorithms for V3 backend
//!
//! Multi-threaded implementations using Rayon for improved performance
//! on multi-core systems (2-4× speedup expected).

pub mod parallel_bfs;

pub use parallel_bfs::{BfsConfig, parallel_bfs};
