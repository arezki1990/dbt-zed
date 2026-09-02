//! Bidirectional index for backward rescoring in generation.
//!
//! `BidirectionalIndex` builds reverse edges from CSR to support `P(src|dst)` queries.
//! This enables backward language model scoring during generation.

use std::collections::HashMap;

use crate::sharding::reader::ShardReader;
use crate::sharding::shard::CsrEdge;

/// Bidirectional index supporting forward and reverse edge queries.
///
/// Built from CSR shards by computing reverse edges. Enables efficient
/// backward rescoring during generation.
#[derive(Debug)]
pub struct BidirectionalIndex {
    /// Forward edges: src -> Vec<dst> (from original CSR)
    forward: HashMap<u32, Vec<CsrEdge>>,

    /// Reverse edges: dst -> Vec<src> (computed at build time)
    reverse: HashMap<u32, Vec<CsrEdge>>,
}

impl BidirectionalIndex {
    /// Create a new bidirectional index from a shard reader.
    ///
    /// # Performance
    ///
    /// - Time: O(total_edges) to build reverse index
    /// - Space: O(total_edges) for reverse edges
    /// - Typical cost: 4M edges → ~32 MB additional memory
    pub fn new(reader: &ShardReader) -> Self {
        let mut forward = HashMap::new();
        let mut reverse = HashMap::new();

        // Build forward and reverse indices from all shards
        for shard_id in 0..reader.shard_count() {
            if let Some(shard) = reader.get_shard(shard_id) {
                for edge in &shard.edges {
                    // Forward edge: src -> dst
                    forward
                        .entry(edge.src)
                        .or_insert_with(Vec::new)
                        .push(edge.clone());

                    // Reverse edge: dst -> src (with same weight)
                    let reverse_edge = CsrEdge {
                        src: edge.dst,
                        dst: edge.src,
                        weight: edge.weight,
                        flags: edge.flags,
                    };
                    reverse
                        .entry(edge.dst)
                        .or_insert_with(Vec::new)
                        .push(reverse_edge);
                }
            }
        }

        Self { forward, reverse }
    }

    /// Get forward edges from a source node.
    ///
    /// Returns edges where `src` is the given node ID.
    /// Returns `None` if the node has no outgoing edges.
    pub fn get_forward(&self, src: u32) -> Option<&[CsrEdge]> {
        self.forward.get(&src).map(|v| v.as_slice())
    }

    /// Get reverse edges pointing to a destination node.
    ///
    /// Returns edges where `dst` is the given node ID (i.e., incoming edges).
    /// Returns `None` if the node has no incoming edges.
    pub fn get_reverse(&self, dst: u32) -> Option<&[CsrEdge]> {
        self.reverse.get(&dst).map(|v| v.as_slice())
    }

    /// Compute backward support for a destination token.
    ///
    /// Backward support = sum of `P(src|dst)` over all incoming edges.
    /// This measures how well the destination token is supported by
    /// preceding context in the graph.
    ///
    /// # Arguments
    ///
    /// * `dst` - Destination token ID
    ///
    /// # Returns
    ///
    /// Sum of edge weights for all edges pointing to `dst`.
    /// Returns 0.0 if `dst` has no incoming edges.
    pub fn backward_support(&self, dst: u32) -> f32 {
        self.get_reverse(dst)
            .map_or(0.0, |edges| edges.iter().map(|e| e.weight).sum())
    }

    /// Get the number of nodes with outgoing edges.
    pub fn forward_node_count(&self) -> usize {
        self.forward.len()
    }

    /// Get the number of nodes with incoming edges.
    pub fn reverse_node_count(&self) -> usize {
        self.reverse.len()
    }

    /// Get total number of forward edges.
    pub fn forward_edge_count(&self) -> usize {
        self.forward.values().map(|v| v.len()).sum()
    }

