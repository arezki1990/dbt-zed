//! NodeRecordV3 - Simplified node record with full ID encoding
//!
//! This module defines the V3 node record format using full ID encoding.
//! Delta/varint compression is deferred to Phase 63b.

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::types::NodeFlags;

mod codec_support;
#[cfg(test)]
mod tests;

/// Record layout constants
pub mod constants {
    /// Fixed metadata size in bytes
    ///
    /// Layout:
    /// - id: 8 bytes (i64, full encoding - no delta)
    /// - flags: 4 bytes (u32)
    /// - kind_offset: 2 bytes (u16, string table offset)
    /// - name_offset: 2 bytes (u16, string table offset)
    /// - data_len: 2 bytes (u16)
    /// - outgoing_cluster_offset: 8 bytes (u64)
    /// - outgoing_edge_count: 4 bytes (u32)
    /// - incoming_cluster_offset: 8 bytes (u64)
    /// - incoming_edge_count: 4 bytes (u32)
    ///
    /// Total: 8 + 4 + 2 + 2 + 2 + 8 + 4 + 8 + 4 = 42 bytes + 2 reserved = 44
    pub const FIXED_METADATA_SIZE: usize = 44;

    /// Maximum inline data size in bytes
    ///
    /// Data <= 64 bytes is stored inline in the record.
    /// Data > 64 bytes is stored externally with a reference.
    pub const MAX_INLINE_DATA: usize = 64;

    // Field offsets within fixed metadata

    /// Node ID offset (i64, full encoding)
    pub const ID_OFFSET: usize = 0;

    /// Flags offset
    pub const FLAGS_OFFSET: usize = 8;

    /// Kind string offset (string table)
    pub const KIND_OFFSET: usize = 12;

    /// Name string offset (string table)
    pub const NAME_OFFSET: usize = 14;

    /// Data length offset
    pub const DATA_LEN_OFFSET: usize = 16;

    /// Outgoing cluster offset
    pub const OUTGOING_CLUSTER_OFFSET: usize = 18;

    /// Outgoing edge count offset
    pub const OUTGOING_COUNT_OFFSET: usize = 26;

    /// Incoming cluster offset
    pub const INCOMING_CLUSTER_OFFSET: usize = 30;

    /// Incoming edge count offset
    pub const INCOMING_COUNT_OFFSET: usize = 38;

    /// External data flag offset (in data_len, high bit)
    pub const EXTERNAL_DATA_FLAG: u16 = 0x8000;

    /// Maximum data length (masking out external flag)
    pub const MAX_DATA_LEN: u16 = 0x7FFF;

    // Field sizes

    /// Node ID size (i64, full - no delta)
    pub const ID_SIZE: usize = 8;

    /// Flags size
    pub const FLAGS_SIZE: usize = 4;

    /// Kind offset size
    pub const KIND_OFFSET_SIZE: usize = 2;

    /// Name offset size
    pub const NAME_OFFSET_SIZE: usize = 2;

    /// Data length size
    pub const DATA_LEN_SIZE: usize = 2;

    /// Outgoing cluster offset size
    pub const OUTGOING_CLUSTER_SIZE: usize = 8;

    /// Outgoing count size
    pub const OUTGOING_COUNT_SIZE: usize = 4;

    /// Incoming cluster offset size
    pub const INCOMING_CLUSTER_SIZE: usize = 8;

    /// Incoming count size
    pub const INCOMING_COUNT_SIZE: usize = 4;
}

/// Fixed metadata size constant
pub const FIXED_METADATA_SIZE: usize = constants::FIXED_METADATA_SIZE;

/// Maximum inline data size constant
pub const MAX_INLINE_DATA: usize = constants::MAX_INLINE_DATA;

/// V3 Node record with simplified full ID encoding
///
/// This structure stores node metadata with full ID encoding (no delta).
/// Delta/varint compression is deferred to Phase 63b.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRecordV3 {
    /// Full node ID (i64, no delta encoding)
    pub id: i64,

    /// Node flags
    pub flags: NodeFlags,

    /// Kind string table offset (u16)
    pub kind_offset: u16,

    /// Name string table offset (u16)
    pub name_offset: u16,

    /// Data length (0-64 inline, >64 external)
    ///
    /// High bit (0x8000) indicates external data storage.
    pub data_len: u16,

    /// Inline data (<= 64 bytes) or None for external
    pub data_inline: Option<Vec<u8>>,

    /// External data offset (if data_len > 64)
    pub data_external_offset: Option<u64>,

    /// Outgoing edge cluster offset
    pub outgoing_cluster_offset: u64,

    /// Outgoing edge count
    pub outgoing_edge_count: u32,

    /// Incoming edge cluster offset
    pub incoming_cluster_offset: u64,

    /// Incoming edge count
    pub incoming_edge_count: u32,
}

impl NodeRecordV3 {
    /// Create a new node record with inline data
    pub fn new_inline(
        id: i64,
        flags: NodeFlags,
        kind_offset: u16,
        name_offset: u16,
        data: Vec<u8>,
        outgoing_cluster_offset: u64,
        outgoing_edge_count: u32,
        incoming_cluster_offset: u64,
        incoming_edge_count: u32,
    ) -> Self {
        let data_len = data.len() as u16;
        assert!(
            data_len <= MAX_INLINE_DATA as u16,
            "Inline data exceeds MAX_INLINE_DATA"
        );

        NodeRecordV3 {
            id,
            flags,
            kind_offset,
            name_offset,
            data_len,
            data_inline: Some(data),
            data_external_offset: None,
            outgoing_cluster_offset,
            outgoing_edge_count,
            incoming_cluster_offset,
            incoming_edge_count,
        }
    }

    /// Create a new node record with external data reference
    pub fn new_external(
        id: i64,
        flags: NodeFlags,
        kind_offset: u16,
        name_offset: u16,
        data_external_offset: u64,
        data_len: u16,
        outgoing_cluster_offset: u64,
        outgoing_edge_count: u32,
        incoming_cluster_offset: u64,
        incoming_edge_count: u32,
    ) -> Self {
        assert!(
            data_len > MAX_INLINE_DATA as u16,
            "External data must exceed MAX_INLINE_DATA"
        );

        NodeRecordV3 {
            id,
            flags,
            kind_offset,
            name_offset,
            data_len,
            data_inline: None,
            data_external_offset: Some(data_external_offset),
            outgoing_cluster_offset,
            outgoing_edge_count,
            incoming_cluster_offset,
            incoming_edge_count,
        }
    }

    /// Get the node ID (full encoding, no delta)
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Check if data is stored inline
    pub fn is_inline(&self) -> bool {
        self.data_inline.is_some()
    }

    /// Check if data is stored externally
    pub fn is_external(&self) -> bool {
        // Check both the external flag in data_len and the optional offset
        self.data_external_offset.is_some() || (self.data_len & constants::EXTERNAL_DATA_FLAG) != 0
    }

    /// Get the data length
    pub fn data_len(&self) -> u16 {
        self.data_len & constants::MAX_DATA_LEN
    }

    /// Calculate the serialized size of this record
    pub fn serialized_size(&self) -> usize {
        let mut size = FIXED_METADATA_SIZE;
        if self.is_inline() {
            size += self.data_inline.as_ref().map(|d| d.len()).unwrap_or(0);
        } else if self.is_external() {
            // External data: store 8-byte offset
            size += 8;
        }
        size
    }

    /// Create an estimate for page capacity planning
    pub fn size_estimate() -> usize {
        FIXED_METADATA_SIZE + MAX_INLINE_DATA / 2
    }
}
