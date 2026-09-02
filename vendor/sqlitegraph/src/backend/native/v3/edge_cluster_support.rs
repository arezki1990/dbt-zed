use super::{
    Direction, EDGE_CLUSTER_PAGE_HEADER_SIZE, EDGE_CLUSTER_PAGE_MAGIC,
    PACKED_EDGE_PAGE_HEADER_SIZE, PACKED_EDGE_PAGE_MAGIC, PACKED_EDGE_PAGE_SLOT_SIZE,
};
use crate::backend::native::types::{NativeBackendError, NativeResult};
use crate::backend::native::v3::compact_edge_record::CompactEdgeRecord;
use std::vec::Vec;

#[path = "edge_cluster_support/page_codec_support.rs"]
mod page_codec_support;
#[path = "edge_cluster_support/record_codec_support.rs"]
mod record_codec_support;
#[cfg(test)]
#[path = "edge_cluster_support/tests.rs"]
mod tests;

/// Edge cluster entry for V3 storage.
/// Uses V2 `CompactEdgeRecord` format for compatibility.
#[derive(Debug, Clone)]
pub struct V3EdgeCluster {
    /// Source node ID (logical, not slot)
    pub src: i64,
    /// Destination node IDs with edge data
    pub edges: Vec<CompactEdgeRecord>,
    /// Edge direction
    pub direction: Direction,
    /// Format version for future migration
    pub format_version: u8,
    /// Page ID where this cluster is stored
    pub page_id: u64,
}

impl V3EdgeCluster {
    pub(crate) fn sort_for_weighted_queries(&mut self) {
        self.edges.sort_by(|a, b| {
            let weight_cmp = Self::extract_edge_weight(&b.edge_data)
                .partial_cmp(&Self::extract_edge_weight(&a.edge_data))
                .unwrap_or(std::cmp::Ordering::Equal);
            weight_cmp.then_with(|| a.neighbor_id.cmp(&b.neighbor_id))
        });
    }

    /// Create new empty edge cluster
    pub fn new(src: i64, direction: Direction, page_id: u64) -> Self {
        Self {
            src,
            edges: Vec::new(),
            direction,
            format_version: 3, // v3 includes delta compression for edge IDs
            page_id,
        }
    }

    /// Add edge to cluster
    /// Edge type is encoded in edge_data using inline format: [type_len: u8][type_bytes]
    pub fn add_edge(&mut self, dst: i64, edge_type: Option<String>) {
        let edge_data = record_codec_support::encode_edge_type_data(edge_type.as_deref());
        let edge = CompactEdgeRecord::new(dst, 0, edge_data);
        self.edges.push(edge);
    }

    /// Add a weighted edge to the cluster
    pub fn add_edge_weighted(&mut self, dst: i64, edge_type: Option<String>, weight: f32) {
        let edge_data =
            record_codec_support::encode_weighted_edge_data(edge_type.as_deref(), weight);
        let edge = CompactEdgeRecord::new(dst, 0, edge_data);
        self.edges.push(edge);
    }

    /// Extract edge type from edge data
    /// Returns None if edge_data is empty (no edge_type stored)
    pub(crate) fn extract_edge_type(edge_data: &[u8]) -> Option<String> {
        record_codec_support::extract_edge_type(edge_data)
    }

    /// Extract edge weight from edge data
    /// Returns 1.0 if not specified (legacy or not provided)
    pub(crate) fn extract_edge_weight(edge_data: &[u8]) -> f32 {
        record_codec_support::extract_edge_weight(edge_data)
    }

    /// Get destination node IDs
    pub fn dsts(&self) -> Vec<i64> {
        self.edges.iter().map(|e| e.neighbor_id).collect()
    }

    /// Get edges with their types (for recovery/rebuilding HashMap)
    pub fn edges_with_types(&self) -> Vec<(i64, Option<String>)> {
        self.edges
            .iter()
            .map(|e| {
                let edge_type = Self::extract_edge_type(&e.edge_data);
                (e.neighbor_id, edge_type)
            })
            .collect()
    }

    /// Serialize to bytes for page storage
    /// Format v3: [version: 1 byte] [src: 8 bytes] [dir: 1 byte] [compressed: 1 byte] [edge_count: 4 bytes] [compressed_ids...][edge_metadata...]
    /// Format v2: [version: 1 byte] [src: 8 bytes] [dir: 1 byte] [edge_count: 4 bytes] [edges...]
    /// Format v1: [version: 1 byte] [edge_count: 4 bytes] [edges...]  (legacy, no src/dir)
    pub fn serialize(&self) -> NativeResult<Vec<u8>> {
        record_codec_support::serialize_cluster(self)
    }

    /// Deserialize from bytes
    /// Format v2: [version: 1 byte] [src: 8 bytes] [dir: 1 byte] [edge_count: 4 bytes] [edges...]
    /// Format v1: [version: 1 byte] [edge_count: 4 bytes] [edges...]  (src=0, dir=Outgoing)
    pub fn deserialize(bytes: &[u8], page_id: u64) -> NativeResult<Self> {
        record_codec_support::deserialize_cluster(bytes, page_id)
    }
}

pub(crate) fn encode_edge_cluster_pages(
    cluster_bytes: &[u8],
    page_size: usize,
    page_ids: &[u64],
) -> NativeResult<Vec<Vec<u8>>> {
    page_codec_support::encode_edge_cluster_pages(cluster_bytes, page_size, page_ids)
}

pub(crate) fn decode_edge_cluster_page_header(page: &[u8]) -> Option<(usize, u64)> {
    page_codec_support::decode_edge_cluster_page_header(page)
}

pub(crate) fn encode_packed_edge_page(
    page_size: usize,
    entries: &[((i64, Direction), Vec<u8>)],
) -> NativeResult<Vec<u8>> {
    page_codec_support::encode_packed_edge_page(page_size, entries)
}

pub(crate) fn decode_packed_edge_page(
    page: &[u8],
    src: i64,
    dir: Direction,
) -> NativeResult<Option<Vec<u8>>> {
    page_codec_support::decode_packed_edge_page(page, src, dir)
}
