//! Graph Topology Generators for Benchmarks
//!
//! Provides various graph structures to test algorithm performance
//! under different topological conditions.
//!
//! Each bench binary uses a subset of these helpers; clippy flags the rest
//! as dead per-target. Allow it here so the shared utilities stay green.

#![allow(
    dead_code,
    reason = "shared bench utilities used across multiple bench targets"
)]

use rand::Rng;
use sqlitegraph::backend::{EdgeSpec, NodeSpec};

/// Graph topology types for benchmarking
#[derive(Debug, Clone, Copy)]
pub enum GraphTopology {
    /// Random graph with uniform edge distribution
    Random,
    /// Chain/linked list structure (worst case for SQLite)
    Chain,
    /// Star topology (single hub)
    Star,
    /// Binary tree structure
    BinaryTree,
    /// Power-law (scale-free) distribution
    PowerLaw { alpha: f64 },
    /// Grid/lattice structure
    Grid { width: usize },
    /// Complete graph (every node connects to every other)
    Complete,
}

impl GraphTopology {
    /// Generate graph data for this topology
    pub fn generate(&self, nodes: usize, target_edges: usize) -> GraphData {
        match self {
            GraphTopology::Random => generate_random(nodes, target_edges),
            GraphTopology::Chain => generate_chain(nodes),
            GraphTopology::Star => generate_star(nodes),
            GraphTopology::BinaryTree => generate_binary_tree(nodes),
            GraphTopology::PowerLaw { alpha } => generate_power_law(nodes, target_edges, *alpha),
            GraphTopology::Grid { width } => generate_grid(nodes, *width),
            GraphTopology::Complete => generate_complete(nodes),
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            GraphTopology::Random => "Random uniform graph",
            GraphTopology::Chain => "Linear chain (linked list)",
            GraphTopology::Star => "Star topology (single hub)",
            GraphTopology::BinaryTree => "Binary tree structure",
            GraphTopology::PowerLaw { .. } => "Power-law (scale-free) graph",
            GraphTopology::Grid { .. } => "Grid/lattice structure",
            GraphTopology::Complete => "Complete graph (clique)",
        }
    }
}

/// Graph data container
#[derive(Clone)]
pub struct GraphData {
    pub nodes: Vec<NodeSpec>,
    pub edges: Vec<EdgeSpec>,
    pub topology: GraphTopology,
    pub node_count: usize,
    pub edge_count: usize,
}

impl GraphData {
    /// Calculate average degree
    pub fn avg_degree(&self) -> f64 {
        if self.node_count == 0 {
            return 0.0;
        }
        2.0 * self.edge_count as f64 / self.node_count as f64
    }

    /// Calculate graph density (0.0 to 1.0 for directed)
    pub fn density(&self) -> f64 {
        if self.node_count <= 1 {
            return 0.0;
        }
        let max_edges = self.node_count * (self.node_count - 1);
        self.edge_count as f64 / max_edges as f64
    }
}

// ============================================================================
// Topology Generators
// ============================================================================

/// Generate random uniform graph
fn generate_random(nodes: usize, edge_count: usize) -> GraphData {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "random"}),
        })
        .collect();

    let mut edges = Vec::with_capacity(edge_count);
    for i in 0..edge_count {
        let mut hasher = DefaultHasher::new();
        i.hash(&mut hasher);
        let hash = hasher.finish();

        let from = ((hash % nodes as u64) + 1) as i64;
        let to = (((hash >> 32) % nodes as u64) + 1) as i64;

        if from != to {
            edges.push(EdgeSpec {
                from,
                to,
                edge_type: "Edge".to_string(),
                data: serde_json::json!({"idx": i}),
            });
        }
    }

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::Random,
        node_count: nodes,
        edge_count,
    }
}

/// Generate linear chain (1->2->3->...)
fn generate_chain(nodes: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "chain"}),
        })
        .collect();

    let edges: Vec<_> = (1..nodes)
        .map(|i| EdgeSpec {
            from: i as i64,
            to: (i + 1) as i64,
            edge_type: "Next".to_string(),
            data: serde_json::json!({}),
        })
        .collect();

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::Chain,
        node_count: nodes,
        edge_count,
    }
}

/// Generate star topology (node 1 is hub)
fn generate_star(nodes: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "star"}),
        })
        .collect();

    let edges: Vec<_> = (2..=nodes)
        .map(|i| EdgeSpec {
            from: 1,
            to: i as i64,
            edge_type: "Spoke".to_string(),
            data: serde_json::json!({}),
        })
        .collect();

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::Star,
        node_count: nodes,
        edge_count,
    }
}

