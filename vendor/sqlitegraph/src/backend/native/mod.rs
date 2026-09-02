//! Native backend module organization and minimal public interface.
//!
//! This module provides the native storage implementation for SQLiteGraph.
//! It implements file-based adjacency storage without SQLite dependency.
//! NO GraphBackend implementation is provided here - this is storage layer only.

// Module organization
pub mod constants;
pub mod cpu_tuning;
pub mod types;

// V3 B+Tree backend (Phase 63)
pub mod v3;

// Re-export public types for external use
pub use types::{
    CpuProfile, EdgeFlags, FileOffset, NativeBackendError, NativeEdgeId, NativeNodeId,
    NativeResult, NodeFlags, RecordSize,
};

pub use v3::V3Backend as NativeGraphBackend;

// V3 re-exports
pub use v3::{PersistentHeaderV3, V3_FORMAT_VERSION, V3_HEADER_SIZE, V3_MAGIC, V3Backend};
