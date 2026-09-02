//! NodePage - Variable-size page storage for NodeRecordV3
//!
//! This module implements the NodePage structure for storing variable-size NodeRecordV3
//! records in a 4KB page. Uses delta/varint encoding for space efficiency.
//!
//! # Compression
//!
//! - Node IDs are delta-encoded from a base ID (saves ~4 bytes per node)
//! - Field counts/lengths use varint encoding (saves ~1-3 bytes per field)
//! - Nodes are packed contiguously without fixed slot boundaries

use super::record::NodeRecordV3;
use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::compression;
use crate::backend::native::v3::compression::delta::{decode_id_delta, encode_id_delta};
use crate::backend::native::v3::compression::varint::{
    decode_varint, decode_varint_u16, encode_varint, encode_varint_u16,
};
use crate::backend::native::v3::constants as v3_constants;

mod codec;
mod codec_support;
#[cfg(test)]
mod tests;

/// NodePage header and layout constants
pub mod constants {

    /// Page header size in bytes
    ///
    /// Layout:
    /// - page_id: 8 bytes (u64)
    /// - next_page_id: 8 bytes (u64, overflow link)
    /// - node_count: 2 bytes (u16)
    /// - used_bytes: 2 bytes (u16, actual bytes used in data region)
    /// - base_id: 8 bytes (i64, for delta encoding)
    /// - checksum: 4 bytes (u32)
    /// - reserved: 0 bytes (header exactly 32 bytes)
    pub const PAGE_HEADER_SIZE: usize = 32;

    /// Fixed metadata size from NodeRecordV3
    pub const FIXED_METADATA_SIZE: usize = 44;

    /// Maximum inline data size from NodeRecordV3
    pub const MAX_INLINE_DATA: usize = 64;

    /// Page ID offset within header
    pub const PAGE_ID_OFFSET: usize = 0;

    /// Next page ID offset (overflow link)
    pub const NEXT_PAGE_ID_OFFSET: usize = 8;

    /// Node count offset (u16)
    pub const NODE_COUNT_OFFSET: usize = 16;

    /// Used bytes offset (u16)
    pub const USED_BYTES_OFFSET: usize = 18;

    /// Base ID offset (i64, for delta encoding)
    pub const BASE_ID_OFFSET: usize = 20;

    /// Checksum offset (u32)
    pub const CHECKSUM_OFFSET: usize = 28;

    /// Total page size (4KB)
    pub const MAX_PAGE_SIZE: usize = 4096;

    /// Usable page size after header
    pub const USABLE_SIZE: usize = MAX_PAGE_SIZE - PAGE_HEADER_SIZE;

    /// Estimated fixed node slot size (conservative estimate for non-compressed records)
    /// 44 bytes metadata + 32 bytes average inline data = 76 bytes
    /// Rounded up to 80 bytes for safety
    pub const ESTIMATED_NODE_SLOT_SIZE: usize = 80;

    /// Fixed node capacity (conservative estimate)
    /// USABLE_SIZE (4064) / ESTIMATED_NODE_SLOT_SIZE (80) ≈ 50 nodes
    /// Using 50 as max capacity
    pub const MAX_NODE_CAPACITY: usize = USABLE_SIZE / ESTIMATED_NODE_SLOT_SIZE;

    // Field sizes
    pub const PAGE_ID_SIZE: usize = 8;
    pub const NEXT_PAGE_ID_SIZE: usize = 8;
    pub const NODE_COUNT_SIZE: usize = 2;
    pub const USED_BYTES_SIZE: usize = 2;
    pub const BASE_ID_SIZE: usize = 8;
    pub const CHECKSUM_SIZE: usize = 4;

    /// Minimum size of a compressed node record (varint encoded)
    /// - ID delta: 1 byte (varint, small delta)
    /// - flags: 4 bytes (fixed)
    /// - kind_offset: 1 byte (varint u16, small values)
    /// - name_offset: 1 byte (varint u16, small values)
    /// - data_len: 1 byte (varint u16, small values)
    /// - outgoing_cluster_offset: 1 byte (varint, small values)
    /// - outgoing_edge_count: 1 byte (varint u32, small values)
    /// - incoming_cluster_offset: 1 byte (varint, small values)
    /// - incoming_edge_count: 1 byte (varint u32, small values)
    ///
    /// Total: ~12 bytes minimum + inline data
    pub const MIN_COMPRESSED_RECORD_SIZE: usize = 12;
}

/// Re-export constants for convenience
pub use constants::{
    BASE_ID_OFFSET, ESTIMATED_NODE_SLOT_SIZE, MAX_NODE_CAPACITY, MAX_PAGE_SIZE,
    MIN_COMPRESSED_RECORD_SIZE, PAGE_HEADER_SIZE, USABLE_SIZE, USED_BYTES_OFFSET,
};

