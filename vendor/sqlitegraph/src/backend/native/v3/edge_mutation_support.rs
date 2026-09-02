use super::*;

impl V3EdgeStore {
    /// Get or create a dirty cluster, loading from disk if it already exists to avoid dropping previous edges.
    fn get_or_create_dirty_cluster<'m>(
        &self,
        dirty: &'m mut std::collections::HashMap<(i64, Direction), V3EdgeCluster>,
        src: i64,
        dir: Direction,
    ) -> NativeResult<&'m mut V3EdgeCluster> {
        let cache_key = (src, dir);
        if dirty.contains_key(&cache_key) {
            return dirty
                .get_mut(&cache_key)
                .ok_or_else(|| NativeBackendError::InvalidOperation {
                    context: format!(
                        "dirty edge cluster missing despite existing key for src={} dir={:?}",
                        src, dir
                    ),
                });
        }

        let key = edge_key(src, dir);
        let lookup_res = {
            let btree = self.btree.read();
            btree.lookup(key)
        };

        let mut existing_cluster = None;
        let page_id_to_use = match lookup_res {
            Ok(Some(pid)) => {
                if let Some(ref db_path) = self.db_path
                    && let Ok(mut cluster) = self.load_cluster_from_disk(pid, src, dir, db_path)
                {
                    cluster.page_id = 0;
                    existing_cluster = Some(cluster);
                }
                pid
            }
            Ok(None) | Err(_) => 0,
        };

        let cluster =
            existing_cluster.unwrap_or_else(|| V3EdgeCluster::new(src, dir, page_id_to_use));
        dirty.insert(cache_key, cluster);
        dirty
            .get_mut(&cache_key)
            .ok_or_else(|| NativeBackendError::InvalidOperation {
                context: format!(
                    "dirty edge cluster missing after insertion for src={} dir={:?}",
                    src, dir
                ),
            })
    }

    /// Insert an edge - uses interior mutability via RwLock, takes &self!
    ///
    /// # SEMANTIC CONSTRAINT
    /// The edge_types HashMap is keyed by (src, dst, dir). This means:
    /// - Only ONE edge type can exist between a given (src, dst, dir) tuple
    /// - Inserting a second edge between same endpoints with different type will OVERWRITE
    /// - This is intentional for V3's simple tuple-key model
    /// - If multi-edge support is needed, the key model must change to use edge_id
    pub fn insert_edge(
        &self,
        src: i64,
        dst: i64,
        dir: Direction,
        edge_type: Option<String>,
    ) -> NativeResult<()> {
        let cache_key = (src, dir);
        let mut cache = self.cache.write();

        if let Some(neighbors) = cache.get_mut(&cache_key) {
            let mut vec: Vec<i64> = neighbors.to_vec();
            if !vec.contains(&dst) {
                vec.push(dst);
                *neighbors = Arc::from(vec);
            }
        } else {
            let mut vec = Vec::new();
            if let Some(ref db_path) = self.db_path
                && let Ok(existing) = self.load_neighbors_from_disk(src, dir, db_path)
            {
                vec = existing.to_vec();
            }
            if !vec.contains(&dst) {
                vec.push(dst);
            }
            cache.insert(cache_key, Arc::from(vec));
        }

        {
            let mut cache_weighted = self.cache_weighted.write();
            if let Some(neighbors) = cache_weighted.get_mut(&cache_key) {
                let mut vec = neighbors.to_vec();
                if !vec.iter().any(|(n, _)| *n == dst) {
                    vec.push((dst, 1.0));
                }
                vec.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                *neighbors = Arc::from(vec);
            } else {
                let mut vec = Vec::new();
                if let Some(ref db_path) = self.db_path
                    && let Ok(existing) = self.load_neighbors_weighted_from_disk(src, dir, db_path)
                {
                    vec = existing.to_vec();
                }
                if !vec.iter().any(|(n, _)| *n == dst) {
                    vec.push((dst, 1.0));
                }
                vec.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                cache_weighted.insert(cache_key, Arc::from(vec));
            }
        }

        if let Some(ref edge_type_str) = edge_type {
            let mut edge_types = self.edge_types.write();
            let key = (src, dst, dir);
            if let Some(existing_type) = edge_types.get(&key)
                && existing_type != edge_type_str
            {
                eprintln!(
                    "WARNING: V3EdgeStore inserting edge_type '{}' for ({}, {}, {:?}), overwriting existing type '{}'. This is a known limitation of tuple-key model.",
                    edge_type_str, src, dst, dir, existing_type
                );
            }

            edge_types.insert(key, edge_type_str.clone());
        } else {
            let mut edge_types = self.edge_types.write();
            edge_types.remove(&(src, dst, dir));
        }

        let page_id = {
            let mut dirty = self.dirty_clusters.write();
            let cluster = self.get_or_create_dirty_cluster(&mut dirty, src, dir)?;
            let cluster_page_id = cluster.page_id;
            cluster.add_edge(dst, edge_type);
            cluster_page_id
        };

        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            let _ = wal_guard.edge_insert(src, dst, dir as u8, page_id);
        }

        Ok(())
    }

    /// Insert a weighted edge
    pub fn insert_edge_weighted(
        &self,
        src: i64,
        dst: i64,
        dir: Direction,
        edge_type: Option<String>,
        weight: f32,
    ) -> NativeResult<()> {
        let cache_key = (src, dir);

        {
            let mut cache = self.cache.write();
            if let Some(neighbors) = cache.get_mut(&cache_key) {
                let mut vec: Vec<i64> = neighbors.to_vec();
                if !vec.contains(&dst) {
                    vec.push(dst);
                    *neighbors = Arc::from(vec);
                }
            } else {
                let mut vec = Vec::new();
                if let Some(ref db_path) = self.db_path
                    && let Ok(existing) = self.load_neighbors_from_disk(src, dir, db_path)
                {
                    vec = existing.to_vec();
                }
                if !vec.contains(&dst) {
                    vec.push(dst);
                }
                cache.insert(cache_key, Arc::from(vec));
            }
        }

        {
            let mut cache_weighted = self.cache_weighted.write();
            if let Some(neighbors) = cache_weighted.get_mut(&cache_key) {
                let mut vec = neighbors.to_vec();
                if let Some(pos) = vec.iter().position(|(n, _)| *n == dst) {
                    vec[pos].1 = weight;
                } else {
                    vec.push((dst, weight));
                }
                vec.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                *neighbors = Arc::from(vec);
            } else {
                let mut vec = Vec::new();
                if let Some(ref db_path) = self.db_path
                    && let Ok(existing) = self.load_neighbors_weighted_from_disk(src, dir, db_path)
                {
                    vec = existing.to_vec();
                }
                if let Some(pos) = vec.iter().position(|(n, _)| *n == dst) {
                    vec[pos].1 = weight;
                } else {
                    vec.push((dst, weight));
                }
                vec.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                cache_weighted.insert(cache_key, Arc::from(vec));
            }
        }

        if let Some(ref edge_type_str) = edge_type {
            let mut edge_types = self.edge_types.write();
            edge_types.insert((src, dst, dir), edge_type_str.clone());
        } else {
            let mut edge_types = self.edge_types.write();
            edge_types.remove(&(src, dst, dir));
        }

        let page_id = {
            let mut dirty = self.dirty_clusters.write();
            let cluster = self.get_or_create_dirty_cluster(&mut dirty, src, dir)?;
            let cluster_page_id = cluster.page_id;
            cluster.add_edge_weighted(dst, edge_type, weight);
            cluster_page_id
        };

        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            let _ = wal_guard.edge_insert(src, dst, dir as u8, page_id);
        }

        Ok(())
    }
}
