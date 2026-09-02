//! Compact edge record format (migrated from deprecated core::edge_cluster)

use crate::backend::native::{NativeBackendError, NativeResult};

/// Adjacency direction for edge cluster construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// Compact on-disk representation for edges inside a cluster.
///
/// Layout: [neighbor_id: 8 bytes] [type_offset: 2 bytes] [data_len: 2 bytes] [data: variable]
#[derive(Debug, Clone)]
pub struct CompactEdgeRecord {
    /// Neighbor node ID (target for outgoing, source for incoming).
    pub neighbor_id: i64,
    /// Edge type offset inside the shared string table.
    pub edge_type_offset: u16,
    /// Serialized JSON payload for the edge.
    pub edge_data: Vec<u8>,
}

impl CompactEdgeRecord {
    /// Construct a new compact record from primitive fields.
    pub fn new(neighbor_id: i64, edge_type_offset: u16, edge_data: Vec<u8>) -> Self {
        Self {
            neighbor_id,
            edge_type_offset,
            edge_data,
        }
    }

    /// Serialize the record into the binary cluster layout.
    pub fn serialize(&self) -> Vec<u8> {
        let edge_data_len = self.edge_data.len() as u16;
        let mut buffer = Vec::with_capacity(8 + 2 + 2 + self.edge_data.len());
        buffer.extend_from_slice(&self.neighbor_id.to_be_bytes());
        buffer.extend_from_slice(&self.edge_type_offset.to_be_bytes());
        buffer.extend_from_slice(&edge_data_len.to_be_bytes());
        buffer.extend_from_slice(&self.edge_data);
        buffer
    }

    /// Deserialize a compact record from the provided bytes.
    pub fn deserialize(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < 12 {
            return Err(NativeBackendError::BufferTooSmall {
                size: bytes.len(),
                min_size: 12,
            });
        }

        let neighbor_id = i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);

        let edge_type_offset = u16::from_be_bytes([bytes[8], bytes[9]]);
        let edge_data_len = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;

        if bytes.len() < 12 + edge_data_len {
            return Err(NativeBackendError::BufferTooSmall {
                size: bytes.len(),
                min_size: 12 + edge_data_len,
            });
        }

        let edge_data = bytes[12..12 + edge_data_len].to_vec();

        Ok(Self {
            neighbor_id,
            edge_type_offset,
            edge_data,
        })
    }

    /// Total serialized size of this record.
    pub fn size_bytes(&self) -> usize {
        8 + 2 + 2 + self.edge_data.len()
    }
}
