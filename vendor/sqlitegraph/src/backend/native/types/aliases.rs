//! Basic type aliases for native backend

/// Native node identifier (alias for i64 to match existing NodeId)
pub type NativeNodeId = i64;

/// Native edge identifier (alias for i64 to match existing EdgeId)
pub type NativeEdgeId = i64;

/// File offset within the graph database file
pub type FileOffset = u64;

/// Size of variable-length records
pub type RecordSize = u32;
