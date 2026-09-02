//! Cache-enabled fast-path for pattern matching.
//!
//! This module provides an optimized version of pattern matching that uses
//! adjacency cache as a fast-path while maintaining identical results
//! to the SQL-based implementation.

pub mod edge_validation;
pub mod fast_path_detection;
pub mod fast_path_execution;
#[cfg(test)]
mod tests;

pub use fast_path_execution::match_triples_fast;
