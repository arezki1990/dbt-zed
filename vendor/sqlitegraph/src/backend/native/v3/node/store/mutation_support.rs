use super::*;

impl NodeStore {
    /// Get the next available node ID and increment
    pub fn allocate_node_id(&mut self) -> i64 {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    /// Set the next node ID (for recovery/initialization)
    pub fn set_next_node_id(&mut self, next_id: i64) {
        self.next_node_id = next_id;
    }

    /// Get all node IDs in the store from the B+Tree
    pub fn node_ids(&self) -> NativeResult<Vec<i64>> {
        if let Some(ref btree) = self.btree_manager {
            btree.keys()
        } else {
            Ok(Vec::new())
        }
    }

    /// Insert a new node into the store
    pub fn insert_node(
        &mut self,
        mut node: NodeRecordV3,
        manual_id: Option<i64>,
    ) -> NativeResult<i64> {
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .node_encode_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let node_id = if let Some(id) = manual_id {
            if id >= self.next_node_id {
                self.next_node_id = id + 1;
            }
            id
        } else {
            self.allocate_node_id()
        };
        node.id = node_id;

        let mut attempts = 0;
        const MAX_ATTEMPTS: usize = 3;

        loop {
            attempts += 1;

            let page_id = if attempts == 1 {
                self.find_or_create_page_for_node(&node)?
            } else {
                let mut allocator = self.page_allocator_mut()?;
                let new_page_id = allocator.allocate()?;
                let new_page = NodePage::new(new_page_id);

                let page_bytes = new_page.pack()?;
                if let Some(coordinator) = &self.file_coordinator {
                    coordinator.write_page(new_page_id, &page_bytes)?;
                } else {
                    let offset = Self::page_offset(new_page_id);
                    let file_exists = self.db_path.exists();
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create(!file_exists)
                        .open(&self.db_path)
                        .map_err(|e| NativeBackendError::IoError {
                            context: format!(
                                "Failed to open db file for retry page write: {}",
                                self.db_path.display()
                            ),
                            source: e,
                        })?;

                    let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
                    let required_len = offset + page_bytes.len() as u64;
                    if required_len > current_len {
                        file.set_len(required_len)
                            .map_err(|e| NativeBackendError::IoError {
                                context: format!(
                                    "Failed to extend file to {} bytes for retry page {}",
                                    required_len, new_page_id
                                ),
                                source: e,
                            })?;
                    }

                    file.seek(SeekFrom::Start(offset)).map_err(|e| {
                        NativeBackendError::IoError {
                            context: format!(
                                "Failed to seek to offset {} for retry page {}",
                                offset, new_page_id
                            ),
                            source: e,
                        }
                    })?;
                    file.write_all(&page_bytes)
                        .map_err(|e| NativeBackendError::IoError {
                            context: format!("Failed to write retry page {} to disk", new_page_id),
                            source: e,
                        })?;
                    file.sync_all().map_err(|e| NativeBackendError::IoError {
                        context: format!("Failed to sync retry page {}", new_page_id),
                        source: e,
                    })?;
                }

                new_page_id
            };

            let mut page = self.load_node_page(page_id)?;

            match page.add_node(node.clone()) {
                Ok(()) => {
                    self.write_node_page(&page)?;

                    let btree = self.btree_manager_mut()?;
                    btree.insert(node_id, page_id)?;

                    let new_root = btree.root_page_id();
                    let new_height = btree.tree_height();
                    self.root_page_id = new_root;
                    self.tree_height = new_height;

                    if !self.batch_mode {
                        if let Some(ref mut wal) = self.wal_writer {
                            let page_bytes = page.pack()?;
                            wal.page_write(page_id, 0, page_bytes.to_vec())?;
                        }

                        self.page_cache_insert(page_id, page.pack()?.to_vec());
                    }

                    return Ok(node_id);
                }
                Err(NativeBackendError::InvalidHeader { ref field, .. })
                    if field == "node_page" && attempts < MAX_ATTEMPTS =>
                {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    /// Update an existing node
    pub fn update_node(&mut self, node_id: i64, node: NodeRecordV3) -> NativeResult<()> {
        let page_id = match self.lookup_page(node_id)? {
            Some(pid) => pid,
            None => {
                return Err(NativeBackendError::InvalidHeader {
                    field: "update_node".to_string(),
                    reason: format!("Node {} not found", node_id),
                });
            }
        };

        let mut page = self.load_node_page(page_id)?;

        let mut found = false;
        for (i, existing_node) in page.nodes.iter().enumerate() {
            if existing_node.id() == node_id {
                page.nodes[i] = node;
                found = true;
                break;
            }
        }

        if !found {
            return Err(NativeBackendError::InvalidHeader {
                field: "update_node".to_string(),
                reason: format!("Node {} not found in page {}", node_id, page_id),
            });
        }

        page.used_bytes = page
            .nodes
            .iter()
            .map(|n| self.estimate_node_size(n))
            .sum::<NativeResult<u16>>()?;

        self.write_node_page(&page)?;

        if let Some(ref mut wal) = self.wal_writer {
            let page_bytes = page.pack()?;
            wal.page_write(page_id, 0, page_bytes.to_vec())?;
        }

        self.page_cache_insert(page_id, page.pack()?.to_vec());

        Ok(())
    }

    /// Delete a node (soft delete with tombstone)
    pub fn delete_node(&mut self, node_id: i64) -> NativeResult<bool> {
        let page_id = match self.lookup_page(node_id)? {
            Some(pid) => pid,
            None => return Ok(false),
        };

        let mut page = self.load_node_page(page_id)?;

        let mut found = false;
        for node in page.nodes.iter_mut() {
            if node.id() == node_id {
                node.flags = NodeFlags::DELETED;
                found = true;
                break;
            }
        }

        if !found {
            return Ok(false);
        }

        self.write_node_page(&page)?;

        let btree = self.btree_manager_mut()?;
        btree.delete(node_id)?;

        if let Some(ref mut wal) = self.wal_writer {
            let page_bytes = page.pack()?;
            wal.page_write(page_id, 0, page_bytes.to_vec())?;
        }

        self.page_cache_insert(page_id, page.pack()?.to_vec());

        Ok(true)
    }

    /// Find or create a page for a new node
    pub(super) fn find_or_create_page_for_node(
        &mut self,
        node: &NodeRecordV3,
    ) -> NativeResult<u64> {
        let node_size = self.estimate_node_size(node)?;

        use super::super::page::node_id_to_block;
        let block_id = node_id_to_block(node.id);

        if let Some(preferred_pages) = self.block_preferred_pages.get(&block_id) {
            for &page_id in preferred_pages.iter().rev() {
                if let Some(page) = self.dirty_pages.get(&page_id)
                    && page.capacity() >= node_size
                {
                    return Ok(page_id);
                }

                if let Some(page_bytes) = self.page_cache_get(page_id)
                    && let Ok(page) = NodePage::unpack(&page_bytes)
                    && page.capacity() >= node_size
                {
                    return Ok(page_id);
                }
            }
        }

        for (&page_id, page) in &self.dirty_pages {
            let cap = page.capacity();
            if cap >= node_size {
                return Ok(page_id);
            }
        }

        {
            let cache = self.page_cache.read();
            for (&page_id, page_bytes) in cache.iter() {
                if self.dirty_pages.contains_key(&page_id) {
                    continue;
                }
                if let Ok(page) = NodePage::unpack(page_bytes) {
                    let cap = page.capacity();
                    if cap >= node_size {
                        return Ok(page_id);
                    }
                }
            }
        }

        let new_page_id = {
            let mut allocator = self.page_allocator_mut()?;
            allocator.allocate()?
        };

        self.associate_page_with_block(new_page_id, block_id);

        let new_page = NodePage::new(new_page_id);
        let page_bytes = new_page.pack()?;

        if let Some(coordinator) = &self.file_coordinator {
            coordinator.write_page(new_page_id, &page_bytes)?;
        } else {
            let offset = Self::page_offset(new_page_id);
            let _required_len = offset + page_bytes.len() as u64;

            let file_exists = self.db_path.exists();
            let mut file = OpenOptions::new()
                .write(true)
                .create(!file_exists)
                .open(&self.db_path)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!(
                        "Failed to open db file for new page write: {}",
                        self.db_path.display()
                    ),
                    source: e,
                })?;

            file.seek(SeekFrom::Start(offset))
                .map_err(|e| NativeBackendError::IoError {
                    context: format!(
                        "Failed to seek to offset {} for new page {}",
                        offset, new_page_id
                    ),
                    source: e,
                })?;

            file.write_all(&page_bytes)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to write new page {} to disk", new_page_id),
                    source: e,
                })?;

            file.sync_all().map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to sync new page {}", new_page_id),
                source: e,
            })?;
        }

