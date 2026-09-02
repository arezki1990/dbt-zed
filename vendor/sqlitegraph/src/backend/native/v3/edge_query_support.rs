use super::*;
use std::sync::atomic::Ordering;

#[path = "edge_query_support/io_support.rs"]
mod io_support;
#[path = "edge_query_support/weighted_support.rs"]
mod weighted_support;

impl V3EdgeStore {
    pub fn btree_lock(&self) -> &Arc<parking_lot::RwLock<BTreeManager>> {
        &self.btree
    }

    pub fn cache_lock(&self) -> &Arc<RwLock<HashMap<(i64, Direction), Arc<[i64]>>>> {
        &self.cache
    }

    pub fn cache_weighted_lock(
        &self,
    ) -> &Arc<RwLock<HashMap<(i64, Direction), Arc<[(i64, f32)]>>>> {
        &self.cache_weighted
    }

    pub fn edge_types_lock(&self) -> &Arc<RwLock<HashMap<(i64, i64, Direction), String>>> {
        &self.edge_types
    }

    pub fn page_size(&self) -> u64 {
        self.page_size as u64
    }

    /// Set the file coordinator for coordinated I/O
    ///
    /// When set, all file reads/writes go through this coordinator to prevent
    /// race conditions and avoid per-cache-miss file open/close overhead.
    /// The coordinator is also propagated to the internal BTreeManager so
    /// edge-cluster lookups share the same persistent handle.
    pub fn set_file_coordinator(&mut self, coordinator: Arc<FileCoordinator>) {
        self.btree.write().set_file_coordinator(coordinator.clone());
        self.file_coordinator = Some(coordinator);
    }

    /// Get neighbors from cache - returns Arc<[i64]> for zero-copy!
    ///
    /// IMPROVED: On cache miss, attempts to load from disk if db_path is set.
    /// This enables recovery after reopening the edge store.
    pub fn neighbors(&self, src: i64, dir: Direction) -> NativeResult<Arc<[i64]>> {
        let key = (src, dir);

        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .logical_neighbors_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // First check in-memory cache
        {
            let cache = self.cache.read();
            if let Some(neighbors) = cache.get(&key) {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "v3-forensics")]
                FORENSIC_COUNTERS
                    .edge_cache_hit_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(neighbors.clone()); // Arc clone is just pointer bump, no data copy
            }
        }

        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .edge_cache_miss_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Cache miss - try to load from disk if we have a db_path
        if let Some(ref db_path) = self.db_path
            && let Ok(neighbors) = self.load_neighbors_from_disk(src, dir, db_path)
        {
            #[cfg(feature = "v3-forensics")]
            if !neighbors.is_empty() {
                FORENSIC_COUNTERS
                    .edge_page_read_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if !neighbors.is_empty() {
                // Cache the loaded neighbors
                let mut cache = self.cache.write();
                cache.insert(key, neighbors.clone());
                return Ok(neighbors);
            }
        }

        Ok(Arc::from([])) // Empty slice, no allocation
    }

    /// Load neighbors from disk for recovery
    /// IMPORTANT: Also rebuilds edge_types HashMap from deserialized edge records
    /// CRITICAL FIX: Query B+Tree for page_id instead of using formula
    pub(crate) fn load_neighbors_from_disk(
        &self,
        src: i64,
        dir: Direction,
        db_path: &Path,
    ) -> NativeResult<Arc<[i64]>> {
        // CRITICAL FIX: Query B+Tree for page_id instead of calculating it
        // This prevents page ID collision with node storage
        let key = edge_key(src, dir);
        let btree = self.btree.read();

        // Try to get page_id from B+Tree
        let page_id = match btree.lookup(key) {
            Ok(Some(pid)) => pid,
            Ok(None) => {
                // No entry in B+Tree means no edges for this (src, dir)
                return Ok(Arc::from([]));
            }
            Err(_) => {
                // B+Tree lookup error - treat as no edges
                return Ok(Arc::from([]));
            }
        };
        drop(btree);

        match self.load_cluster_from_disk(page_id, src, dir, db_path) {
            Ok(cluster) => {
                let edges_with_types = cluster.edges_with_types();
                let mut edge_types = self.edge_types.write();
                for (dst, edge_type) in edges_with_types {
                    if let Some(et) = edge_type {
                        edge_types.insert((src, dst, dir), et);
                    }
                }

                let neighbors: Vec<i64> = cluster.dsts();
                Ok(Arc::from(neighbors.into_boxed_slice()))
            }
            Err(_) => Ok(Arc::from([])),
        }
    }

    /// Get outgoing neighbors
    pub fn outgoing(&self, src: i64) -> NativeResult<Arc<[i64]>> {
        self.neighbors(src, Direction::Outgoing)
    }

    /// Get incoming neighbors
    pub fn incoming(&self, src: i64) -> NativeResult<Arc<[i64]>> {
        self.neighbors(src, Direction::Incoming)
    }

    /// Get neighbors filtered by edge type
    /// Returns only neighbors connected by edges matching the specified edge_type
    pub fn neighbors_filtered(
        &self,
        src: i64,
        dir: Direction,
        edge_type: &str,
    ) -> NativeResult<Arc<[i64]>> {
        // Get all neighbors first
        let all_neighbors = self.neighbors(src, dir)?;

        // Filter by edge type
        let edge_types = self.edge_types.read();
        let filtered: Vec<i64> = all_neighbors
            .iter()
            .filter(|&&dst| {
                edge_types
                    .get(&(src, dst, dir))
                    .map(|stored_type| stored_type == edge_type)
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        Ok(Arc::from(filtered.into_boxed_slice()))
    }

    /// Get the edge type for a specific edge
    pub fn get_edge_type(&self, src: i64, dst: i64, dir: Direction) -> Option<String> {
        let edge_types = self.edge_types.read();
        edge_types.get(&(src, dst, dir)).cloned()
    }
}
