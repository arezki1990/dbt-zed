//! HNSW Layer Management
//!
//! This module implements the layered graph structure of HNSW, managing
//! multiple layers with different connectivity levels. Each layer represents
//! a proximity graph at different scales of the data distribution.
//!
//! # Architecture
//!
//! - **Layer 0**: Base layer with maximum connectivity (M connections per node)
//! - **Higher Layers**: Progressively sparser layers with reduced connectivity
//! - **Entry Points**: Optimal entry nodes for navigation between layers
//! - **Navigation**: Layer-by-layer traversal for efficient search
//!
//! # Layer Properties
//!
//! ```text
//! Layer 0 (Base):    M connections per node, all nodes
//! Layer 1:           M/2 connections, subset of nodes
//! Layer 2:           M/4 connections, smaller subset
//! Layer N:           1 connection, top-level navigation
//! ```

use crate::hnsw::errors::{HnswError, HnswIndexError};
use std::collections::{HashMap, HashSet};

/// HNSW layer containing nodes and their connections
///
/// Each layer represents a proximity graph with a specific maximum
/// number of connections per node. Lower layers are denser with more
/// nodes and connections, while higher layers are sparser.
#[derive(Debug, Clone)]
pub struct HnswLayer {
    /// Layer level (0 = base layer)
    level: u8,

    /// Maximum connections per node in this layer
    max_connections: usize,

    /// Nodes in this layer: node_id -> connections
    nodes: HashMap<u64, HashSet<u64>>,

    /// Entry points for efficient navigation (sorted for deterministic search)
    entry_points: Vec<u64>,

    /// Total number of vectors indexed in the layer
    vector_count: usize,

    /// Nodes that have been modified since the last persist_topology call.
    /// Used for incremental persistence — only dirty nodes are written to DB.
    dirty_nodes: HashSet<u64>,
}

impl HnswLayer {
    /// Create a new HNSW layer
    ///
    /// # Arguments
    ///
    /// * `level` - Layer level (0 = base layer)
    /// * `base_connections` - Maximum connections in base layer (M parameter)
    ///
    /// # Returns
    ///
    /// New HnswLayer instance with empty node set
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sqlitegraph::hnsw::layer::HnswLayer;
    ///
    /// let layer = HnswLayer::new(0, 16);
    /// assert_eq!(layer.level(), 0);
    /// assert_eq!(layer.max_connections(), 16);
    /// assert_eq!(layer.node_count(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(level: u8, base_connections: usize) -> Self {
        let max_connections = Self::compute_max_connections(level, base_connections);