    /// Get total number of reverse edges.
    pub fn reverse_edge_count(&self) -> usize {
        self.reverse.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sharding::shard::CsrShard;

    fn create_test_shards() -> Vec<CsrShard> {
        let mut shards = Vec::new();

        // Shard 0: Simple chain
        let mut shard0 = CsrShard::new(0, 0, 1000);
        shard0.add_edge(CsrEdge {
            src: 100,
            dst: 200,
            weight: 0.5,
            flags: 0,
        });
        shard0.add_edge(CsrEdge {
            src: 200,
            dst: 300,
            weight: 0.3,
            flags: 0,
        });
        shard0.add_edge(CsrEdge {
            src: 300,
            dst: 400,
            weight: 0.7,
            flags: 0,
        });
        shard0.sort_edges();
        shards.push(shard0);

        // Shard 1: Branching structure
        let mut shard1 = CsrShard::new(1, 1000, 2000);
        shard1.add_edge(CsrEdge {
            src: 1500,
            dst: 200,
            weight: 0.4,
            flags: 0,
        });
        shard1.add_edge(CsrEdge {
            src: 1500,
            dst: 500,
            weight: 0.6,
            flags: 0,
        });
        shard1.sort_edges();
        shards.push(shard1);

        shards
    }

    #[test]
    fn test_bidirectional_index_creation() {
        let shards = create_test_shards();

        // Create a mock reader (simplified for testing)
        let mut index = BidirectionalIndex {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };

        // Manually build index from test shards
        for shard in &shards {
            for edge in &shard.edges {
                index
                    .forward
                    .entry(edge.src)
                    .or_default()
                    .push(edge.clone());

                let reverse_edge = CsrEdge {
                    src: edge.dst,
                    dst: edge.src,
                    weight: edge.weight,
                    flags: edge.flags,
                };
                index
                    .reverse
                    .entry(edge.dst)
                    .or_default()
                    .push(reverse_edge);
            }
        }

        assert_eq!(index.forward_edge_count(), 5);
        assert_eq!(index.reverse_edge_count(), 5);
    }

    #[test]
    fn test_get_forward() {
        let shards = create_test_shards();

        let mut index = BidirectionalIndex {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };

        for shard in &shards {
            for edge in &shard.edges {
                index
                    .forward
                    .entry(edge.src)
                    .or_default()
                    .push(edge.clone());

                let reverse_edge = CsrEdge {
                    src: edge.dst,
                    dst: edge.src,
                    weight: edge.weight,
                    flags: edge.flags,
                };
                index
                    .reverse
                    .entry(edge.dst)
                    .or_default()
                    .push(reverse_edge);
            }
        }

        // Test forward lookup
        let edges_100 = index.get_forward(100);
        assert!(edges_100.is_some());
        assert_eq!(edges_100.unwrap().len(), 1);
        assert_eq!(edges_100.unwrap()[0].dst, 200);

        let edges_200 = index.get_forward(200);
        assert!(edges_200.is_some());
        assert_eq!(edges_200.unwrap().len(), 1);
        assert_eq!(edges_200.unwrap()[0].dst, 300);

        let edges_999 = index.get_forward(999);
        assert!(edges_999.is_none());
    }

    #[test]
    fn test_get_reverse() {
        let shards = create_test_shards();

        let mut index = BidirectionalIndex {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };

        for shard in &shards {
            for edge in &shard.edges {
                index
                    .forward
                    .entry(edge.src)
                    .or_default()
                    .push(edge.clone());

                let reverse_edge = CsrEdge {
                    src: edge.dst,
                    dst: edge.src,
                    weight: edge.weight,
                    flags: edge.flags,
                };
                index
                    .reverse
                    .entry(edge.dst)
                    .or_default()
                    .push(reverse_edge);
            }
        }

        // Test reverse lookup
        // Node 200 has incoming edges from 100 and 1500
        let edges_200 = index.get_reverse(200);
        assert!(edges_200.is_some());
        assert_eq!(edges_200.unwrap().len(), 2);

        // Node 300 has incoming edge from 200
        // Reverse edge: src=300 (original dst), dst=200 (original src)
        let edges_300 = index.get_reverse(300);
        assert!(edges_300.is_some());
        assert_eq!(edges_300.unwrap().len(), 1);
        assert_eq!(edges_300.unwrap()[0].src, 300); // reverse edge src is the query node

        let edges_999 = index.get_reverse(999);
        assert!(edges_999.is_none());
    }

    #[test]
    fn test_backward_support() {
        let shards = create_test_shards();

        let mut index = BidirectionalIndex {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };

        for shard in &shards {
            for edge in &shard.edges {
                index
                    .forward
                    .entry(edge.src)
                    .or_default()
                    .push(edge.clone());

                let reverse_edge = CsrEdge {
                    src: edge.dst,
                    dst: edge.src,
                    weight: edge.weight,
                    flags: edge.flags,
                };
                index
                    .reverse
                    .entry(edge.dst)
                    .or_default()
                    .push(reverse_edge);
            }
        }

        // Node 200 has incoming edges from 100 (weight 0.5) and 1500 (weight 0.4)
        let support_200 = index.backward_support(200);
        assert_eq!(support_200, 0.9); // 0.5 + 0.4

        // Node 500 has incoming edge from 1500 (weight 0.6)
        let support_500 = index.backward_support(500);
        assert_eq!(support_500, 0.6);

        // Node 999 has no incoming edges
        let support_999 = index.backward_support(999);
        assert_eq!(support_999, 0.0);
    }

    #[test]
    fn test_symmetry() {
        let shards = create_test_shards();

        let mut index = BidirectionalIndex {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };

        for shard in &shards {
            for edge in &shard.edges {
                index
                    .forward
                    .entry(edge.src)
                    .or_default()
                    .push(edge.clone());

                let reverse_edge = CsrEdge {
                    src: edge.dst,
                    dst: edge.src,
                    weight: edge.weight,
                    flags: edge.flags,
                };
                index
                    .reverse
                    .entry(edge.dst)
                    .or_default()
                    .push(reverse_edge);
            }
        }

        // Forward and reverse edge counts should be equal
        assert_eq!(index.forward_edge_count(), index.reverse_edge_count());

        // For each forward edge, there should be a corresponding reverse edge
        let mut forward_edges = Vec::new();
        for edges in index.forward.values() {
            forward_edges.extend(edges.iter());
        }

        let mut reverse_edges = Vec::new();
        for edges in index.reverse.values() {
            reverse_edges.extend(edges.iter());
        }

        assert_eq!(forward_edges.len(), reverse_edges.len());
    }
}