/// Generate binary tree
fn generate_binary_tree(nodes: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "binary_tree"}),
        })
        .collect();

    let mut edges = Vec::new();
    for i in 1..=nodes {
        let left = i * 2;
        let right = i * 2 + 1;

        if left <= nodes {
            edges.push(EdgeSpec {
                from: i as i64,
                to: left as i64,
                edge_type: "Left".to_string(),
                data: serde_json::json!({}),
            });
        }
        if right <= nodes {
            edges.push(EdgeSpec {
                from: i as i64,
                to: right as i64,
                edge_type: "Right".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::BinaryTree,
        node_count: nodes,
        edge_count,
    }
}

/// Generate power-law (scale-free) graph using preferential attachment
fn generate_power_law(nodes: usize, target_edges: usize, alpha: f64) -> GraphData {
    use rand::SeedableRng;
    use rand::distributions::{Distribution, WeightedIndex};
    use rand::rngs::StdRng;

    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "power_law"}),
        })
        .collect();

    let mut edges = Vec::with_capacity(target_edges);
    let mut degrees = vec![0usize; nodes];

    // Seed with initial edges
    let mut rng = StdRng::seed_from_u64(42);

    for i in 1..nodes.min(10) {
        edges.push(EdgeSpec {
            from: i as i64,
            to: (i + 1) as i64,
            edge_type: "Edge".to_string(),
            data: serde_json::json!({}),
        });
        degrees[i] += 1;
        degrees[i - 1] += 1;
    }

    // Preferential attachment
    while edges.len() < target_edges {
        // Pick source uniformly
        let source = rng.gen_range(0..nodes);

        // Pick target with probability proportional to degree^alpha
        let weights: Vec<_> = degrees
            .iter()
            .map(|&d| ((d + 1) as f64).powf(alpha) as u64)
            .collect();

        if let Ok(dist) = WeightedIndex::new(&weights) {
            let target = dist.sample(&mut rng);

            if source != target {
                edges.push(EdgeSpec {
                    from: (source + 1) as i64,
                    to: (target + 1) as i64,
                    edge_type: "Edge".to_string(),
                    data: serde_json::json!({}),
                });
                degrees[source] += 1;
                degrees[target] += 1;
            }
        }
    }

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::PowerLaw { alpha },
        node_count: nodes,
        edge_count,
    }
}

/// Generate grid/lattice structure
fn generate_grid(nodes: usize, width: usize) -> GraphData {
    let height = nodes.div_ceil(width);

    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "grid"}),
        })
        .collect();

    let mut edges = Vec::new();

    for i in 0..nodes {
        let x = i % width;
        let y = i / width;

        // Right neighbor
        if x + 1 < width && i + 1 < nodes {
            edges.push(EdgeSpec {
                from: (i + 1) as i64,
                to: (i + 2) as i64,
                edge_type: "Right".to_string(),
                data: serde_json::json!({}),
            });
        }

        // Bottom neighbor
        if y + 1 < height && i + width < nodes {
            edges.push(EdgeSpec {
                from: (i + 1) as i64,
                to: (i + width + 1) as i64,
                edge_type: "Down".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::Grid { width },
        node_count: nodes,
        edge_count,
    }
}

/// Generate complete graph (clique)
fn generate_complete(nodes: usize) -> GraphData {
    let node_specs = (0..nodes)
        .map(|i| NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i, "topology": "complete"}),
        })
        .collect();

    let mut edges = Vec::new();
    for i in 1..=nodes {
        for j in (i + 1)..=nodes {
            edges.push(EdgeSpec {
                from: i as i64,
                to: j as i64,
                edge_type: "Edge".to_string(),
                data: serde_json::json!({}),
            });
        }
    }

    let edge_count = edges.len();

    GraphData {
        nodes: node_specs,
        edges,
        topology: GraphTopology::Complete,
        node_count: nodes,
        edge_count,
    }
}

#[cfg(test)]
mod tests {
    #[allow(
        unused_imports,
        reason = "items resolved through allow(dead_code) attribute"
    )]
    use super::*;

    #[test]
    fn test_chain_topology() {
        let data = GraphTopology::Chain.generate(10, 0);
        assert_eq!(data.node_count, 10);
        assert_eq!(data.edge_count, 9); // 9 edges in chain of 10
        assert_eq!(data.avg_degree(), 1.8); // Each internal node has degree 2
    }

    #[test]
    fn test_star_topology() {
        let data = GraphTopology::Star.generate(10, 0);
        assert_eq!(data.node_count, 10);
        assert_eq!(data.edge_count, 9); // 9 spokes
    }

    #[test]
    fn test_binary_tree() {
        let data = GraphTopology::BinaryTree.generate(7, 0); // Perfect tree with 7 nodes
        assert_eq!(data.node_count, 7);
        assert_eq!(data.edge_count, 6); // 7 nodes in binary tree = 6 edges
    }
}
