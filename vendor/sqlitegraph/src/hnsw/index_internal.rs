// HNSW Index Internal Helpers
//
// This module contains internal helper methods for HnswIndex.
// Includes layer management, ID translation, validation, and layer insertion.
//
// Note: This file is included via include! macro in index.rs
// All imports are inherited from the parent module

// Note: HnswIndex is defined in the parent (index.rs)
// This file is included via include! macro, so types are available in scope

impl HnswIndex {
    /// Validate the HNSW configuration
    pub(crate) fn validate_config(config: &crate::hnsw::config::HnswConfig) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::{HnswError, HnswIndexError};

        if config.dimension == 0 {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(0)));
        }
        if config.m == 0 {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(0)));
        }
        if config.ef_construction < config.m {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(0)));
        }
        if config.ef_search == 0 {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(0)));
        }
        if config.ml == 0 {
            return Err(HnswError::Index(HnswIndexError::InvalidNodeId(0)));
        }
        Ok(())
    }

    /// Determine which layer to insert a vector into using exponential distribution
    pub(crate) fn determine_insertion_level(&mut self) -> usize {
        if self.config.enable_multilayer {
            if let Some(distributor) = &mut self.level_distributor {
                distributor.sample_level_internal()
            } else {
                0 // Fallback to single-layer if distributor not initialized
            }
        } else {
            0 // Single-layer mode for backward compatibility
        }
    }

    /// Insert a vector into a specific layer
    pub(crate) fn insert_into_layer(&mut self, vector_id: u64, level: usize) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::{HnswError, HnswIndexError};

        // Determine local node ID based on mode
        // In multi-layer mode: use LayerMappings for ID translation
        // In single-layer mode: direct conversion (vector_id - 1)
        let node_id = if let Some(manager) = &mut self.multi_layer_manager {
            // Multi-layer mode: use LayerMappings to get local ID
            manager.get_local_id(vector_id, level)
                .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(vector_id)))?
        } else {
            // Single-layer mode: direct 1-based to 0-based conversion
            vector_id - 1
        };

        // Ensure layer exists, create if necessary
        while level >= self.layers.len() {
            let new_level = self.layers.len() as u8;
            let max_connections = (self.config.m / 2_usize.pow(new_level as u32)).max(1);
            self.layers.push(crate::hnsw::layer::HnswLayer::new(new_level, max_connections));
        }

        // Add the node to the layer first (this makes it an entry point if it's one of the first nodes)
        {
            let layer = &mut self.layers[level];
            layer.add_node(node_id)?;
        }

        // For base layer, no need to connect to existing entry points if this is the first node
        if level == 0 && self.layers[level].node_count() == 1 {
            return Ok(());
        }

        // 3. Find entry points to start the search
        let global_entry_points = self.get_layer_entry_points(level);
        let entry_points: Vec<u64> = if let Some(manager) = &self.multi_layer_manager {
            // Multi-layer mode: use LayerMappings for ID translation
            global_entry_points
                .into_iter()
                .filter_map(|global_id| manager.get_local_id(global_id, level))
                .collect()
        } else {
            // Single-layer mode: direct 1-based to 0-based conversion
            global_entry_points
                .into_iter()
                .map(|global_id| global_id - 1)
                .collect()
        };

        if entry_points.is_empty() {
            // No entry points yet, this must be the first node
            return Ok(());
        }

        // 4. Search for the nearest neighbors to the new vector
        // Use a closure to look up vectors on-demand from vector_cache, avoiding
        // the expensive clone of all vectors into a local HashMap.
        let ef = self.config.ef_construction;
        let vector_cache = &self.vector_cache;
        let search_engine = &self.search_engine;
        let layer_ref = &self.layers[level];
        let new_vector = vector_cache
            .get(&vector_id)
            .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(vector_id)))?;
        self.vector_cache_hits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let search_result = if let Some(manager) = &self.multi_layer_manager {
            search_engine.search_layer_fn(
                layer_ref,
                new_vector,
                |local_id: u64| {
                    let global_id = manager.get_global_id(level, local_id)?;
                    vector_cache.get(&global_id).map(|v| v.as_slice())
                },
                &entry_points,
                ef,
            )?
        } else {
            search_engine.search_layer_fn(
                layer_ref,
                new_vector,
                |local_id: u64| {
                    let global_id = local_id + 1;
                    vector_cache.get(&global_id).map(|v| v.as_slice())
                },
                &entry_points,
                ef,
            )?
        };

        // 5. Select top M neighbors (limited by max_connections)
        let candidates = search_result.neighbors();
        let distances = search_result.distances();
        let m = self.layers[level].max_connections();

        // Build distance map for the new node's neighbors
        let mut neighbor_distances = std::collections::HashMap::new();
        for (i, &neighbor_id) in candidates.iter().enumerate() {
            if i < m {
                neighbor_distances.insert(neighbor_id, distances[i]);
            }
        }

        // 6. Add connections from new node to its nearest neighbors
        // The new node gets connections to its M nearest neighbors
        let layer = &mut self.layers[level];
        for &neighbor_id in neighbor_distances.keys() {
            if neighbor_id != node_id {
                layer.add_one_way_connection(node_id, neighbor_id)?;
            }
        }

        // Prune the new node's connections by distance (keeps closest)
        layer.prune_connections_by_distance(node_id, &neighbor_distances);

        // 7. Add reverse connections from neighbors to the new node
        // We use a more lenient pruning strategy for reverse connections to ensure
        // the graph remains well-connected. Only prune if we exceed 2*M connections.
        let max_reverse_conns = (m * 2).max(32); // More lenient limit
        for (&neighbor_id, _dist_to_neighbor) in neighbor_distances.iter() {
            if neighbor_id != node_id {
                // Add reverse connection (existing node -> new node)
                layer.add_one_way_connection(neighbor_id, node_id)?;

                // Only prune if significantly over limit
                if let Ok(existing_conns) = layer.get_connections(neighbor_id)
                    && existing_conns.len() > max_reverse_conns {
                        // Build distance map with small distance for new node to keep it
                        let mut reverse_distances = std::collections::HashMap::new();
                        reverse_distances.insert(node_id, 0.0); // Keep new connection
                        for &existing_id in existing_conns {
                            if existing_id != node_id {
                                reverse_distances.insert(existing_id, 1.0); // Existing connections
                            }
                        }
                        layer.prune_connections_by_distance(neighbor_id, &reverse_distances);
                    }
            }
        }

        Ok(())
    }

    /// Get entry points for a specific layer
    ///
    /// Returns global vector IDs (1-based) for vectors that are entry points
    /// in the specified layer. In multi-layer mode, uses the manager to
    /// translate local node IDs to global vector IDs.
    pub(crate) fn get_layer_entry_points(&self, level: usize) -> Vec<u64> {
        if self.layers.is_empty() {
            return Vec::new();
        }

        if level == self.layers.len() - 1 {
            // Top layer: return all entry points (already 1-based vector IDs)
            self.entry_points.clone()
        } else if level == 0 {
            // Base layer: use its own entry points
            let layer_entry_points = self.layers[level].get_entry_points();
            if let Some(manager) = &self.multi_layer_manager {
                // Multi-layer mode: convert local node IDs to global vector IDs
                layer_entry_points
                    .iter()
                    .filter_map(|&local_id| manager.get_global_id(level, local_id))
                    .collect()
            } else {
                // Single-layer mode: direct 0-based to 1-based conversion
                layer_entry_points
                    .iter()
                    .map(|&node_id| node_id + 1)
                    .collect()
            }
        } else {
            // Intermediate layers: use entry points from the layer above
            if level + 1 < self.layers.len() {
                let layer_entry_points = self.layers[level + 1].get_entry_points();
                if let Some(manager) = &self.multi_layer_manager {
                    // Multi-layer mode: convert local node IDs to global vector IDs
                    layer_entry_points
                        .iter()
                        .filter_map(|&local_id| manager.get_global_id(level + 1, local_id))
                        .collect()
                } else {
                    // Single-layer mode: direct 0-based to 1-based conversion
                    layer_entry_points
                        .iter()
                        .map(|&node_id| node_id + 1)
                        .collect()
                }
            } else {
                Vec::new()
            }
        }
    }

    /// Get local ID for a global vector ID in a specific layer
    ///
    /// In multi-layer mode, uses LayerMappings for ID translation.
    /// In single-layer mode, uses direct 1-based to 0-based conversion.
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Global vector ID (1-based)
    /// * `layer_id` - Layer ID (0-based)
    ///
    /// # Returns
    ///
    /// Local node ID for the layer
    pub(crate) fn get_local_id_for_layer(&self, vector_id: u64, layer_id: usize) -> Result<u64, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::{HnswError, HnswIndexError};

        if let Some(manager) = &self.multi_layer_manager {
            manager.get_local_id(vector_id, layer_id)
                .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(vector_id)))
        } else {
            // Single-layer mode: direct conversion
            Ok(vector_id - 1)
        }
    }

    /// Get global ID for a local node ID in a specific layer
    ///
    /// In multi-layer mode, uses LayerMappings for ID translation.
    /// In single-layer mode, uses direct 0-based to 1-based conversion.
    ///
    /// # Arguments
    ///
    /// * `layer_id` - Layer ID (0-based)
    /// * `local_id` - Local node ID (0-based)
    ///
    /// # Returns
    ///
    /// Global vector ID (1-based)
    pub(crate) fn get_global_id_for_layer(&self, layer_id: usize, local_id: u64) -> Result<u64, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::{HnswError, HnswIndexError};

        if let Some(manager) = &self.multi_layer_manager {
            manager.get_global_id(layer_id, local_id)
                .ok_or(HnswError::Index(HnswIndexError::InvalidNodeId(local_id)))
        } else {
            // Single-layer mode: direct conversion
            Ok(local_id + 1)
        }
    }

    /// Load vectors keyed by layer-local node IDs for a specific layer.
    ///
    /// In multi-layer mode, uses the manager's global→local mapping for `level`.
    /// In single-layer mode, uses `vector_id - 1` (local_id == vector_id - 1 by convention).
    /// Vectors not present in the given layer are omitted.
    pub(crate) fn load_vectors_as_local_map(&self, level: usize) -> Result<HashMap<u64, Vec<f32>>, crate::hnsw::errors::HnswError> {
        // Fast path: use incremental cache instead of querying SQLite per vector.
        // The cache is populated on store_vector, so it's always in sync with storage.
        if !self.vector_cache.is_empty() {
            self.vector_cache_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut vectors_map = HashMap::with_capacity(self.vector_cache.len());
            for (&vector_id, vector) in &self.vector_cache {
                let key = if let Some(manager) = &self.multi_layer_manager {
                    manager.get_local_id(vector_id, level)
                } else {
                    Some(vector_id - 1)
                };
                if let Some(k) = key {
                    vectors_map.insert(k, vector.clone());
                }
            }
            return Ok(vectors_map);
        }

        // Fallback: query SQLite per vector (slow, used when cache is empty
        // e.g. on low-memory systems where bulk cache load was skipped).
        self.vector_cache_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let vector_ids = self.storage.list_vectors()?;
        let mut vectors_map = HashMap::with_capacity(vector_ids.len());
        for vector_id in vector_ids {
            if let Ok(Some(vector)) = self.storage.get_vector(vector_id) {
                let key = if let Some(manager) = &self.multi_layer_manager {
                    manager.get_local_id(vector_id, level)
                } else {
                    Some(vector_id - 1)
                };
                if let Some(k) = key {
                    vectors_map.insert(k, vector);
                }
            }
        }
        Ok(vectors_map)
    }
}
