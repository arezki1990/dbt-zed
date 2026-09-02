//! Utility functions and result types for native backend

use super::{NativeBackendError, NativeNodeId};

/// Calculate the file offset for a given node's slot
///
/// This helper centralizes the node slot offset calculation to prevent duplication
/// and ensure consistency across the codebase.
///
/// # Arguments
/// * `node_data_offset` - The base offset where node data begins in the file
/// * `node_id` - The 1-based node identifier
///
/// # Returns
/// The file offset where this node's slot begins
///
/// # Note
/// Node IDs are 1-based, so we subtract 1 to get the correct slot index.
/// Each node slot is 4KB (4096 bytes) in size.
#[inline]
pub fn node_slot_offset(node_data_offset: u64, node_id: NativeNodeId) -> u64 {
    debug_assert!(node_id > 0, "Node IDs must be positive (1-based)");
    node_data_offset + ((node_id - 1) as u64 * super::super::constants::node::NODE_SLOT_SIZE)
}

/// Result type alias for native backend operations
pub type NativeResult<T> = Result<T, NativeBackendError>;
