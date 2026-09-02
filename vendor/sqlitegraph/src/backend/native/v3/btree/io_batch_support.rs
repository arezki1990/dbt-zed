use super::*;

impl BTreeManager {
    /// Asynchronously load an index page.
    pub(super) async fn load_page_async(
        &self,
        page_id: u64,
        async_coordinator: &AsyncFileCoordinator,
    ) -> Result<IndexPage, SqliteGraphError> {
        if let Some(page) = self.page_cache.get(page_id) {
            return Ok(page);
        }

        if page_id == 0 {
            return Err(SqliteGraphError::connection(
                "Cannot load page 0 (reserved for header)",
            ));
        }

        let initial_buf = vec![0u8; self.page_size as usize];
        let (buffer, _) = async_coordinator.read_page(page_id, initial_buf).await?;

        let page = IndexPage::unpack(&buffer).map_err(|e| {
            SqliteGraphError::validation(format!("Failed to unpack index page: {}", e))
        })?;

        self.page_cache.insert(page_id, page.clone());
        Ok(page)
    }

    /// Load page from cache or disk
    pub(super) fn load_page(&self, page_id: u64) -> NativeResult<IndexPage> {
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .page_read_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if let Some(page) = self.page_cache.get(page_id) {
            #[cfg(feature = "v3-forensics")]
            FORENSIC_COUNTERS
                .btree_cache_hit_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(page);
        }

        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .btree_cache_miss_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let db_path = match &self.db_path {
            Some(path) => path,
            None => {
                return Err(NativeBackendError::InvalidHeader {
                    field: "page_cache".to_string(),
                    reason: format!("page {} not in cache (no disk path configured)", page_id),
                });
            }
        };

        if page_id == 0 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_id".to_string(),
                reason: "Cannot load page 0 (reserved for header)".to_string(),
            });
        }

        let mut buffer = vec![0u8; self.page_size as usize];
        if let Some(coordinator) = &self.file_coordinator {
            coordinator.read_page(page_id, &mut buffer)?;
        } else {
            let offset = V3_HEADER_SIZE + (page_id - 1) * self.page_size;
            let mut file = File::open(db_path).map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to open db file for page load: {}", page_id),
                source: e,
            })?;

            file.seek(SeekFrom::Start(offset))
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to seek to page {} at offset {}", page_id, offset),
                    source: e,
                })?;

            file.read_exact(&mut buffer)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to read page {} from disk", page_id),
                    source: e,
                })?;
        }

        let page = IndexPage::unpack(&buffer)?;
        self.page_cache.insert(page_id, page.clone());
        Ok(page)
    }

    /// Write page to cache and disk (if db_path configured)
    pub(super) fn write_page(&mut self, page: &IndexPage) -> NativeResult<()> {
        let page_id = page.page_id();

        #[cfg(feature = "v3-forensics")]
        {
            FORENSIC_COUNTERS
                .page_write_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let offset = V3_HEADER_SIZE + (page_id.saturating_sub(1)) * self.page_size;
            let page_type = match page {
                IndexPage::Leaf { .. } => PageType::BTree,
                IndexPage::Internal { .. } => PageType::BTree,
            };

            crate::track_page_alloc!(page_id, Subsystem::BTreeManager, page_type);
            crate::track_page_write!(
                page_id,
                Subsystem::BTreeManager,
                page_type,
                offset,
                "BTreeManager::write_page"
            );
        }

        let page_bytes = page.pack()?;

        if self.db_path.is_some() {
            if page_id == 0 {
                return Err(NativeBackendError::InvalidHeader {
                    field: "page_id".to_string(),
                    reason: "Cannot write page 0 (reserved for header)".to_string(),
                });
            }

            if let Some(coordinator) = &self.file_coordinator {
                coordinator.write_page(page_id, &page_bytes)?;
            } else {
                let offset = V3_HEADER_SIZE + (page_id - 1) * self.page_size;
                let required_len = offset + page_bytes.len() as u64;

                let db_path =
                    self.db_path
                        .as_ref()
                        .ok_or_else(|| NativeBackendError::InvalidHeader {
                            field: "db_path".to_string(),
                            reason: "Cannot write page to disk without db_path".to_string(),
                        })?;
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(db_path)
                    .map_err(|e| NativeBackendError::IoError {
                        context: format!("Failed to open db file for page write: {}", page_id),
                        source: e,
                    })?;

                let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
                if required_len > current_len {
                    file.set_len(required_len)
                        .map_err(|e| NativeBackendError::IoError {
                            context: format!(
                                "Failed to extend file to {} bytes for B+Tree page {}",
                                required_len, page_id
                            ),
                            source: e,
                        })?;
                    file.sync_all().map_err(|e| NativeBackendError::IoError {
                        context: format!(
                            "Failed to sync file size to {} bytes for B+Tree page {}",
                            required_len, page_id
                        ),
                        source: e,
                    })?;
                }

                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| NativeBackendError::IoError {
                        context: format!("Failed to seek to page {} at offset {}", page_id, offset),
                        source: e,
                    })?;

                file.write_all(&page_bytes)
                    .map_err(|e| NativeBackendError::IoError {
                        context: format!("Failed to write page {} to disk", page_id),
                        source: e,
                    })?;

                file.sync_all().map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to sync B+Tree page {}", page_id),
                    source: e,
                })?;
            }
        }

        self.page_cache.insert(page_id, page.clone());
        Ok(())
    }

    /// Get reference to allocator (read lock)
    pub fn allocator(&self) -> parking_lot::RwLockReadGuard<'_, PageAllocator> {
        self.allocator.read()
    }

    /// Get mutable reference to allocator (write lock)
    pub fn allocator_mut(&self) -> parking_lot::RwLockWriteGuard<'_, PageAllocator> {
        self.allocator.write()
    }

    /// Clear the page cache
    pub fn clear_cache(&mut self) {
        self.page_cache.clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        self.page_cache.stats()
    }

    /// Get a reference to the shared page cache
    pub fn page_cache(&self) -> BTreePageCache {
        self.page_cache.clone()
    }

    /// Create a new write batch for batched page writes
    pub fn create_write_batch(&self) -> WriteBatch {
        WriteBatch::new()
    }

    /// Stage a page write to a batch instead of writing immediately
    pub fn stage_page_to_batch(&self, batch: &mut WriteBatch, page: IndexPage) -> NativeResult<()> {
        batch.stage_page(page)
    }

    /// Commit a write batch to disk with single fsync
    pub fn commit_batch(&mut self, batch: WriteBatch) -> NativeResult<()> {
        let db_path =
            self.db_path
                .as_ref()
                .ok_or_else(|| NativeBackendError::InvalidOperation {
                    context: "Cannot commit batch: no db_path configured".to_string(),
                })?;

        batch.commit(db_path)
    }

    /// Perform a simple insert that doesn't require splitting, staged to batch
    pub fn insert_simple_to_batch(
        &mut self,
        batch: &mut WriteBatch,
        key: i64,
        value: u64,
    ) -> NativeResult<bool> {
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            let page_id = self.allocator.write().allocate()?;
            let mut leaf = IndexPage::new_leaf(page_id);

            if let IndexPage::Leaf { entries, .. } = &mut leaf {
                entries.push((key as u64, value));
            }

            batch.stage_page(leaf.clone())?;
            self.page_cache.insert(page_id, leaf);
            self.root_page_id = page_id;
            self.tree_height = 1;

            return Ok(true);
        }

        Err(NativeBackendError::InvalidOperation {
            context: "Batch insert to non-empty tree not yet implemented".to_string(),
        })
    }
}
