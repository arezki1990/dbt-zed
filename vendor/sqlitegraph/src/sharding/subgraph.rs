//! BFS subgraph builder for prompt-local traversal.
//!
//! `SubgraphBuilder` constructs a local subgraph from prompt tokens using sharded CSR.
//! This enables efficient generation without loading the full graph into memory.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::sharding::reader::ShardReader;
use crate::sharding::shard::{CsrEdge, CsrShard};

/// A local subgraph extracted from the full CSR graph.
///
/// Contains nodes and edges reachable from prompt tokens within a given depth.
#[derive(Debug, Clone)]
pub struct Subgraph {
    /// All node IDs in this subgraph
    pub nodes: HashSet<u32>,

    /// All edges in this subgraph
    pub edges: Vec<CsrEdge>,

    /// Maximum depth from source nodes
    pub max_depth: usize,
}

impl Subgraph {
    /// Create a new empty subgraph.
    pub fn new() -> Self {
        Self {
            nodes: HashSet::new(),
            edges: Vec::new(),
            max_depth: 0,
        }
    }

    /// Add a node to the subgraph.
    pub fn add_node(&mut self, node_id: u32) {
        self.nodes.insert(node_id);
    }

    /// Add an edge to the subgraph.
    pub fn add_edge(&mut self, edge: CsrEdge) {
        let src = edge.src;
        let dst = edge.dst;
        self.edges.push(edge);
        self.nodes.insert(src);
        self.nodes.insert(dst);
    }

    /// Return the number of nodes in this subgraph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Return the number of edges in this subgraph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl Default for Subgraph {
    fn default() -> Self {
        Self::new()
    }
}

/// BFS subgraph builder using sharded CSR graphs.
///
/// Given a set of prompt tokens, constructs a subgraph by exploring outgoing
/// edges to a specified depth. Uses shard-level filtering to avoid loading
/// irrelevant graph regions.
pub struct SubgraphBuilder {
    /// Loaded CSR shards
    shards: Vec<CsrShard>,

    /// Map from source ID to shard index for fast lookup
    shard_index: HashMap<u32, usize>,
}

impl SubgraphBuilder {
    /// Create a new subgraph builder from a shard reader.
    pub fn new(reader: &ShardReader) -> Self {
        let mut shard_index = HashMap::new();

        // Build index mapping source IDs to shard IDs
        for shard_id in 0..reader.shard_count() {
            if let Some(shard) = reader.get_shard(shard_id) {
                for source_id in shard.source_start..shard.source_end {
                    shard_index.insert(source_id, shard_id);
                }
            }
        }

        // Collect all shards into a Vec for indexed access
        let shards: Vec<CsrShard> = (0..reader.shard_count())
            .filter_map(|id| reader.get_shard(id).cloned())
            .collect();

        Self {
            shards,
            shard_index,
        }
    }

    /// Build a subgraph from prompt tokens to a given depth.
    ///
    /// # Arguments
    ///
    /// * `prompt_tokens` - Starting node IDs for BFS expansion
    /// * `max_depth` - Maximum BFS depth (default 2 for generation)
    ///
    /// # Returns
    ///
    /// A `Subgraph` containing all nodes and edges reachable from prompt tokens
    /// within `max_depth` hops.
    ///
    /// # Performance
    ///
    /// - Time: O(edges_visited) where edges_visited depends on branching factor
    /// - Space: O(nodes_visited + edges_visited)
    /// - For depth=2 on typical graphs: ~3000 nodes, ~4000 edges, <1s build time
    pub fn build_subgraph(&self, prompt_tokens: &[u32], max_depth: usize) -> Subgraph {
        let mut subgraph = Subgraph::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        // Initialize BFS with prompt tokens at depth 0
        for &token_id in prompt_tokens {
            if !visited.contains(&token_id) {
                visited.insert(token_id);
                queue.push_back((token_id, 0));
                subgraph.add_node(token_id);
            }
        }

        // BFS expansion
        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            // Find outgoing edges from this node
            if let Some(edges) = self.get_outgoing_edges(node_id) {
                for edge in edges {
                    let dst = edge.dst;

                    // Add edge and destination node to subgraph
                    subgraph.add_edge(edge.clone());

                    // Enqueue destination if not visited
                    if !visited.contains(&dst) {
                        visited.insert(dst);
                        queue.push_back((dst, depth + 1));
                    }
                }
            }
        }

