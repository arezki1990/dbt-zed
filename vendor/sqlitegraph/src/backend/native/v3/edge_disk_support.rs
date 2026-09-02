use super::*;

#[path = "edge_disk_support/flush_support.rs"]
mod flush_support;
#[path = "edge_disk_support/metadata_support.rs"]
mod metadata_support;

impl V3EdgeStore {
    pub fn flush(
        &self,
        _kv_store: Option<
            &parking_lot::RwLock<Option<crate::backend::native::v3::kv_store::store::KvStore>>,
        >,
    ) -> NativeResult<()> {
        flush_support::flush(self)
    }

    /// Get the path to the edge metadata file
    fn metadata_path(&self) -> Option<PathBuf> {
        self.db_path
            .as_ref()
            .map(|p| p.with_extension("v3edgemeta"))
    }

    pub(crate) fn load_cluster_from_disk(
        &self,
        page_id: u64,
        src: i64,
        dir: Direction,
        db_path: &Path,
    ) -> NativeResult<V3EdgeCluster> {
        use std::fs::File;

        let mut file = File::open(db_path).map_err(|e| NativeBackendError::IoError {
            context: format!(
                "Failed to open db file for edge cluster read: {}",
                db_path.display()
            ),
            source: e,
        })?;

        self.load_cluster_from_open_file(&mut file, page_id, src, dir, db_path, None)
    }

    pub(crate) fn load_cluster_from_open_file(
        &self,
        file: &mut std::fs::File,
        page_id: u64,
        src: i64,
        dir: Direction,
        db_path: &Path,
        first_page: Option<Vec<u8>>,
    ) -> NativeResult<V3EdgeCluster> {
        let mut current_page_id = page_id;
        let mut cluster_bytes = Vec::new();
        let mut buffer = match first_page {
            Some(buffer) => buffer,
            None => self.read_page_from_disk(file, db_path, current_page_id)?,
        };

        loop {
            if let Some(cluster_bytes) = decode_packed_edge_page(&buffer, src, dir)? {
                let mut cluster = V3EdgeCluster::deserialize(&cluster_bytes, page_id)?;
                cluster.page_id = 0;
                return Ok(cluster);
            }

            if let Some((payload_len, next_page_id)) = decode_edge_cluster_page_header(&buffer) {
                let payload_end = EDGE_CLUSTER_PAGE_HEADER_SIZE + payload_len;
                cluster_bytes
                    .extend_from_slice(&buffer[EDGE_CLUSTER_PAGE_HEADER_SIZE..payload_end]);
                if next_page_id == 0 {
                    break;
                }
                current_page_id = next_page_id;
                buffer = self.read_page_from_disk(file, db_path, current_page_id)?;
            } else {
                return V3EdgeCluster::deserialize(&buffer, page_id);
            }
        }

        V3EdgeCluster::deserialize(&cluster_bytes, page_id)
    }

    /// Persist B+Tree root metadata to disk for recovery
    ///
    /// This writes a small metadata file containing the B+Tree root_page_id
    /// and tree_height so that the edge index can be recovered after restart.
    fn persist_btree_metadata(&self) -> NativeResult<()> {
        metadata_support::persist_btree_metadata(self)
    }

    /// Recover B+Tree root metadata from disk
    ///
    /// Returns (root_page_id, tree_height) if metadata file exists and is valid.
    /// Returns None if metadata doesn't exist or is corrupted.
    fn recover_btree_metadata(&self) -> NativeResult<Option<(u64, u32)>> {
        metadata_support::recover_btree_metadata(self)
    }

    /// Restore B+Tree state from persisted metadata
    ///
    /// This should be called after creating a new V3EdgeStore to recover
    /// the B+Tree root from a previous session.
    pub fn restore_btree_from_metadata(&self) -> NativeResult<bool> {
        if let Some((root_page_id, tree_height)) = self.recover_btree_metadata()? {
            let mut btree = self.btree.write();
            btree.set_root_page_id(root_page_id);
            btree.set_tree_height(tree_height);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Write a page of data to disk
    ///
    /// BUG FIX: Previously opened a raw file handle bypassing FileCoordinator,
    /// which could cause data corruption from concurrent writes with NodeStore.
    /// Now routes through FileCoordinator when available.
    fn write_page_to_disk(&self, db_path: &Path, page_id: u64, data: &[u8]) -> NativeResult<()> {
        metadata_support::write_page_to_disk(self, db_path, page_id, data)
    }

    /// Flush WAL buffer to disk (for durability testing)
    #[cfg(test)]
    pub fn flush_wal(&self) -> NativeResult<()> {
        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            wal_guard.flush()
        } else {
            Ok(())
        }
    }

    /// Get the current B+Tree root page ID
    /// CRITICAL: Must be called during flush_to_disk to update header
    ///
    /// Returns None if the tree is empty (EMPTY_TREE_ROOT = u64::MAX)
    /// Returns Some(page_id) if the tree has a valid root
    pub fn btree_root_page_id(&self) -> Option<u64> {
        let root = self.btree.read().root_page_id();
        // Filter out EMPTY_TREE_ROOT (u64::MAX) and 0 (uninitialized)
        if root != 0 && root != u64::MAX {
            Some(root)
        } else {
            None
        }
    }

    /// Get the current B+Tree height
    /// CRITICAL: Must be called during flush_to_disk to update header
    pub fn btree_height(&self) -> u32 {
        self.btree.read().tree_height()
    }

    /// Set the WAL writer for this edge store
    ///
    /// This is called after opening an existing database when a WAL is discovered.
    pub fn set_wal(&mut self, wal: Arc<RwLock<WALWriter>>) {
        self.wal = Some(wal);
    }
}
