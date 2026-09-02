// HNSW Index Persistence Operations
//
// This module contains persistence operations for HnswIndex.
// Includes metadata save/load, vector loading, and database operations.
//
// Note: This file is included via include! macro in index.rs
// All imports are inherited from the parent module

// Note: HnswIndex is defined in the parent (index.rs)
// This file is included via include! macro, so types are available in scope

impl HnswIndex {
    /// Save index metadata to database
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure
    pub fn save_metadata(&self, conn: &rusqlite::Connection) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let distance_metric_str = self.config.distance_metric.as_str();

        // Check if index already exists
        let existing_id = Self::get_index_id(conn, &self.name)?;

        if let Some(index_id) = existing_id {
            // Update existing index - use positional params with correct types
            conn.execute(
                "UPDATE hnsw_indexes SET dimension = ?1, m = ?2, ef_construction = ?3, \
                 distance_metric = ?4, vector_count = ?5, updated_at = ?6 WHERE id = ?7",
                rusqlite::params![
                    self.config.dimension as i64,
                    self.config.m as i64,
                    self.config.ef_construction as i64,
                    distance_metric_str,
                    self.vector_count as i64,
                    now,
                    index_id,
                ],
            ).map_err(|e| HnswError::Storage(
                HnswStorageError::DatabaseError(e.to_string())
            ))?;
        } else {
            // Insert new index
            conn.execute(
                "INSERT INTO hnsw_indexes \
                 (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    &self.name,
                    self.config.dimension as i64,
                    self.config.m as i64,
                    self.config.ef_construction as i64,
                    distance_metric_str,
                    self.vector_count as i64,
                    now,
                    now,
                ],
            ).map_err(|e| HnswError::Storage(
                HnswStorageError::DatabaseError(e.to_string())
            ))?;
        }

        Ok(())
    }

    /// Get index ID from database by name
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    /// * `name` - Index name
    ///
    /// # Returns
    ///
    /// Some(index_id) if found, None if not found
    pub(crate) fn get_index_id(conn: &rusqlite::Connection, name: &str) -> Result<Option<i64>, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let id = conn
            .query_row(
                "SELECT id FROM hnsw_indexes WHERE name = ?",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| HnswError::Storage(
                HnswStorageError::DatabaseError(e.to_string())
            ))?;

        Ok(id)
    }

    /// Load index metadata from database
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    /// * `name` - Index name
    ///
    /// # Returns
    ///
    /// Loaded HnswIndex with metadata (vectors not loaded yet)
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure or if index not found
    pub fn load_metadata(conn: &rusqlite::Connection, name: &str) -> Result<Self, crate::hnsw::errors::HnswError> {
        use crate::hnsw::{layer::HnswLayer, neighborhood::NeighborhoodSearch, storage::InMemoryVectorStorage};

        // Query index metadata
        let (dimension, m, ef_construction, distance_metric_str, vector_count) = conn
            .query_row(
                "SELECT dimension, m, ef_construction, distance_metric, vector_count
                 FROM hnsw_indexes WHERE name = ?",
                [name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as usize,
                        row.get::<_, i64>(1)? as usize,
                        row.get::<_, i64>(2)? as usize,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)? as usize,
                    ))
                },
            )
            .optional()
            .map_err(|e| {
                let stor_err = crate::hnsw::errors::HnswStorageError::DatabaseError(e.to_string());
                crate::hnsw::errors::HnswError::Storage(stor_err)
            })?
            .ok_or_else(|| {
                let stor_err = crate::hnsw::errors::HnswStorageError::VectorNotFound(0);
                crate::hnsw::errors::HnswError::Storage(stor_err)
            })?;

        // Parse distance metric
        let distance_metric = Self::parse_distance_metric(&distance_metric_str)?;

        // Build config - use single-layer mode for safety
        let config = crate::hnsw::config::HnswConfig {
            dimension,
            m,
            ef_construction,
            ef_search: ef_construction, // Default to ef_construction
            ml: 16, // Default max layers
            distance_metric,
            enable_multilayer: false, // Disable multilayer for loaded indexes (plan 02)
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        };

        // Create index with loaded config
        let storage = Box::new(InMemoryVectorStorage::new());
        let mut layers = Vec::with_capacity(config.ml as usize);
        for level in 0..config.ml {
            let max_connections = if level == 0 {
                config.m
            } else {
                (config.m / 2usize.pow(level as u32)).max(1)
            };
            layers.push(HnswLayer::new(level, max_connections));
        }

        let search_engine = NeighborhoodSearch::new(config.distance_metric);

        // No level distributor for loaded indexes (enable_multilayer=false)
        let level_distributor = None;

        // No multi-layer manager for loaded indexes (enable_multilayer=false)
        let multi_layer_manager = None;

        Ok(Self {
            name: name.to_string(),
            config,
            layers,
            storage,
            entry_points: Vec::new(),
            vector_count,
            search_engine,
            level_distributor,
            multi_layer_manager,
            vector_cache: HashMap::new(),
            insert_count: std::sync::atomic::AtomicU64::new(0),
            search_count: std::sync::atomic::AtomicU64::new(0),
            vector_cache_hits: std::sync::atomic::AtomicU64::new(0),
            vector_cache_misses: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Load all vectors from database and rebuild HNSW index
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure or vector loading errors
    ///
    /// # Note
    ///
    /// This method loads all vectors from the database and rebuilds the HNSW
    /// graph structure by inserting each vector. The O(N log N) rebuild cost
    /// is a trade-off for simpler implementation compared to persisting layers.
    pub fn load_vectors_and_rebuild(&mut self, conn: &rusqlite::Connection) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let index_id = Self::get_index_id(conn, &self.name)?
            .ok_or({
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::VectorNotFound(0))
            })?;

        // Load all vectors from database
        let vectors = Self::load_vectors_from_db(conn, index_id)?;

        // Clear vector_count since we're rebuilding
        self.vector_count = 0;

        // Rebuild HNSW graph by inserting each vector
        for (vector_id, data, metadata) in vectors {
            // Use internal insert that doesn't re-persist to database
            self.insert_vector_internal(vector_id, &data, metadata)?;
        }

        // Nodes were rebuilt from DB — clear dirty flags so we don't
        // re-persist them on the next topology write.
        for layer in &mut self.layers {
            layer.clear_dirty();
        }

        // Rebuild traffic should not pollute runtime operation counters.
        self.insert_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.search_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.vector_cache_hits
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.vector_cache_misses
            .store(0, std::sync::atomic::Ordering::Relaxed);

        Ok(())
    }

    /// Load vectors from database
    ///
    /// # Arguments
    ///
    /// * `conn` - SQLite connection
    /// * `index_id` - Index ID in database
    ///
    /// # Returns
    ///
    /// Vector of (id, data, metadata) tuples
    pub(crate) fn load_vectors_from_db(
        conn: &rusqlite::Connection,
        index_id: i64,
    ) -> Result<Vec<(u64, Vec<f32>, Option<serde_json::Value>)>, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let mut stmt = conn
            .prepare("SELECT id, vector_data, metadata FROM hnsw_vectors WHERE index_id = ? ORDER BY id")
            .map_err(|e| {
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
            })?;

        let vectors = stmt
            .query_map([index_id], |row| {
                let id: i64 = row.get(0)?;
                let vector_data: Vec<u8> = row.get(1)?;
                let metadata_json: Option<String> = row.get(2)?;
                Ok((id as u64, vector_data, metadata_json))
            })
            .map_err(|e| {
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(
                    e.to_string(),
                ))
            })?
            .map(|row| {
                let (vector_id, vector_bytes, metadata_json) = row.map_err(|e| {
                    crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(
                        e.to_string(),
                    ))
                })?;

                // Deserialize vector
                let vector = if vector_bytes.len() % std::mem::size_of::<f32>() != 0 {
                    return Err(crate::hnsw::errors::HnswError::Storage(
                        HnswStorageError::InvalidVectorData,
                    ));
                } else {
                    bytemuck::cast_slice::<u8, f32>(&vector_bytes).to_vec()
                };

                // Parse metadata
                let metadata = metadata_json
                    .map(|s| {
                        serde_json::from_str(&s).map_err(|e| {
                            crate::hnsw::errors::HnswError::Storage(HnswStorageError::IoError(
                                format!("Failed to parse metadata: {}", e),
                            ))
                        })
                    })
                    .transpose()?;

                Ok((vector_id, vector, metadata))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(vectors)
    }

    /// Internal insert without persistence
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Explicit vector ID (from database)
    /// * `vector` - Vector data
    /// * `metadata` - Optional metadata
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    ///
    /// # Note
    ///
    /// This inserts into the HNSW graph structure using existing logic
    /// but skips database write since the vector is already persisted.
    pub(crate) fn insert_vector_internal(
        &mut self,
        vector_id: u64,
        vector: &[f32],
        metadata: Option<serde_json::Value>,
    ) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::{HnswError, HnswIndexError};

        // Validate vector dimension
        if vector.len() != self.config.dimension {
            return Err(HnswError::Index(HnswIndexError::VectorDimensionMismatch {
                expected: self.config.dimension,
                actual: vector.len(),
            }));
        }

        // Reject degenerate vectors before they reach the SIMD kernel. This path
        // re-inserts an already-stored vector by id (e.g. on index reload), so
        // the guard is defense-in-depth for legacy/third-party data. See
        // `distance_metric::validate_vector_for_metric` (Cosine-only).
        crate::hnsw::distance_metric::validate_vector_for_metric(
            self.config.distance_metric,
            vector,
        )
        .map_err(HnswError::Index)?;

        // Store the vector in memory (not to database)
        self.storage.store_vector_with_id(vector_id, vector.to_vec(), metadata)?;

        // Cache the vector for fast neighbor lookups during subsequent inserts.
        self.vector_cache.insert(vector_id, vector.to_vec());

        // Determine insertion layer and register with multi-layer manager
        // In multi-layer mode, the manager determines the level and creates mappings
        // In single-layer mode, we use the level distributor
        let insertion_level = if let Some(manager) = &mut self.multi_layer_manager {
            // Multi-layer mode: let the manager determine the level and create mappings
            let (highest_level, _layer_assignments) = manager.insert_vector(vector_id)?;
            highest_level
        } else {
            // Single-layer mode: use level distributor
            self.determine_insertion_level()
        };

        // Insert into layers from insertion_level down to 0
        for level in (0..=insertion_level).rev() {
            self.insert_into_layer(vector_id, level)?;
        }

        // Update entry points if this is a high-level vector
        if insertion_level >= self.entry_points.len() {
            self.entry_points.push(vector_id);
        }

        self.vector_count += 1;
        Ok(())
    }

    /// Load index with vectors from database
    ///
    /// # Arguments
    ///
    /// * `conn` - SQLite connection
    /// * `name` - Index name
    ///
    /// # Returns
    ///
    /// Fully loaded HnswIndex with all vectors
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure or if index not found
    ///
    /// # Note
    ///
    /// This is a convenience method that loads metadata and vectors in one call.
    pub fn load_with_vectors(conn: &rusqlite::Connection, name: &str) -> Result<Self, crate::hnsw::errors::HnswError> {
        let mut hnsw = Self::load_metadata(conn, name)?;
        hnsw.load_vectors_and_rebuild(conn)?;
        Ok(hnsw)
    }

    /// Parse distance metric from string
    ///
    /// # Arguments
    /// * `s` - String representation of distance metric
    ///
    /// # Returns
    ///
    /// Parsed DistanceMetric
    ///
    /// # Errors
    ///
    /// Returns HnswError for unknown metrics
    pub(crate) fn parse_distance_metric(s: &str) -> Result<crate::hnsw::distance_metric::DistanceMetric, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        match s {
            "cosine" => Ok(crate::hnsw::distance_metric::DistanceMetric::Cosine),
            "euclidean" => Ok(crate::hnsw::distance_metric::DistanceMetric::Euclidean),
            "dot_product" => Ok(crate::hnsw::distance_metric::DistanceMetric::DotProduct),
            "manhattan" => Ok(crate::hnsw::distance_metric::DistanceMetric::Manhattan),
            _ => Err(crate::hnsw::errors::HnswError::Storage(
                HnswStorageError::IoError(format!(
                    "Unknown distance metric: {}", s
                )),
            )),
        }
    }

    /// List all HNSW indexes in the database
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    ///
    /// # Returns
    ///
    /// List of index names
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure
    pub fn list_indexes(conn: &rusqlite::Connection) -> Result<Vec<String>, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let mut stmt = conn
            .prepare("SELECT name FROM hnsw_indexes ORDER BY name")
            .map_err(|e| {
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
            })?;

        let index_names = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| {
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
            })?;

        Ok(index_names)
    }

    /// Delete an index from the database
    ///
    /// # Arguments
    /// * `conn` - SQLite connection
    /// * `name` - Index name
    ///
    /// # Returns
    ///
    /// Ok(()) if deleted or didn't exist
    ///
    /// # Errors
    ///
    /// Returns HnswError on database failure
    ///
    /// # Note
    ///
    /// CASCADE will automatically delete vectors, layers, and entry points
    pub fn delete_index(conn: &rusqlite::Connection, name: &str) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        conn.execute(
            "DELETE FROM hnsw_indexes WHERE name = ?",
            [name],
        )
        .map_err(|e| {
            crate::hnsw::errors::HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
        })?;

        Ok(())
    }

    /// Delete a vector by id.
    ///
    /// Invariant: `entry_points` stays non-empty while `vector_count > 0`. If
    /// the deleted vector was the current entry point, a replacement is
    /// re-elected deterministically (see [`Self::elect_entry_point`]) so the
    /// index remains searchable; deleting the last vector leaves the index
    /// empty with no entry point.
    pub fn delete_vector(&mut self, id: u64) -> Result<(), crate::hnsw::errors::HnswError> {
        let node_id = if let Some(manager) = &self.multi_layer_manager {
            manager.get_local_id(id, 0).unwrap_or(id)
        } else {
            id - 1
        };
        self.storage.delete_vector(id)?;
        // Evict from the in-memory vector cache too — `search` reads vectors
        // from it (`load_vectors_as_local_map`), so a stale entry would let a
        // deleted vector still be scored and returned.
        self.vector_cache.remove(&id);
        for layer in &mut self.layers {
            layer.remove_node(node_id);
        }
        self.entry_points.retain(|&ep| ep != id);
        self.vector_count = self.vector_count.saturating_sub(1);
        // If deleting `id` emptied the entry-point set while vectors remain,
        // re-elect one — otherwise `search` rejects a non-empty index with
        // `IndexNotInitialized`. Deleting the last vector intentionally leaves
        // the set empty (`vector_count == 0`).
        if self.entry_points.is_empty() && self.vector_count > 0 {
            // `extend` pushes 0 or 1 entry point (Option is an iterator).
            self.entry_points.extend(self.elect_entry_point());
        }
        self.persist_topology()?;
        Ok(())
    }

    /// Deterministically choose a new entry point from the surviving vectors:
    /// scan from the highest non-empty layer down, and within the first layer
    /// that holds a *living* node pick the lowest global id. The `min` makes
    /// the choice stable across identical index states (required: deterministic
    /// ordering). `search` skips empty upper layers, so an entry point at the
    /// highest non-empty layer keeps the greedy descent valid.
    ///
    /// `remove_node` clears a deleted node's connections but leaves its key in
    /// the layer map (a tombstone), so candidates are filtered against a
    /// **live-set oracle**: `vector_cache` when populated, falling back to
    /// `storage.list_vectors()` when the cache is empty (the memory-constrained
    /// restore rebuilds the topology but lazy-loads the cache). The fallback
    /// keeps the entry-point invariant *unconditional*. A deleted vector is
    /// absent from both — `delete_vector` removes it from storage before
    /// electing — so tombstones are skipped either way. The storage list is
    /// fetched **once**, not per candidate. Returns `None` only when no living
    /// vector remains.
    fn elect_entry_point(&self) -> Option<u64> {
        // Live-set oracle source: the in-memory cache when populated, else
        // storage (the constrained restore leaves `vector_cache` empty). Fetch
        // the storage list ONCE — never `get_vector` per candidate (O(N) calls).
        let storage_live: Option<std::collections::HashSet<u64>> = if self.vector_cache.is_empty() {
            self.storage.list_vectors().ok().map(|v| v.into_iter().collect())
        } else {
            None
        };
        for level in (0..self.layers.len()).rev() {
            let mut best: Option<u64> = None;
            for (&local_id, _) in self.layers[level].nodes_iter() {
                let Ok(global) = self.get_global_id_for_layer(level, local_id) else {
                    continue;
                };
                let is_live = match &storage_live {
                    Some(set) => set.contains(&global),
                    None => self.vector_cache.contains_key(&global),
                };
                if is_live {
                    best = Some(best.map_or(global, |b| b.min(global)));
                }
            }
            if best.is_some() {
                return best;
            }
        }
        None
    }

    pub fn persist_topology(&mut self) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let Some((conn, index_id)) = self.storage.as_sqlite_connection() else {
            return Ok(());
        };

        // Wrap all operations in a transaction for batch efficiency
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        let result = self.persist_topology_inner(conn, index_id);

        match &result {
            Ok(()) => {
                let _ = conn.execute_batch("COMMIT");
            }
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }

        if result.is_ok() {
            for layer in &mut self.layers {
                layer.clear_dirty();
            }
        }

        result
    }

    /// Internal topology persistence — caller must handle transaction.
    ///
    /// Only writes nodes that have been marked dirty since the last persist.
    /// This makes batch inserts O(batch_size) instead of O(total_nodes).
    fn persist_topology_inner(
        &self,
        conn: &rusqlite::Connection,
        index_id: i64,
    ) -> Result<(), crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let has_dirty = self.layers.iter().any(|l| !l.dirty_nodes().is_empty());

        if has_dirty {
            for (level, layer) in self.layers.iter().enumerate() {
                let dirty = layer.dirty_nodes();
                if dirty.is_empty() {
                    continue;
                }
                for &node_id in dirty {
                    if let Some(connections) = layer.get_node_connections(node_id) {
                        let connections_bytes: Vec<u8> = connections
                            .iter()
                            .flat_map(|c| c.to_le_bytes())
                            .collect();
                        conn.execute(
                            "INSERT OR REPLACE INTO hnsw_layers (index_id, layer_level, node_id, connections)
                             VALUES (?1, ?2, ?3, ?4)",
                            rusqlite::params![index_id, level as i64, node_id as i64, connections_bytes],
                        )
                        .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;
                    }
                }
            }
        }

        // Entry points are small; always rewrite for simplicity.
        conn.execute(
            "DELETE FROM hnsw_entry_points WHERE index_id = ?",
            [index_id],
        )
        .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        for (i, &ep) in self.entry_points.iter().enumerate() {
            conn.execute(
                "INSERT OR REPLACE INTO hnsw_entry_points (index_id, node_id, order_idx) VALUES (?1, ?2, ?3)",
                rusqlite::params![index_id, ep as i64, i as i64],
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;
        }

        Ok(())
    }

    pub fn restore_topology(&mut self) -> Result<bool, crate::hnsw::errors::HnswError> {
        use crate::hnsw::errors::HnswStorageError;

        let Some((conn, index_id)) = self.storage.as_sqlite_connection() else {
            return Ok(false);
        };

        let has_layers: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM hnsw_layers WHERE index_id = ? LIMIT 1)",
                [index_id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_layers {
            return Ok(false);
        }

        for layer in &mut self.layers {
            layer.clear();
        }
        self.entry_points.clear();

        let mut stmt = conn
            .prepare(
                "SELECT layer_level, node_id, connections FROM hnsw_layers WHERE index_id = ? ORDER BY layer_level, node_id",
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        let rows = stmt
            .query_map([index_id], |row| {
                let level: i64 = row.get(0)?;
                let node_id: i64 = row.get(1)?;
                let connections_blob: Vec<u8> = row.get(2)?;
                Ok((level, node_id, connections_blob))
            })
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        for row in rows {
            let (level, node_id, connections_blob) = row
                .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;
            let level_idx = level as usize;
            if level_idx >= self.layers.len() {
                continue;
            }
            let layer = &mut self.layers[level_idx];
            if !layer.contains_node(node_id as u64) {
                let _ = layer.add_node(node_id as u64);
            }
            let connections: std::collections::HashSet<u64> = connections_blob
                .chunks_exact(8)
                .map(|chunk| {
                    let bytes: [u8; 8] = chunk.try_into().unwrap_or([0; 8]);
                    u64::from_le_bytes(bytes)
                })
                .collect();
            for &neighbor in &connections {
                let _ = layer.add_connection(node_id as u64, neighbor);
            }
        }

        let mut ep_stmt = conn
            .prepare("SELECT node_id FROM hnsw_entry_points WHERE index_id = ? ORDER BY order_idx ASC")
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        self.entry_points = ep_stmt
            .query_map([index_id], |row| row.get::<_, i64>(0))
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?
            .map(|r| r.map(|v| v as u64))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        self.vector_count = self.layers.first().map(|l| l.node_count()).unwrap_or(0);

        // Loaded from DB — nothing is dirty.
        for layer in &mut self.layers {
            layer.clear_dirty();
        }

        // Populate vector_cache from storage so subsequent operations
        // (search, insert) don't need to query SQLite per vector.
        // Only cache if estimated memory fits within available RAM.
        self.vector_cache.clear();
        if let Ok(vector_ids) = self.storage.list_vectors() {
            let est_bytes = vector_ids.len() as u64 * (40 + self.config.dimension as u64 * 4);
            let available_bytes = available_memory_bytes();
            // Use at most 25% of available RAM for the vector cache.
            // Leave 75% for the OS, application, and other data structures.
            if available_bytes == 0 || est_bytes < available_bytes / 4 {
                self.vector_cache.reserve(vector_ids.len());
                for vid in vector_ids {
                    if let Ok(Some(vec)) = self.storage.get_vector(vid) {
                        self.vector_cache.insert(vid, vec);
                    }
                }
            }
            // else: skip caching, load_vectors_as_local_map will fall back
            // to per-query SQLite lookups (slower but memory-safe).
        }

        Ok(true)
    }
}