        subgraph.max_depth = max_depth;
        subgraph
    }

    /// Get outgoing edges from a node using shard filtering.
    ///
    /// Returns edges only from the shard that contains this source node.
    fn get_outgoing_edges(&self, source_id: u32) -> Option<Vec<CsrEdge>> {
        // Find which shard contains this source ID
        let shard_id = self.shard_index.get(&source_id)?;

        // Get the shard
        let shard = &self.shards.get(*shard_id)?;

        // Binary search the contiguous range of edges for source_id
        let start_idx = shard.edges.partition_point(|edge| edge.src < source_id);
        let end_idx =
            shard.edges[start_idx..].partition_point(|edge| edge.src == source_id) + start_idx;

        Some(shard.edges[start_idx..end_idx].to_vec())
    }

    /// Get shard statistics for debugging.
    pub fn shard_stats(&self) -> Vec<(usize, usize, usize)> {
        self.shards
            .iter()
            .map(|shard| {
                (
                    shard.shard_id,
                    shard.edge_count(),
                    (shard.source_end - shard.source_start) as usize,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sharding::shard::CsrShard;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_shards(_dir: &PathBuf) -> Vec<CsrShard> {
        let mut shards = Vec::new();

        // Shard 0: sources [0, 1000)
        let mut shard0 = CsrShard::new(0, 0, 1000);
        shard0.add_edge(CsrEdge {
            src: 100,
            dst: 2000,
            weight: 0.5,
            flags: 0,
        });
        shard0.add_edge(CsrEdge {
            src: 100,
            dst: 3000,
            weight: 0.3,
            flags: 0,
        });
        shard0.add_edge(CsrEdge {
            src: 200,
            dst: 4000,
            weight: 0.7,
            flags: 0,
        });
        shard0.sort_edges();
        shards.push(shard0);

        // Shard 1: sources [1000, 2000)
        let mut shard1 = CsrShard::new(1, 1000, 2000);
        shard1.add_edge(CsrEdge {
            src: 1500,
            dst: 5000,
            weight: 0.4,
            flags: 0,
        });
        shard1.add_edge(CsrEdge {
            src: 1500,
            dst: 6000,
            weight: 0.6,
            flags: 0,
        });
        shard1.sort_edges();
        shards.push(shard1);

        // Shard 2: sources [2000, 3000)
        let mut shard2 = CsrShard::new(2, 2000, 3000);
        shard2.add_edge(CsrEdge {
            src: 2500,
            dst: 7000,
            weight: 0.8,
            flags: 0,
        });
        shard2.add_edge(CsrEdge {
            src: 2500,
            dst: 8000,
            weight: 0.2,
            flags: 0,
        });
        shard2.sort_edges();
        shards.push(shard2);

        shards
    }

    #[test]
    fn test_subgraph_builder_creation() {
        let temp = TempDir::new().unwrap();
        let shards = create_test_shards(&temp.path().to_path_buf());

        // Create a simple reader with our test shards
        let mut reader_data = vec![];
        for shard in &shards {
            reader_data.push((
                shard.shard_id,
                shard.source_start,
                shard.source_end,
                shard.edge_count(),
                "test.csr",
            ));
        }

        // Verify we can create shards
        assert_eq!(shards.len(), 3);
        assert_eq!(shards[0].source_start, 0);
        assert_eq!(shards[0].source_end, 1000);
    }

    #[test]
    fn test_subgraph_empty_tokens() {
        let temp = TempDir::new().unwrap();
        let shards = create_test_shards(&temp.path().to_path_buf());

        // Create builder from shards
        let builder = SubgraphBuilder {
            shards: shards.clone(),
            shard_index: {
                let mut index = HashMap::new();
                for shard in &shards {
                    for source_id in shard.source_start..shard.source_end {
                        index.insert(source_id, shard.shard_id);
                    }
                }
                index
            },
        };

        let subgraph = builder.build_subgraph(&[], 2);

        assert_eq!(subgraph.node_count(), 0);
        assert_eq!(subgraph.edge_count(), 0);
    }

    #[test]
    fn test_subgraph_single_token_depth_1() {
        let temp = TempDir::new().unwrap();
        let shards = create_test_shards(&temp.path().to_path_buf());

        let builder = SubgraphBuilder {
            shards: shards.clone(),
            shard_index: {
                let mut index = HashMap::new();
                for shard in &shards {
                    for source_id in shard.source_start..shard.source_end {
                        index.insert(source_id, shard.shard_id);
                    }
                }
                index
            },
        };

        // Start from token 100, expand to depth 1
        let subgraph = builder.build_subgraph(&[100], 1);

        // Should include: 100, 2000, 3000
        assert_eq!(subgraph.node_count(), 3);
        assert_eq!(subgraph.edge_count(), 2); // 100->2000, 100->3000
    }

    #[test]
    fn test_subgraph_single_token_depth_2() {
        let temp = TempDir::new().unwrap();
        let shards = create_test_shards(&temp.path().to_path_buf());

        let builder = SubgraphBuilder {
            shards: shards.clone(),
            shard_index: {
                let mut index = HashMap::new();
                for shard in &shards {
                    for source_id in shard.source_start..shard.source_end {
                        index.insert(source_id, shard.shard_id);
                    }
                }
                index
            },
        };

        // Start from token 100, expand to depth 2
        let subgraph = builder.build_subgraph(&[100], 2);

        // Depth 1: 100->2000, 100->3000
        // Depth 2: 2000 and 3000 have no outgoing edges (no shards cover them)
        // Total: 100, 2000, 3000 (3 nodes), 2 edges
        assert_eq!(subgraph.node_count(), 3);
        assert_eq!(subgraph.edge_count(), 2);
    }

    #[test]
    fn test_subgraph_multi_token() {
        let temp = TempDir::new().unwrap();
        let shards = create_test_shards(&temp.path().to_path_buf());

        let builder = SubgraphBuilder {
            shards: shards.clone(),
            shard_index: {
                let mut index = HashMap::new();
                for shard in &shards {
                    for source_id in shard.source_start..shard.source_end {
                        index.insert(source_id, shard.shard_id);
                    }
                }
                index
            },
        };

        // Start from tokens 100 and 1500
        let subgraph = builder.build_subgraph(&[100, 1500], 1);

        // From 100: 2000, 3000
        // From 1500: 5000, 6000
        // Total: 100, 1500, 2000, 3000, 5000, 6000 (6 nodes), 4 edges
        assert_eq!(subgraph.node_count(), 6);
        assert_eq!(subgraph.edge_count(), 4);
    }
}