/// NodePage for storing variable-size NodeRecordV3 records
///
/// Pages store nodes with delta/varint compression for space efficiency.
/// Overflow pages are linked via next_page_id for large nodes.
///
/// # Block Locality (PROTOTYPE)
///
/// The `block_id` field is computed from `base_id` for block-aware caching:
/// - `block_id = (base_id - 1) / BLOCK_SIZE` where `BLOCK_SIZE = 128`
/// - This is in-memory metadata only, not persisted
/// - Used for block-aware cache eviction decisions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePage {
    /// Page ID for this page
    pub page_id: u64,

    /// Next page ID for overflow (0 if none)
    pub next_page_id: u64,

    /// Node records stored in this page
    pub nodes: Vec<NodeRecordV3>,

    /// Actual bytes used in data region (tracked for capacity)
    pub used_bytes: u16,

    /// Base ID for delta encoding (minimum ID in this page)
    pub base_id: i64,

    /// Page checksum for validation
    pub checksum: u32,

    /// Block ID for locality-aware caching (PROTOTYPE: computed, not persisted)
    ///
    /// This field is computed from `base_id` after unpacking the page.
    /// It allows the cache to make block-aware retention decisions.
    /// NOT persisted to disk - recomputed on each page load.
    pub block_id: i64,
}

/// Block size for locality calculations
///
/// Each logical block contains approximately 128 node IDs.
/// Nodes 1-127 → block 0, nodes 128-255 → block 1, etc.
pub const BLOCK_SIZE: i64 = 128;

/// Compute block_id from a node_id
#[inline]
pub const fn node_id_to_block(node_id: i64) -> i64 {
    if node_id < 1 {
        return 0;
    }
    (node_id - 1) / BLOCK_SIZE
}

impl NodePage {
    /// Create a new empty node page
    pub fn new(page_id: u64) -> Self {
        NodePage {
            page_id,
            next_page_id: 0,
            nodes: Vec::new(),
            used_bytes: 0,
            base_id: 0,
            checksum: 0,
            block_id: 0, // Will be computed when nodes are added
        }
    }

    /// Create a new node page with the given capacity pre-allocated
    pub fn with_capacity(page_id: u64, capacity: usize) -> Self {
        NodePage {
            page_id,
            next_page_id: 0,
            nodes: Vec::with_capacity(capacity.min(MAX_NODE_CAPACITY)),
            used_bytes: 0,
            base_id: 0,
            checksum: 0,
            block_id: 0,
        }
    }

    /// Get the block_id for this page
    ///
    /// For pages with nodes, computes based on base_id.
    /// For empty pages, returns 0.
    pub fn block_id(&self) -> i64 {
        if self.block_id != 0 {
            self.block_id
        } else {
            node_id_to_block(self.base_id)
        }
    }

    /// Recompute block_id from current nodes
    ///
    /// Updates block_id based on the minimum node_id in this page.
    /// Called after adding nodes or unpacking from disk.
    pub fn recompute_block_id(&mut self) {
        if let Some(min_id) = self.nodes.iter().map(|n| n.id()).min() {
            self.base_id = min_id;
            self.block_id = node_id_to_block(min_id);
        } else {
            self.base_id = 0;
            self.block_id = 0;
        }
    }

    /// Get the number of nodes in this page
    pub fn node_count(&self) -> u16 {
        self.nodes.len() as u16
    }

    /// Check if the page has an overflow page
    pub fn has_overflow(&self) -> bool {
        self.next_page_id != 0
    }

    /// Check if the page is full (at estimated capacity)
    pub fn is_full(&self) -> bool {
        // Check if adding an average-sized node would exceed usable size
        (self.capacity() as usize) < ESTIMATED_NODE_SLOT_SIZE
    }

    /// Check if the page is empty
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Calculate the actual used size in bytes (from tracked value)
    pub fn used_size(&self) -> usize {
        self.used_bytes as usize
    }

    /// Calculate remaining capacity in bytes
    pub fn remaining_capacity(&self) -> usize {
        USABLE_SIZE.saturating_sub(self.used_bytes as usize)
    }

    /// Calculate remaining capacity as u16 (for internal use)
    pub fn capacity(&self) -> u16 {
        (USABLE_SIZE as u16).saturating_sub(self.used_bytes)
    }

    /// Find a node by ID using binary search.
    ///
    /// # Panics
    /// Panics if nodes are not sorted by ID. This is guaranteed by the insertion
    /// order (sequential node_id allocation) and page filling strategy.
    ///
    /// # Returns
    /// * `Some(&node_record)` if found
    /// * `None` if not found in this page
    #[inline]
    pub fn find_node(&self, node_id: i64) -> Option<&NodeRecordV3> {
        // Binary search on sorted node IDs
        // Nodes are guaranteed sorted by sequential insertion
        let mut left = 0;
        let mut right = self.nodes.len();

        while left < right {
            let mid = left + (right - left) / 2;
            let mid_id = self.nodes[mid].id();

            if mid_id == node_id {
                return Some(&self.nodes[mid]);
            } else if mid_id < node_id {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        // Check the last element
        if left < self.nodes.len() && self.nodes[left].id() == node_id {
            Some(&self.nodes[left])
        } else {
            None
        }
    }

    /// Add a node to the page
    ///
    /// Returns an error if the node would cause the page to overflow.
    /// Uses compressed size calculation with delta/varint encoding.
    pub fn add_node(&mut self, node: NodeRecordV3) -> NativeResult<()> {
        // Temporarily add node to get accurate base_id
        let temp_base_id = if self.nodes.is_empty() {
            node.id()
        } else {
            self.base_id.min(node.id())
        };

        // Calculate compressed size using the temp base_id
        let compressed_size = self.estimate_compressed_size_with_base(&node, temp_base_id)?;

        // Check if adding this node would exceed page capacity
        if self.capacity() < compressed_size {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "adding node would exceed page capacity: need {} bytes, have {} remaining",
                    compressed_size,
                    self.capacity()
                ),
            });
        }