        Self {
            level,
            max_connections,
            nodes: HashMap::new(),
            entry_points: Vec::new(),
            vector_count: 0,
            dirty_nodes: HashSet::new(),
        }
    }

    /// Compute maximum connections for a given layer level
    ///
    /// Higher layers have exponentially fewer connections.
    /// Formula: M / 2^level, minimum 1 connection.
    ///
    /// # Arguments
    ///
    /// * `level` - Layer level
    /// * `base_connections` - Base layer connections (M)
    ///
    /// # Returns
    ///
    /// Maximum connections for this layer
    fn compute_max_connections(level: u8, base_connections: usize) -> usize {
        let result = base_connections.checked_shr(level as u32).unwrap_or(0);
        result.max(1)
    }

    /// Get layer level
    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn nodes_iter(&self) -> impl Iterator<Item = (&u64, &HashSet<u64>)> {
        self.nodes.iter()
    }

    /// Get maximum connections per node
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Get number of nodes in this layer
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get total vector count in this layer
    pub fn vector_count(&self) -> usize {
        self.vector_count
    }

    /// Check if a node exists in this layer
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node identifier to check
    ///
    /// # Returns
    ///
    /// true if node exists, false otherwise
    pub fn contains_node(&self, node_id: u64) -> bool {
        self.nodes.contains_key(&node_id)
    }

    /// Get connections for a node without returning an error if missing.
    ///
    /// Used by incremental persistence to look up dirty nodes efficiently.
    pub(crate) fn get_node_connections(&self, node_id: u64) -> Option<&HashSet<u64>> {
        self.nodes.get(&node_id)
    }

    /// Get connections for a specific node
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node identifier
    ///
    /// # Returns
    ///
    /// Ok(HashSet) with connections, or Err if node doesn't exist
    pub fn get_connections(&self, node_id: u64) -> Result<&HashSet<u64>, HnswError> {
        self.nodes
            .get(&node_id)
            .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(node_id)))
    }

    /// Add a node to this layer
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node identifier (must be sequential starting from 0)
    ///
    /// # Returns
    ///
    /// Ok(()) if successful, Err if node_id is invalid
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sqlitegraph::hnsw::layer::HnswLayer;
    ///
    /// let mut layer = HnswLayer::new(0, 16);
    ///
    /// // Add nodes sequentially
    /// layer.add_node(0)?;
    /// layer.add_node(1)?;
    ///
    /// assert_eq!(layer.node_count(), 2);
    /// assert!(layer.contains_node(1));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn add_node(&mut self, node_id: u64) -> Result<(), HnswError> {
        if self.nodes.contains_key(&node_id) {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(node_id)));
        }

        self.nodes.insert(node_id, HashSet::new());
        self.dirty_nodes.insert(node_id);
        self.vector_count += 1;

        // Add as entry point if this is one of the first nodes
        if self.entry_points.len() < self.max_connections {
            self.entry_points.push(node_id);
        }

        Ok(())
    }

    /// Add a unidirectional connection from node_a to node_b (without bidirectional)
    ///
    /// This is used during HNSW insertion to carefully control both directions
    /// of connections separately with proper pruning.
    ///
    /// # Arguments
    ///
    /// * `from_node` - Source node identifier
    /// * `to_node` - Target node identifier
    ///
    /// # Returns
    ///
    /// Ok(()) if successful, Err if operation fails
    pub(crate) fn add_one_way_connection(
        &mut self,
        from_node: u64,
        to_node: u64,
    ) -> Result<(), HnswError> {
        if from_node == to_node {
            return Err(HnswError::Index(HnswIndexError::SelfConnection(from_node)));
        }

        if !self.contains_node(from_node) {
            return Err(HnswError::Index(HnswIndexError::NodeNotFound(from_node)));
        }
        if !self.contains_node(to_node) {
            return Err(HnswError::Index(HnswIndexError::NodeNotFound(to_node)));
        }

        self.nodes
            .get_mut(&from_node)
            .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(from_node)))?
            .insert(to_node);
        self.dirty_nodes.insert(from_node);
        Ok(())
    }

    /// Add a bidirectional connection between two nodes
    ///
    /// # Arguments
    ///
    /// * `node_a` - First node identifier
    /// * `node_b` - Second node identifier
    ///
    /// # Returns
    ///
    /// Ok(()) if successful, Err if operation fails
    ///
    /// # Notes
    ///
    /// - Both nodes must exist in this layer
    /// - Connections are bidirectional (added to both nodes)
    /// - May prune existing connections if limit exceeded
    pub fn add_connection(&mut self, node_a: u64, node_b: u64) -> Result<(), HnswError> {
        if node_a == node_b {
            return Err(HnswError::Index(HnswIndexError::SelfConnection(node_a)));
        }

        // Check both nodes exist
        if !self.contains_node(node_a) {
            return Err(HnswError::Index(HnswIndexError::NodeNotFound(node_a)));
        }
        if !self.contains_node(node_b) {
            return Err(HnswError::Index(HnswIndexError::NodeNotFound(node_b)));
        }

        // Add bidirectional connection
        self.nodes
            .get_mut(&node_a)
            .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(node_a)))?
            .insert(node_b);
        self.nodes
            .get_mut(&node_b)
            .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(node_b)))?
            .insert(node_a);
        self.dirty_nodes.insert(node_a);
        self.dirty_nodes.insert(node_b);

        // Prune connections if needed
        self.prune_connections(node_a);
        self.prune_connections(node_b);

        Ok(())
    }

    /// Remove excessive connections from a node
    ///
    /// Keeps the most important connections up to max_connections.
    /// Prioritizes lower-indexed connections for deterministic behavior.
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node identifier
    fn prune_connections(&mut self, node_id: u64) {
        if !self.contains_node(node_id) {
            return;
        }

        let Some(connections) = self.nodes.get_mut(&node_id) else {
            return;
        };
        if connections.len() > self.max_connections {
            // NOTE: Pruning by node_id is a simplistic approach that can disconnect
            // the graph. Proper HNSW implementations prune by distance, keeping
            // the nearest neighbors. This is a known limitation.
            //
            // For better results, increase M (max_connections) to allow more
            // connections per node, which improves graph connectivity.

            // Convert to sorted Vec for deterministic pruning
            let mut conn_vec: Vec<u64> = connections.iter().cloned().collect();
            conn_vec.sort_unstable();

            // Keep only first max_connections items (lowest node_ids)
            conn_vec.truncate(self.max_connections);

            // Update the connections set
            *connections = conn_vec.into_iter().collect();
            self.dirty_nodes.insert(node_id);
        }
    }

    /// Prune connections based on distance, keeping only the nearest neighbors
    ///
    /// This is called from outside the layer with distance information
    /// to properly prune connections based on actual similarity.
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node whose connections to prune
    /// * `connection_distances` - Map of connection node_id to distance
    pub fn prune_connections_by_distance(
        &mut self,
        node_id: u64,
        connection_distances: &std::collections::HashMap<u64, f32>,
    ) {
        if !self.contains_node(node_id) {
            return;
        }

        let Some(connections) = self.nodes.get_mut(&node_id) else {
            return;
        };
        if connections.len() > self.max_connections {
            // Sort by distance, keep the closest ones
            let mut conn_with_dist: Vec<(u64, f32)> = connections
                .iter()
                .filter_map(|&id| connection_distances.get(&id).map(|&d| (id, d)))
                .collect();

            conn_with_dist.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });

            // Keep only the closest max_connections
            let to_keep: HashSet<u64> = conn_with_dist
                .into_iter()
                .take(self.max_connections)
                .map(|(id, _)| id)
                .collect();

            *connections = to_keep;
            self.dirty_nodes.insert(node_id);
        }
    }

    /// Get entry points for layer navigation
    ///
    /// Entry points are nodes that provide good starting positions
    /// for search operations in this layer.
    ///
    /// # Returns
    ///
    /// Slice of entry point node IDs (sorted deterministically)
    pub fn get_entry_points(&self) -> &[u64] {
        &self.entry_points
    }

    /// Update entry points after new node insertion
    ///
    /// Strategically selects nodes that provide good navigation
    /// coverage across the layer.
    ///
    /// # Arguments
    ///
    /// * `new_node_id` - Recently added node ID
    pub fn update_entry_points(&mut self, new_node_id: u64) {
        if !self.contains_node(new_node_id) {
            return;
        }

        // Simple strategy: keep nodes with highest connectivity
        let mut candidates: Vec<(u64, usize)> = self
            .nodes
            .iter()
            .map(|(id, connections)| (*id, connections.len()))
            .collect();

        candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        self.entry_points = candidates
            .iter()
            .take(self.max_connections)
            .map(|(id, _)| *id)
            .collect();
    }

    /// Check if this layer is the base layer (level 0)
    pub fn is_base_layer(&self) -> bool {
        self.level == 0
    }

    /// Get memory usage estimate in bytes
    ///
    /// # Returns
    ///
    /// Estimated memory usage for the layer data structures
    pub fn memory_usage(&self) -> usize {
        // Base overhead + nodes + connections + entry_points
        let base_overhead = std::mem::size_of::<Self>();
        let nodes_size = self.nodes.len() * std::mem::size_of::<HashSet<u64>>();
        let connections_size: usize = self
            .nodes
            .values()
            .map(|conns| conns.len() * std::mem::size_of::<u64>())
            .sum();
        let entry_points_size = self.entry_points.len() * std::mem::size_of::<u64>();

        base_overhead + nodes_size + connections_size + entry_points_size
    }

    /// Clear all data from the layer
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.entry_points.clear();
        self.vector_count = 0;
        self.dirty_nodes.clear();
    }

    /// Return the set of node IDs that have been modified since the last persist.
    pub fn dirty_nodes(&self) -> &HashSet<u64> {
        &self.dirty_nodes
    }

    /// Clear the dirty-node set after a successful persist.
    pub fn clear_dirty(&mut self) {
        self.dirty_nodes.clear();
    }

    pub fn remove_node(&mut self, node_id: u64) {
        if !self.nodes.contains_key(&node_id) {
            return;
        }
        let Some(connections) = self.nodes.get_mut(&node_id) else {
            return;
        };
        connections.clear();
        self.dirty_nodes.insert(node_id);
        for (other_id, conns) in self.nodes.iter_mut() {
            if *other_id != node_id && conns.remove(&node_id) {
                self.dirty_nodes.insert(*other_id);
            }
        }
        self.entry_points.retain(|&ep| ep != node_id);
        self.vector_count = self.vector_count.saturating_sub(1);
    }

    /// Get layer statistics for monitoring
    ///
    /// # Returns
    ///
    /// Tuple of (node_count, total_connections, avg_connections_per_node)
    pub fn get_statistics(&self) -> (usize, usize, f32) {
        let node_count = self.nodes.len();
        let total_connections: usize = self.nodes.values().map(|conns| conns.len()).sum();

        let avg_connections = if node_count > 0 {
            total_connections as f32 / node_count as f32
        } else {
            0.0
        };

        (node_count, total_connections, avg_connections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_creation() {
        let layer = HnswLayer::new(0, 16);
        assert_eq!(layer.level(), 0);
        assert_eq!(layer.max_connections(), 16);
        assert_eq!(layer.node_count(), 0);
        assert_eq!(layer.vector_count(), 0);
        assert!(layer.is_base_layer());
    }

    #[test]
    fn test_layer_level_scaling() {
        // Test that higher layers have fewer connections
        let layer0 = HnswLayer::new(0, 32);
        let layer1 = HnswLayer::new(1, 32);
        let layer2 = HnswLayer::new(2, 32);

        assert_eq!(layer0.max_connections(), 32);
        assert_eq!(layer1.max_connections(), 16);
        assert_eq!(layer2.max_connections(), 8);
    }

    #[test]
    fn test_layer_level_scaling_minimum() {
        // Test that minimum is 1 connection
        let layer10 = HnswLayer::new(10, 16);
        assert_eq!(layer10.max_connections(), 1);
    }

    #[test]
    fn test_add_node_sequential() {
        let mut layer = HnswLayer::new(0, 8);

        layer.add_node(0).unwrap();
        layer.add_node(1).unwrap();

        assert_eq!(layer.node_count(), 2);
        assert!(layer.contains_node(0));
        assert!(layer.contains_node(1));
        assert!(!layer.contains_node(2));
    }

    #[test]
    fn test_add_node_out_of_order() {
        let mut layer = HnswLayer::new(0, 8);

        layer.add_node(0).unwrap();
        layer.add_node(7756).unwrap();

        assert!(layer.contains_node(7756));
        assert_eq!(layer.node_count(), 2);

        let dup = layer.add_node(0);
        assert!(dup.is_err());
    }

    #[test]
    fn test_add_connection_success() {
        let mut layer = HnswLayer::new(0, 4);

        layer.add_node(0).unwrap();
        layer.add_node(1).unwrap();
        layer.add_node(2).unwrap();

        // Add bidirectional connection
        layer.add_connection(0, 1).unwrap();

        // Verify bidirectional connection
        assert!(layer.get_connections(0).unwrap().contains(&1));
        assert!(layer.get_connections(1).unwrap().contains(&0));
        assert!(!layer.get_connections(2).unwrap().contains(&0));
    }

    #[test]
    fn test_add_connection_self_connection() {
        let mut layer = HnswLayer::new(0, 4);

        layer.add_node(0).unwrap();

        let result = layer.add_connection(0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_connection_nonexistent_node() {
        let mut layer = HnswLayer::new(0, 4);

        layer.add_node(0).unwrap();

        let result = layer.add_connection(0, 1); // Node 1 doesn't exist
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_pruning() {
        let mut layer = HnswLayer::new(0, 2); // Max 2 connections

        // Create nodes and connect them all
        for i in 0..4 {
            layer.add_node(i).unwrap();
        }

        // Node 0 connects to 1, 2, 3 (exceeds limit)
        layer.add_connection(0, 1).unwrap();
        layer.add_connection(0, 2).unwrap();
        layer.add_connection(0, 3).unwrap();

        // Should be pruned to only 2 connections
        let connections = layer.get_connections(0).unwrap();
        assert_eq!(connections.len(), 2);

        // Should keep the lowest numbered connections (deterministic)
        assert!(connections.contains(&1));
        assert!(connections.contains(&2));
        assert!(!connections.contains(&3));
    }

    #[test]
    fn test_entry_points_initial() {
        let mut layer = HnswLayer::new(0, 3);

        // First few nodes become entry points
        layer.add_node(0).unwrap();
        layer.add_node(1).unwrap();

        let entry_points = layer.get_entry_points();
        assert_eq!(entry_points.len(), 2);
        assert!(entry_points.contains(&0));
        assert!(entry_points.contains(&1));
    }

    #[test]
    fn test_update_entry_points() {
        let mut layer = HnswLayer::new(0, 2);

        // Add nodes
        for i in 0..5 {
            layer.add_node(i).unwrap();
        }

        // Create different connectivity levels
        layer.add_connection(0, 1).unwrap();
        layer.add_connection(0, 2).unwrap();
        layer.add_connection(1, 3).unwrap();

        // Node 0 has 2 connections, node 1 has 2, others have 1
        layer.update_entry_points(4);

        let entry_points = layer.get_entry_points();
        assert_eq!(entry_points.len(), 2);

        // Should prioritize nodes with highest connectivity
        assert!(entry_points.contains(&0));
        assert!(entry_points.contains(&1));
    }

    #[test]
    fn test_get_connections_nonexistent() {
        let layer = HnswLayer::new(0, 4);

        let result = layer.get_connections(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_memory_usage() {
        let mut layer = HnswLayer::new(0, 4);

        // Empty layer
        let base_usage = layer.memory_usage();
        assert!(base_usage > 0);

        // Add nodes
        for i in 0..3 {
            layer.add_node(i).unwrap();
        }

        let with_nodes = layer.memory_usage();
        assert!(with_nodes > base_usage);

        // Add connections
        layer.add_connection(0, 1).unwrap();
        layer.add_connection(1, 2).unwrap();

        let with_connections = layer.memory_usage();
        assert!(with_connections > with_nodes);
    }

    #[test]
    fn test_get_statistics() {
        let mut layer = HnswLayer::new(0, 4);

        // Empty layer
        let (nodes, total, avg) = layer.get_statistics();
        assert_eq!(nodes, 0);
        assert_eq!(total, 0);
        assert_eq!(avg, 0.0);

        // Add nodes and connections
        for i in 0..3 {
            layer.add_node(i).unwrap();
        }

        layer.add_connection(0, 1).unwrap();
        layer.add_connection(1, 2).unwrap();

        let (nodes, total, avg) = layer.get_statistics();
        assert_eq!(nodes, 3);
        assert_eq!(total, 4); // 2 connections per link = 4 total
        assert!((avg - 1.33).abs() < 0.1); // 4/3 = 1.33 avg per node
    }

    #[test]
    fn test_clear_layer() {
        let mut layer = HnswLayer::new(0, 4);

        for i in 0..3 {
            layer.add_node(i).unwrap();
        }

        layer.add_connection(0, 1).unwrap();

        assert_eq!(layer.node_count(), 3);
        assert_eq!(layer.vector_count(), 3);
        assert!(!layer.get_entry_points().is_empty());

        layer.clear();

        assert_eq!(layer.node_count(), 0);
        assert_eq!(layer.vector_count(), 0);
        assert!(layer.get_entry_points().is_empty());
    }

    #[test]
    fn test_higher_layer_properties() {
        let layer = HnswLayer::new(3, 16);

        assert_eq!(layer.level(), 3);
        assert_eq!(layer.max_connections(), 2); // 16 / 2^3 = 2
        assert!(!layer.is_base_layer());
    }
}
