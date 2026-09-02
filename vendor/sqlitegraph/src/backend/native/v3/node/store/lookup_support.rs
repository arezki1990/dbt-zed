use super::*;

impl NodeStore {
    pub fn has_index(&self) -> bool {
        self.root_page_id != 0
    }

    pub fn root_page_id_pub(&self) -> u64 {
        self.root_page_id
    }

    pub fn tree_height_pub(&self) -> u32 {
        self.tree_height
    }

    /// Get the B+Tree root page ID from the BTreeManager
    pub fn btree_root_page_id(&self) -> Option<u64> {
        self.btree_manager.as_ref().and_then(|btree| {
            let root = btree.root_page_id();
            if root != 0 && root != u64::MAX {
                Some(root)
            } else {
                None
            }
        })
    }

    /// Get the B+Tree height from the BTreeManager
    pub fn btree_height(&self) -> Option<u32> {
        self.btree_manager.as_ref().and_then(|btree| {
            let height = btree.tree_height();
            if height > 0 { Some(height) } else { None }
        })
    }

    pub fn lookup_page(&mut self, node_id: i64) -> NativeResult<Option<u64>> {
        if let Some(ref btree) = self.btree_manager {
            return btree.lookup(node_id);
        }

        if self.root_page_id == 0 {
            return Ok(None);
        }

        let search_key = node_id as u64;
        let mut current_page_id = self.root_page_id;
        let mut depth = 0;

        while depth < self.tree_height as usize + MAX_TREE_HEIGHT as usize {
            let index_page = if let Some(cached) = self.index_cache.get(&current_page_id) {
                cached.clone()
            } else {
                let page_bytes = self.load_page_from_disk(current_page_id)?;
                let index = IndexPage::unpack(&page_bytes)?;
                self.index_cache.put(current_page_id, index.clone());
                index
            };

            match index_page {
                IndexPage::Leaf {
                    entries, next_leaf, ..
                } => {
                    let result = IndexPage::binary_search_leaf(&entries, search_key);
                    return match result {
                        Ok(idx) => {
                            if let Some((_, page_id)) = entries.get(idx) {
                                Ok(Some(*page_id))
                            } else {
                                Err(NativeBackendError::InvalidHeader {
                                    field: "btree_leaf".to_string(),
                                    reason: "entry index out of bounds".to_string(),
                                })
                            }
                        }
                        Err(_idx) => {
                            if next_leaf == 0 {
                                Ok(None)
                            } else {
                                current_page_id = next_leaf;
                                continue;
                            }
                        }
                    };
                }
                IndexPage::Internal { keys, children, .. } => {
                    let child_idx = IndexPage::find_child_index(&keys, search_key);
                    if child_idx < children.len() {
                        current_page_id = children[child_idx];
                    } else {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "btree_internal".to_string(),
                            reason: format!("child index {} out of bounds", child_idx),
                        });
                    }
                }
            }

            depth += 1;
        }

        Err(NativeBackendError::InvalidHeader {
            field: "btree_depth".to_string(),
            reason: format!("exceeded maximum depth {}", MAX_TREE_HEIGHT),
        })
    }

    pub fn lookup_node(&mut self, node_id: i64) -> NativeResult<Option<NodeRecordV3>> {
        let page_id = match self.lookup_page(node_id)? {
            Some(pid) => pid,
            None => return Ok(None),
        };

        let node_page = self.load_node_page(page_id)?;
        for node in &node_page.nodes {
            if node.id() == node_id {
                return Ok(Some(node.clone()));
            }
        }

        Ok(None)
    }

    pub(super) fn load_page_from_disk(&mut self, page_id: u64) -> NativeResult<Vec<u8>> {
        if let Some(cached) = self.page_cache_get(page_id) {
            #[cfg(feature = "v3-forensics")]
            FORENSIC_COUNTERS
                .node_cache_hit_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(cached);
        }

        #[cfg(feature = "v3-forensics")]
        {
            FORENSIC_COUNTERS
                .page_read_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            FORENSIC_COUNTERS
                .node_cache_miss_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        let mut buffer = vec![0u8; DEFAULT_PAGE_SIZE as usize];

        if let Some(coordinator) = &self.file_coordinator {
            coordinator.read_page(page_id, &mut buffer)?;
        } else {
            let page_offset = Self::page_offset(page_id);
            let mut file = File::open(&self.db_path).map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to open database file: {}", self.db_path.display()),
                source: e,
            })?;

            file.seek(SeekFrom::Start(page_offset))
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to seek to page {}", page_id),
                    source: e,
                })?;

            file.read_exact(&mut buffer)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to read page {}", page_id),
                    source: e,
                })?;
        }

        self.page_cache_insert(page_id, buffer.clone());

        Ok(buffer)
    }

    pub(super) fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            return 0;
        }
        let data_page_index = page_id.saturating_sub(1);
        V3_HEADER_SIZE + data_page_index * DEFAULT_PAGE_SIZE
    }

    #[inline]
    pub(super) fn extract_block_id_from_page_bytes(page_bytes: &[u8]) -> Option<i64> {
        use super::super::page::BLOCK_SIZE;

        if page_bytes.len() < 28 {
            return None;
        }

        let base_id = i64::from_be_bytes(page_bytes[20..28].try_into().ok()?);
        let block_id = if base_id < 1 {
            0
        } else {
            (base_id - 1) / BLOCK_SIZE
        };

        Some(block_id)
    }

    pub fn clear_cache(&mut self) {
        self.page_cache.write().clear();
        self.index_cache.clear();
    }

    pub fn cache_stats(&self) -> (usize, usize) {
        (self.page_cache.read().len(), self.index_cache.len())
    }

    pub(super) fn page_cache_get(&self, page_id: u64) -> Option<Vec<u8>> {
        let mut cache = self.page_cache.write();
        cache.get(&page_id).cloned()
    }

    pub(super) fn page_cache_insert(&self, page_id: u64, data: Vec<u8>) {
        if let Some(block_id) = Self::extract_block_id_from_page_bytes(&data) {
            self.current_access_block
                .store(block_id, std::sync::atomic::Ordering::Relaxed);
        }

        self.unpacked_page_cache_invalidate(page_id);

        let mut cache = self.page_cache.write();
        cache.put(page_id, data);
    }

    pub(super) fn unpacked_page_cache_get(&self, page_id: u64) -> Option<Arc<NodePage>> {
        let mut cache = self.unpacked_page_cache.write();
        cache.get(&page_id).cloned()
    }

    pub(super) fn unpacked_page_cache_insert(&self, page_id: u64, page: Arc<NodePage>) {
        let mut cache = self.unpacked_page_cache.write();
        cache.put(page_id, page);
    }

    pub(super) fn unpacked_page_cache_invalidate(&self, page_id: u64) {
        let mut cache = self.unpacked_page_cache.write();
        cache.pop(&page_id);
    }

    pub fn update_root(&mut self, new_root: u64) {
        self.root_page_id = new_root;
        self.index_cache.clear();
    }

    pub fn update_tree_height(&mut self, new_height: u32) {
        self.tree_height = new_height;
    }

    pub fn is_valid_node_id(&self, node_id: i64) -> bool {
        if node_id <= 0 {
            return false;
        }
        if !self.has_index() {
            return false;
        }
        true
    }

    pub(super) fn lookup_page_ro(&self, node_id: i64) -> NativeResult<Option<u64>> {
        if let Some(ref btree) = self.btree_manager {
            return btree.lookup(node_id);
        }
        Ok(None)
    }
}