        // Update base_id
        self.base_id = temp_base_id;

        self.nodes.push(node);
        self.used_bytes += compressed_size;

        // CRITICAL VALIDATION: Verify the actual packed size fits in USABLE_SIZE
        // This catches any discrepancy between estimated size and actual packed size
        let actual_packed_size = self.pack_nodes()?.len();
        if actual_packed_size > USABLE_SIZE {
            // Rollback: remove the node we just added
            self.nodes.pop();
            self.used_bytes -= compressed_size;
            // Restore previous base_id if we had nodes
            if !self.nodes.is_empty() {
                self.base_id = self
                    .nodes
                    .iter()
                    .map(|n| n.id())
                    .min()
                    .unwrap_or(self.base_id);
            }

            // Provide detailed diagnostics for debugging
            #[cfg(feature = "v3-page-overflow-debug")]
            {
                eprintln!(
                    "Page {} overflow: node_count={}, used_bytes={}, actual_packed_size={}, USABLE_SIZE={}, estimated={}",
                    self.page_id,
                    self.nodes.len() + 1, // including the failed node
                    self.used_bytes + compressed_size,
                    actual_packed_size,
                    USABLE_SIZE,
                    compressed_size
                );
            }

            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "page {} full: node_count={}, used_bytes={}, actual packed {} exceeds USABLE_SIZE {} (new node estimated {})",
                    self.page_id,
                    self.nodes.len(),
                    self.used_bytes,
                    actual_packed_size,
                    USABLE_SIZE,
                    compressed_size
                ),
            });
        }

        // Update used_bytes to actual packed size for accuracy
        self.used_bytes = actual_packed_size as u16;

        // Recompute block_id after adding node (PROTOTYPE: block-aware caching)
        self.block_id = node_id_to_block(self.base_id);

        Ok(())
    }

    /// Estimate the compressed size of a node record with a specific base_id
    ///
    /// This is used by add_node to calculate size before updating base_id.
    fn estimate_compressed_size_with_base(
        &self,
        node: &NodeRecordV3,
        base_id: i64,
    ) -> NativeResult<u16> {
        let mut size: usize = 0;

        // ID delta (varint, usually 1-4 bytes)
        let delta = encode_id_delta(node.id(), base_id);
        size += compression::varint::varint_size(delta as u64);

        // Flags: 4 bytes (fixed)
        size += 4;

        // kind_offset: varint u16 (usually 1-2 bytes)
        size += compression::varint::varint_size(node.kind_offset as u64);

        // name_offset: varint u16 (usually 1-2 bytes)
        size += compression::varint::varint_size(node.name_offset as u64);

        // data_len: varint u16 (usually 1 byte for small data)
        size += compression::varint::varint_size(node.data_len() as u64);

        // outgoing_cluster_offset: varint u64 (1-10 bytes)
        size += compression::varint::varint_size(node.outgoing_cluster_offset);

        // outgoing_edge_count: varint u32 (usually 1-3 bytes)
        size += compression::varint::varint_size(node.outgoing_edge_count as u64);

        // incoming_cluster_offset: varint u64 (1-10 bytes)
        size += compression::varint::varint_size(node.incoming_cluster_offset);

        // incoming_edge_count: varint u32 (usually 1-3 bytes)
        size += compression::varint::varint_size(node.incoming_edge_count as u64);

        // Inline data OR external offset (8 bytes)
        if let Some(ref data) = node.data_inline {
            size += data.len();
        } else if node.data_external_offset.is_some() {
            size += 8; // External offset is u64 (8 bytes)
        }

        // Ensure we don't overflow u16
        if size > u16::MAX as usize {
            return Err(NativeBackendError::InvalidHeader {
                field: "compressed_size".to_string(),
                reason: format!("compressed size {} exceeds u16::MAX", size),
            });
        }

        Ok(size as u16)
    }

    /// Get the total size this page would consume on disk
    pub fn disk_size(&self) -> usize {
        MAX_PAGE_SIZE
    }

    /// Calculate space efficiency (ratio of used to total space)
    pub fn space_efficiency(&self) -> f64 {
        if MAX_PAGE_SIZE == 0 {
            return 0.0;
        }
        (self.used_size() as f64) / (USABLE_SIZE as f64)
    }
}

/// Create a new empty page with default capacity
impl Default for NodePage {
    fn default() -> Self {
        Self::new(0)
    }
}