        self.page_cache_insert(new_page_id, page_bytes.to_vec());

        Ok(new_page_id)
    }

    /// Associate a page with a block for physical placement bias
    pub(super) fn associate_page_with_block(&mut self, page_id: u64, block_id: i64) {
        let pages = self.block_preferred_pages.entry(block_id).or_default();

        if !pages.contains(&page_id) {
            pages.push(page_id);

            while pages.len() > self.max_preferred_pages_per_block {
                pages.remove(0);
            }
        }
    }

    /// Load a NodePage from disk
    pub(super) fn load_node_page(&mut self, page_id: u64) -> NativeResult<NodePage> {
        if let Some(page) = self.dirty_pages.get(&page_id) {
            #[cfg(feature = "v3-forensics")]
            FORENSIC_COUNTERS
                .dirty_page_hit_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(page.clone());
        }

        if let Some(cached_page) = self.unpacked_page_cache_get(page_id) {
            #[cfg(feature = "v3-forensics")]
            FORENSIC_COUNTERS
                .node_page_cache_hit_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached_page).clone());
        }

        if let Some(cached) = self.page_cache_get(page_id) {
            #[cfg(feature = "v3-forensics")]
            FORENSIC_COUNTERS
                .node_page_cache_hit_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let page = NodePage::unpack(&cached)?;
            self.unpacked_page_cache_insert(page_id, Arc::new(page.clone()));
            return Ok(page);
        }

        let page_bytes = self.load_page_from_disk(page_id)?;
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .node_page_cache_miss_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let page = NodePage::unpack(&page_bytes)?;
        self.page_cache_insert(page_id, page_bytes);
        self.unpacked_page_cache_insert(page_id, Arc::new(page.clone()));
        Ok(page)
    }

    /// Write a NodePage to disk
    pub(super) fn write_node_page(&mut self, page: &NodePage) -> NativeResult<()> {
        let page_id = page.page_id;

        #[cfg(feature = "v3-forensics")]
        {
            FORENSIC_COUNTERS
                .page_write_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let offset = if self.file_coordinator.is_some() {
                V3_HEADER_SIZE + (page_id.saturating_sub(1)) * DEFAULT_PAGE_SIZE
            } else {
                Self::page_offset(page_id)
            };

            crate::track_page_alloc!(page_id, Subsystem::NodeStore, PageType::Node);
            crate::track_page_write!(
                page_id,
                Subsystem::NodeStore,
                PageType::Node,
                offset,
                "NodeStore::write_node_page"
            );
        }

        if self.batch_mode {
            self.dirty_pages.insert(page_id, page.clone());
            return Ok(());
        }

        let page_bytes = page.pack()?;

        if let Some(coordinator) = &self.file_coordinator {
            coordinator.write_page(page_id, &page_bytes)?;
            return Ok(());
        }

        let offset = Self::page_offset(page_id);
        let required_len = offset + page_bytes.len() as u64;

        let file_exists = self.db_path.exists();
        let mut file = OpenOptions::new()
            .write(true)
            .create(!file_exists)
            .open(&self.db_path)
            .map_err(|e| NativeBackendError::IoError {
                context: format!(
                    "Failed to open database file for writing: {}",
                    self.db_path.display()
                ),
                source: e,
            })?;

        let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if required_len > current_len {
            file.set_len(required_len)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!(
                        "Failed to extend file to {} bytes for page {}",
                        required_len, page_id
                    ),
                    source: e,
                })?;
            file.sync_all().map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to sync after extending file for page {}", page_id),
                source: e,
            })?;
        }

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to seek to page {} offset {}", page_id, offset),
                source: e,
            })?;

        file.write_all(&page_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to write page {}", page_id),
                source: e,
            })?;

        file.sync_all().map_err(|e| NativeBackendError::IoError {
            context: format!("Failed to sync page {}", page_id),
            source: e,
        })?;

        Ok(())
    }

    /// Estimate the compressed size of a node record
    pub(super) fn estimate_node_size(&self, node: &NodeRecordV3) -> NativeResult<u16> {
        use crate::backend::native::v3::compression::delta::encode_id_delta;
        use crate::backend::native::v3::compression::varint::varint_size;

        let mut size: usize = 0;
        let base_id = 0;

        let delta = encode_id_delta(node.id(), base_id);
        size += varint_size(delta as u64);
        size += 4;
        size += varint_size(node.kind_offset as u64);
        size += varint_size(node.name_offset as u64);
        size += varint_size(node.data_len() as u64);
        size += varint_size(node.outgoing_cluster_offset);
        size += varint_size(node.outgoing_edge_count as u64);
        size += varint_size(node.incoming_cluster_offset);
        size += varint_size(node.incoming_edge_count as u64);

        if let Some(ref data) = node.data_inline {
            size += data.len();
        } else if node.data_external_offset.is_some() {
            size += 8;
        }

        if size > u16::MAX as usize {
            return Err(NativeBackendError::InvalidHeader {
                field: "compressed_size".to_string(),
                reason: format!("compressed size {} exceeds u16::MAX", size),
            });
        }

        Ok(size as u16)
    }
}
