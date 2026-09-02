//! SQLite-specific backend implementation for GraphBackend trait.
//!
//! This module provides the concrete SQLite backend implementation that wraps
//! SqliteGraph and implements the GraphBackend trait. It includes the main
//! SqliteGraphBackend type and all supporting types needed for SQLite operations.

// Module organization
mod impl_;
pub mod types;

#[cfg(test)]
mod pubsub_tests;

// Re-export the main backend implementation
pub use impl_::SqliteGraphBackend;

// Re-export public types for external use
#[allow(
    unused_imports,
    reason = "public backend API re-export is consumed externally even when unused inside this crate"
)]
pub use types::{BackendDirection, EdgeSpec, NeighborQuery, NodeSpec};
