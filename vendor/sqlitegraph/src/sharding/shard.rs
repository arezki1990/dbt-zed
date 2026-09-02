//! CSR shard types for subgraph-based graph traversal.
//!
//! Shards break large CSR graphs into manageable pieces for prompt-local traversal.
//! Each shard contains edges for a range of source token IDs.

use serde::{Deserialize, Serialize};

/// A single CSR shard containing edges for a range of source IDs.
///
/// # Format
///
/// Shards are created by `scripts/shard_csr_edges.py` (graphtransformer F21):
/// - Shard size: 1000 source IDs per shard
/// - Format: CSR edges with src/dst/weight/flags
/// - Stored as separate files: `shard_NNNN.csr`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrShard {
    /// Unique shard identifier (0-indexed)
    pub shard_id: usize,

    /// Inclusive start of source ID range for this shard
    pub source_start: u32,

    /// Exclusive end of source ID range for this shard
    pub source_end: u32,

    /// Edges in this shard (sorted by source, then destination)
    pub edges: Vec<CsrEdge>,
}

/// A single directed edge in the CSR graph.
///
/// Represents `P(dst|src)` — conditional probability of destination given source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CsrEdge {
    /// Source token/node ID
    pub src: u32,

    /// Destination token/node ID
    pub dst: u32,

    /// Edge weight (softmax-normalized probability)
    pub weight: f32,

    /// Context metadata (context bucket, edge type, etc.)
    pub flags: u32,
}

impl CsrShard {
    /// Create a new empty shard.
    pub fn new(shard_id: usize, source_start: u32, source_end: u32) -> Self {
        Self {
            shard_id,
            source_start,
            source_end,
            edges: Vec::new(),
        }
    }

    /// Add an edge to this shard.
    ///
    /// # Panics
    ///
    /// Panics if edge `src` is outside this shard's range `[source_start, source_end)`.
    pub fn add_edge(&mut self, edge: CsrEdge) {
        assert!(
            edge.src >= self.source_start && edge.src < self.source_end,
            "Edge src {} outside shard range [{}, {})",
            edge.src,
            self.source_start,
            self.source_end
        );
        self.edges.push(edge);
    }

    /// Sort edges by source, then destination for fast lookup.
    pub fn sort_edges(&mut self) {
        self.edges
            .sort_by(|a, b| a.src.cmp(&b.src).then_with(|| a.dst.cmp(&b.dst)));
    }

    /// Return the number of edges in this shard.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_creation() {
        let shard = CsrShard::new(0, 1000, 2000);
        assert_eq!(shard.shard_id, 0);
        assert_eq!(shard.source_start, 1000);
        assert_eq!(shard.source_end, 2000);
        assert_eq!(shard.edge_count(), 0);
    }

    #[test]
    fn test_add_edge() {
        let mut shard = CsrShard::new(0, 1000, 2000);
        let edge = CsrEdge {
            src: 1500,
            dst: 2500,
            weight: 0.5,
            flags: 0,
        };
        shard.add_edge(edge.clone());
        assert_eq!(shard.edge_count(), 1);
        assert_eq!(shard.edges[0], edge);
    }

    #[test]
    #[should_panic]
    fn test_add_edge_outside_range_panics() {
        let mut shard = CsrShard::new(0, 1000, 2000);
        let edge = CsrEdge {
            src: 500, // Outside range
            dst: 2500,
            weight: 0.5,
            flags: 0,
        };
        shard.add_edge(edge); // Should panic
    }

    #[test]
    fn test_sort_edges() {
        let mut shard = CsrShard::new(0, 1000, 2000);
        shard.add_edge(CsrEdge {
            src: 1500,
            dst: 2500,
            weight: 0.3,
            flags: 0,
        });
        shard.add_edge(CsrEdge {
            src: 1200,
            dst: 2300,
            weight: 0.5,
            flags: 0,
        });
        shard.add_edge(CsrEdge {
            src: 1500,
            dst: 2400,
            weight: 0.2,
            flags: 0,
        });

        shard.sort_edges();

        // Should be sorted by src, then dst
        assert_eq!(shard.edges[0].src, 1200);
        assert_eq!(shard.edges[1].src, 1500);
        assert_eq!(shard.edges[1].dst, 2400); // Same src, lower dst first
        assert_eq!(shard.edges[2].dst, 2500);
    }
}
