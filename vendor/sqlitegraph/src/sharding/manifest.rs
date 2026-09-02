//! Shard manifest metadata for CSR sharded graphs.
//!
//! Manifest file (`manifest.json`) stores metadata about all shards in the graph.

use serde::{Deserialize, Serialize};

/// Manifest containing metadata for all CSR shards.
///
/// Created by `scripts/shard_csr_edges.py` during graph extraction.
/// Format: JSON with shard metadata and global statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// All shards in the graph
    pub shards: Vec<ShardMetadata>,

    /// Total number of edges across all shards
    pub total_edges: usize,

    /// Number of source nodes covered by shards
    pub total_sources: usize,

    /// Shard file format version
    pub version: String,

    /// Creation timestamp
    pub created_at: i64,
}

/// Metadata for a single CSR shard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardMetadata {
    /// Shard identifier (matches `CsrShard.shard_id`)
    pub shard_id: usize,

    /// Source ID range this shard covers
    pub source_start: u32,
    pub source_end: u32,

    /// Number of edges in this shard
    pub edge_count: usize,

    /// Shard file name: `shard_NNNN.csr`
    pub file_name: String,
}

impl Manifest {
    /// Create a new manifest from shard metadata.
    pub fn new(shards: Vec<ShardMetadata>, version: String) -> Self {
        let total_edges = shards.iter().map(|s| s.edge_count).sum();
        let total_sources = shards
            .iter()
            .map(|s| (s.source_end - s.source_start) as usize)
            .sum();

        Self {
            shards,
            total_edges,
            total_sources,
            version,
            created_at: 0, // Timestamp is loaded from manifest file
        }
    }

    /// Get metadata for a specific shard.
    pub fn get_shard(&self, shard_id: usize) -> Option<&ShardMetadata> {
        self.shards.iter().find(|s| s.shard_id == shard_id)
    }

    /// Get all shard IDs in order.
    pub fn shard_ids(&self) -> Vec<usize> {
        let mut ids: Vec<_> = self.shards.iter().map(|s| s.shard_id).collect();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_creation() {
        let shards = vec![
            ShardMetadata {
                shard_id: 0,
                source_start: 0,
                source_end: 1000,
                edge_count: 100,
                file_name: "shard_0000.csr".to_string(),
            },
            ShardMetadata {
                shard_id: 1,
                source_start: 1000,
                source_end: 2000,
                edge_count: 150,
                file_name: "shard_0001.csr".to_string(),
            },
        ];

        let manifest = Manifest::new(shards, "1.0".to_string());

        assert_eq!(manifest.total_edges, 250);
        assert_eq!(manifest.total_sources, 2000);
        assert_eq!(manifest.version, "1.0");
        assert_eq!(manifest.created_at, 0);
    }

    #[test]
    fn test_get_shard() {
        let shards = vec![
            ShardMetadata {
                shard_id: 0,
                source_start: 0,
                source_end: 1000,
                edge_count: 100,
                file_name: "shard_0000.csr".to_string(),
            },
            ShardMetadata {
                shard_id: 1,
                source_start: 1000,
                source_end: 2000,
                edge_count: 150,
                file_name: "shard_0001.csr".to_string(),
            },
        ];
        let manifest = Manifest::new(shards, "1.0".to_string());

        assert_eq!(manifest.get_shard(0).unwrap().shard_id, 0);
        assert_eq!(manifest.get_shard(1).unwrap().shard_id, 1);
        assert!(manifest.get_shard(2).is_none());
    }

    #[test]
    fn test_shard_ids() {
        let shards = vec![
            ShardMetadata {
                shard_id: 1,
                source_start: 1000,
                source_end: 2000,
                edge_count: 150,
                file_name: "shard_0001.csr".to_string(),
            },
            ShardMetadata {
                shard_id: 0,
                source_start: 0,
                source_end: 1000,
                edge_count: 100,
                file_name: "shard_0000.csr".to_string(),
            },
        ];
        let manifest = Manifest::new(shards, "1.0".to_string());

        let ids = manifest.shard_ids();
        assert_eq!(ids, vec![0, 1]); // Sorted
    }
}
