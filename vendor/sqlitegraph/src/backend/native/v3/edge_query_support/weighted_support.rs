use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

impl V3EdgeStore {
    /// Get weighted neighbors from cache - returns Arc<[(i64, f32)]> for zero-copy!
    pub fn neighbors_weighted(&self, src: i64, dir: Direction) -> NativeResult<Arc<[(i64, f32)]>> {
        let key = (src, dir);

        {
            let cache = self.cache_weighted.read();
            if let Some(neighbors) = cache.get(&key) {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(neighbors.clone());
            }
        }

        self.cache_misses.fetch_add(1, Ordering::Relaxed);

        if let Some(ref db_path) = self.db_path
            && let Ok(neighbors) = self.load_neighbors_weighted_from_disk(src, dir, db_path)
        {
            if !neighbors.is_empty() {
                let mut cache = self.cache_weighted.write();
                cache.insert(key, neighbors.clone());
                return Ok(neighbors);
            }
        }

        Ok(Arc::from([]))
    }

    pub(crate) fn load_neighbors_weighted_from_disk(
        &self,
        src: i64,
        dir: Direction,
        db_path: &Path,
    ) -> NativeResult<Arc<[(i64, f32)]>> {
        let key = edge_key(src, dir);
        let btree = self.btree.read();

        let page_id = match btree.lookup(key) {
            Ok(Some(pid)) => pid,
            Ok(None) | Err(_) => return Ok(Arc::from([])),
        };
        drop(btree);

        match self.load_cluster_from_disk(page_id, src, dir, db_path) {
            Ok(cluster) => Ok(super::io_support::weighted_neighbors_from_cluster(
                self, src, dir, &cluster,
            )),
            Err(_) => Ok(Arc::from([])),
        }
    }

    pub fn warm_weighted_neighbors(&self, sources: &[i64], dir: Direction) -> NativeResult<usize> {
        let db_path = match &self.db_path {
            Some(path) => path,
            None => return Ok(0),
        };
        if sources.is_empty() {
            return Ok(0);
        }

        let misses: Vec<i64> = {
            let cache = self.cache_weighted.read();
            sources
                .iter()
                .copied()
                .filter(|src| !cache.contains_key(&(*src, dir)))
                .collect()
        };
        if misses.is_empty() {
            return Ok(0);
        }

        let lookups: Vec<(i64, u64)> = {
            let btree = self.btree.read();
            misses
                .into_iter()
                .filter_map(|src| match btree.lookup(edge_key(src, dir)) {
                    Ok(Some(page_id)) => Some((src, page_id)),
                    Ok(None) | Err(_) => None,
                })
                .collect()
        };
        if lookups.is_empty() {
            return Ok(0);
        }

        let mut page_groups: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
        for (src, page_id) in lookups {
            page_groups.entry(page_id).or_default().push(src);
        }

        let mut file = std::fs::File::open(db_path).map_err(|e| NativeBackendError::IoError {
            context: format!(
                "Failed to open db file for edge warm read: {}",
                db_path.display()
            ),
            source: e,
        })?;

        let mut warmed = Vec::new();
        for (page_id, srcs) in page_groups {
            let buffer = self.read_page_from_disk(&mut file, db_path, page_id)?;
            if buffer.len() >= PACKED_EDGE_PAGE_HEADER_SIZE
                && buffer[0..4] == PACKED_EDGE_PAGE_MAGIC
            {
                for src in srcs {
                    if let Some(cluster_bytes) = decode_packed_edge_page(&buffer, src, dir)? {
                        let mut cluster = V3EdgeCluster::deserialize(&cluster_bytes, page_id)?;
                        cluster.page_id = 0;
                        cluster.sort_for_weighted_queries();
                        let weighted = super::io_support::weighted_neighbors_from_cluster(
                            self, src, dir, &cluster,
                        );
                        let unweighted =
                            super::io_support::unweighted_neighbors_from_weighted(&weighted);
                        warmed.push((src, weighted, unweighted));
                    }
                }
                continue;
            }

            for src in srcs {
                let mut cluster = self.load_cluster_from_open_file(
                    &mut file,
                    page_id,
                    src,
                    dir,
                    db_path,
                    Some(buffer.clone()),
                )?;
                cluster.sort_for_weighted_queries();
                let weighted =
                    super::io_support::weighted_neighbors_from_cluster(self, src, dir, &cluster);
                let unweighted = super::io_support::unweighted_neighbors_from_weighted(&weighted);
                warmed.push((src, weighted, unweighted));
            }
        }

        if warmed.is_empty() {
            return Ok(0);
        }

        let warmed_count = warmed.len();
        let mut weighted_cache = self.cache_weighted.write();
        let mut unweighted_cache = self.cache.write();
        for (src, weighted, unweighted) in warmed {
            weighted_cache.insert((src, dir), weighted);
            unweighted_cache.insert((src, dir), unweighted);
        }
        Ok(warmed_count)
    }

    pub fn neighbors_weighted_filtered(
        &self,
        src: i64,
        dir: Direction,
        edge_type: &str,
    ) -> NativeResult<Arc<[(i64, f32)]>> {
        let all_neighbors = self.neighbors_weighted(src, dir)?;

        let edge_types = self.edge_types.read();
        let filtered: Vec<(i64, f32)> = all_neighbors
            .iter()
            .filter(|&&(dst, _)| {
                edge_types
                    .get(&(src, dst, dir))
                    .map(|stored_type| stored_type == edge_type)
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        Ok(Arc::from(filtered.into_boxed_slice()))
    }
}
