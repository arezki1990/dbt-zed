//! BTreeManager for V3 native backend
//!
//! Manages B+Tree operations for node_id -> page_id index mapping.
//! Provides O(log n) lookup, insert, and delete operations.
//!
//! ## Architecture
//!
//! 1. **B+Tree Structure**: Internal nodes with keys/children, leaf nodes with entries
//! 2. **Page Management**: Uses PageAllocator for page lifecycle
//! 3. **Durability**: Optional WAL integration for crash recovery
//! 4. **Splitting**: Automatic page splitting when capacity exceeded
//!
//! ## Operations
//!
//! - `lookup`: Traverse B+Tree to find page_id for a node_id
//! - `insert`: Add new key->value mapping, split pages as needed
//! - `delete`: Remove key->value mapping, handle underflow (future)
//! - `split_page`: Split full page into two, propagate up if needed

use crate::SqliteGraphError;
use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::AsyncFileCoordinator;
use crate::backend::native::v3::allocator::PageAllocator;
use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
use crate::backend::native::v3::file_coordinator::FileCoordinator;
#[cfg(feature = "v3-forensics")]
use crate::backend::native::v3::forensics::{FORENSIC_COUNTERS, PageType, Subsystem};
use crate::backend::native::v3::index::IndexPage;
use crate::backend::native::v3::wal::WALWriter;
use crate::backend::native::v3::write_batch::WriteBatch;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

mod io_batch_support;
#[cfg(test)]
mod tests;

/// Maximum B+Tree height for safety
const MAX_TREE_HEIGHT: u32 = 10;

/// Marker for empty tree (no root page)
const EMPTY_TREE_ROOT: u64 = u64::MAX;

/// BTreeManager for B+Tree index operations
///
/// Manages the B+Tree structure for mapping node_id -> page_id.
/// Uses PageAllocator for page management and optional WAL for durability.
use parking_lot::RwLock;
use std::sync::Arc;

/// Shared page cache for B+Tree index pages
///
/// This type allows the cache to be shared across multiple BTreeManager instances,
/// enabling cache persistence across backend reopen cycles.
#[derive(Clone)]
pub struct BTreePageCache {
    inner: Arc<RwLock<lru::LruCache<u64, IndexPage>>>,
    capacity: usize,
}

impl BTreePageCache {
    /// Create a new shared page cache with the given capacity
    pub fn new(capacity: usize) -> Self {
        let cap = std::num::NonZeroUsize::new(capacity.max(1)).expect("capacity must be >= 1");
        Self {
            inner: Arc::new(RwLock::new(lru::LruCache::new(cap))),
            capacity,
        }
    }

    /// Create a new shared page cache with default capacity (1024 pages = 4 MiB)
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Get a page from the cache (returns clone if found, updates recency)
    pub fn get(&self, page_id: u64) -> Option<IndexPage> {
        self.inner.write().get(&page_id).cloned()
    }

    /// Insert a page into the cache. LRU eviction handles capacity automatically.
    pub fn insert(&self, page_id: u64, page: IndexPage) {
        self.inner.write().put(page_id, page);
    }

    /// Clear all entries from the cache
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// Get cache statistics (current size, capacity)
    pub fn stats(&self) -> (usize, usize) {
        (self.inner.read().len(), self.capacity)
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Get the number of entries in the cache
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }
}

#[derive(Clone)]
pub struct BTreeManager {
    /// Page allocator for page lifecycle management (shared with NodeStore)
    allocator: Arc<RwLock<PageAllocator>>,
    /// Optional WAL writer for durability (Arc<RwLock> for Clone + mutable access)
    wal: Option<Arc<RwLock<WALWriter>>>,
    /// Root page ID of the B+Tree (EMPTY_TREE_ROOT if tree is empty)
    root_page_id: u64,
    /// Current tree height (0 for empty tree)
    tree_height: u32,
    /// Shared page cache for index pages (can be shared across instances)
    page_cache: BTreePageCache,
    /// Database file path for disk I/O (None for in-memory/test mode)
    db_path: Option<PathBuf>,
    /// Page size for disk operations
    page_size: u64,
    /// Coordinated file handle for all main DB I/O (optional for backward compatibility)
    /// When set, all file writes go through this coordinator to prevent race conditions
    file_coordinator: Option<Arc<FileCoordinator>>,
}

impl BTreeManager {
    /// Create a new BTreeManager
    ///
    /// # Arguments
    ///
    /// * `allocator` - Arc<RwLock<PageAllocator>> for shared page management
    /// * `wal` - Optional WALWriter for durability
    /// * `db_path` - Optional path to database file for disk I/O (None for in-memory/test mode)
    ///
    /// # Returns
    ///
    /// New BTreeManager instance with empty tree and a new page cache
    pub fn new<P: Into<Option<PathBuf>>>(
        allocator: Arc<RwLock<PageAllocator>>,
        wal: Option<WALWriter>,
        db_path: P,
    ) -> Self {
        Self {
            allocator,
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            root_page_id: EMPTY_TREE_ROOT,
            tree_height: 0,
            page_cache: BTreePageCache::with_default_capacity(),
            db_path: db_path.into(),
            page_size: DEFAULT_PAGE_SIZE,
            file_coordinator: None,
        }
    }

    /// Create a new BTreeManager with a shared page cache
    ///
    /// # Arguments
    ///
    /// * `allocator` - Arc<RwLock<PageAllocator>> for shared page management
    /// * `wal` - Optional WALWriter for durability
    /// * `db_path` - Optional path to database file for disk I/O (None for in-memory/test mode)
    /// * `page_cache` - Shared page cache (allows cache persistence across backend reopen cycles)
    ///
    /// # Returns
    ///
    /// New BTreeManager instance with empty tree, using the provided shared cache
    pub fn with_cache<P: Into<Option<PathBuf>>>(
        allocator: Arc<RwLock<PageAllocator>>,
        wal: Option<WALWriter>,
        db_path: P,
        page_cache: BTreePageCache,
    ) -> Self {
        Self {
            allocator,
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            root_page_id: EMPTY_TREE_ROOT,
            tree_height: 0,
            page_cache,
            db_path: db_path.into(),
            page_size: DEFAULT_PAGE_SIZE,
            file_coordinator: None,
        }
    }

    /// Create a BTreeManager with an existing root page
    ///
    /// # Arguments
    ///
    /// * `allocator` - Arc<RwLock<PageAllocator>> for shared page management
    /// * `wal` - Optional WALWriter for durability
    /// * `root_page_id` - Existing root page ID
    /// * `tree_height` - Current tree height
    /// * `db_path` - Optional path to database file for disk I/O (None for in-memory/test mode)
    ///
    /// # Returns
    ///
    /// BTreeManager instance with existing tree state and a new page cache
    pub fn with_root<P: Into<Option<PathBuf>>>(
        allocator: Arc<RwLock<PageAllocator>>,
        wal: Option<WALWriter>,
        root_page_id: u64,
        tree_height: u32,
        db_path: P,
    ) -> Self {
        Self {
            allocator,
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            root_page_id,
            tree_height,
            page_cache: BTreePageCache::with_default_capacity(),
            db_path: db_path.into(),
            page_size: DEFAULT_PAGE_SIZE,
            file_coordinator: None,
        }
    }

    /// Create a BTreeManager with an existing root page and shared page cache
    ///
    /// # Arguments
    ///
    /// * `allocator` - Arc<RwLock<PageAllocator>> for shared page management
    /// * `wal` - Optional WALWriter for durability
    /// * `root_page_id` - Existing root page ID
    /// * `tree_height` - Current tree height
    /// * `db_path` - Optional path to database file for disk I/O (None for in-memory/test mode)
    /// * `page_cache` - Shared page cache (allows cache persistence across backend reopen cycles)
    ///
    /// # Returns
    ///
    /// BTreeManager instance with existing tree state, using the provided shared cache
    pub fn with_root_and_cache<P: Into<Option<PathBuf>>>(
        allocator: Arc<RwLock<PageAllocator>>,
        wal: Option<WALWriter>,
        root_page_id: u64,
        tree_height: u32,
        db_path: P,
        page_cache: BTreePageCache,
    ) -> Self {
        Self {
            allocator,
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            root_page_id,
            tree_height,
            page_cache,
            db_path: db_path.into(),
            page_size: DEFAULT_PAGE_SIZE,
            file_coordinator: None,
        }
    }

    /// Set the file coordinator for coordinated I/O
    ///
    /// When set, all file writes will go through this coordinator to prevent
    /// race conditions when multiple components write to the same file.
    pub fn set_file_coordinator(&mut self, coordinator: Arc<FileCoordinator>) {
        self.file_coordinator = Some(coordinator);
    }

    /// Get the root page ID
    pub fn root_page_id(&self) -> u64 {
        self.root_page_id
    }

    /// Get the current tree height
    pub fn tree_height(&self) -> u32 {
        self.tree_height
    }

    /// Set the root page ID (for recovery from metadata)
    pub fn set_root_page_id(&mut self, page_id: u64) {
        self.root_page_id = page_id;
    }

    /// Set the tree height (for recovery from metadata)
    pub fn set_tree_height(&mut self, height: u32) {
        self.tree_height = height;
    }

    /// Check if tree is empty (no root page)
    pub fn is_empty(&self) -> bool {
        self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0
    }

    /// Lookup page containing key (node_id -> page_id)
    ///
    /// Traverses the B+Tree from root to leaf to find the page_id
    /// associated with the given node_id.
    ///
    /// # Arguments
    ///
    /// * `key` - Node ID to look up
    ///
    /// # Returns
    ///
    /// * `Ok(Some(page_id))` - Found the key, returns associated page_id
    /// * `Ok(None)` - Key not found in tree
    /// * `Err(...)` - Error during lookup
    pub fn lookup(&self, key: i64) -> NativeResult<Option<u64>> {
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .btree_lookup_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Empty tree: root_page_id is EMPTY_TREE_ROOT (u64::MAX) for new trees,
        // or 0 for databases that haven't created any index pages yet
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            return Ok(None);
        }

        let search_key = key as u64;
        let mut current_page_id = self.root_page_id;
        let mut depth = 0;

        while depth < MAX_TREE_HEIGHT as usize {
            // Get the index page (from cache or load from disk)
            let index_page = self.load_page(current_page_id)?;

            match &index_page {
                IndexPage::Leaf {
                    entries, next_leaf, ..
                } => {
                    // Binary search for key in leaf entries
                    let result = IndexPage::binary_search_leaf(entries, search_key);
                    let result = match result {
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
                            // Key not found in this leaf
                            if *next_leaf == 0 {
                                Ok(None)
                            } else {
                                // Continue to next leaf (for range queries, not needed for exact match)
                                current_page_id = *next_leaf;
                                continue;
                            }
                        }
                    };

                    // Record traversal depth before returning
                    #[cfg(feature = "v3-forensics")]
                    FORENSIC_COUNTERS
                        .btree_traversal_depth_total
                        .fetch_add((depth + 1) as u64, std::sync::atomic::Ordering::Relaxed);

                    return result;
                }
                IndexPage::Internal { keys, children, .. } => {
                    // Find child index using binary search
                    let child_idx = IndexPage::find_child_index(keys, search_key);
                    if child_idx < children.len() {
                        current_page_id = children[child_idx];
                    } else {
                        #[cfg(feature = "v3-forensics")]
                        FORENSIC_COUNTERS
                            .btree_traversal_depth_total
                            .fetch_add((depth + 1) as u64, std::sync::atomic::Ordering::Relaxed);

                        return Err(NativeBackendError::InvalidHeader {
                            field: "btree_internal".to_string(),
                            reason: format!("child index {} out of bounds", child_idx),
                        });
                    }
                }
            }

            depth += 1;
        }

        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .btree_traversal_depth_total
            .fetch_add(depth as u64, std::sync::atomic::Ordering::Relaxed);

        Err(NativeBackendError::InvalidHeader {
            field: "btree_depth".to_string(),
            reason: format!("exceeded maximum depth {}", MAX_TREE_HEIGHT),
        })
    }

    /// Asynchronously look up a key in the B+Tree.
    pub async fn lookup_async(
        &self,
        key: i64,
        async_coordinator: &AsyncFileCoordinator,
    ) -> Result<Option<u64>, SqliteGraphError> {
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            return Ok(None);
        }

        let search_key = key as u64;
        let mut current_page_id = self.root_page_id;
        let mut depth = 0;

        while depth < MAX_TREE_HEIGHT as usize {
            let index_page = self
                .load_page_async(current_page_id, async_coordinator)
                .await?;

            match &index_page {
                IndexPage::Leaf {
                    entries, next_leaf, ..
                } => {
                    let result = IndexPage::binary_search_leaf(entries, search_key);
                    let res_val = match result {
                        Ok(idx) => {
                            if let Some((_, page_id)) = entries.get(idx) {
                                Ok(Some(*page_id))
                            } else {
                                Err(SqliteGraphError::validation(
                                    "entry index out of bounds in BTree leaf",
                                ))
                            }
                        }
                        Err(_idx) => {
                            if *next_leaf == 0 {
                                Ok(None)
                            } else {
                                current_page_id = *next_leaf;
                                continue;
                            }
                        }
                    };
                    return res_val;
                }
                IndexPage::Internal { keys, children, .. } => {
                    let child_idx = IndexPage::find_child_index(keys, search_key);
                    if child_idx < children.len() {
                        current_page_id = children[child_idx];
                    } else {
                        return Err(SqliteGraphError::validation(
                            "child index out of bounds in BTree internal",
                        ));
                    }
                }
            }
            depth += 1;
        }
        Ok(None)
    }

    /// Get all keys stored in the B+Tree by traversing leaf pages horizontally.
    pub fn keys(&self) -> NativeResult<Vec<i64>> {
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            return Ok(Vec::new());
        }

        // 1. Traverse down the leftmost path to find the first leaf
        let mut current_page_id = self.root_page_id;
        let mut depth = 0;
        loop {
            let index_page = self.load_page(current_page_id)?;
            match index_page {
                IndexPage::Leaf { .. } => break,
                IndexPage::Internal { children, .. } => {
                    if children.is_empty() {
                        return Ok(Vec::new());
                    }
                    current_page_id = children[0];
                }
            }
            depth += 1;
            if depth > MAX_TREE_HEIGHT as usize {
                return Err(NativeBackendError::InvalidHeader {
                    field: "btree_depth".to_string(),
                    reason: format!(
                        "exceeded maximum depth {} in keys traversal",
                        MAX_TREE_HEIGHT
                    ),
                });
            }
        }

        // 2. Traverse leaf pages horizontally
        let mut keys = Vec::new();
        while current_page_id != 0 {
            let index_page = self.load_page(current_page_id)?;
            match index_page {
                IndexPage::Leaf {
                    entries, next_leaf, ..
                } => {
                    for (k, _) in entries {
                        keys.push(k as i64);
                    }
                    current_page_id = next_leaf;
                }
                _ => {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_traverse".to_string(),
                        reason: "Expected leaf page in horizontal traversal".to_string(),
                    });
                }
            }
        }

        Ok(keys)
    }

    /// Insert key->value mapping into B+Tree
    ///
    /// Inserts a new node_id -> page_id mapping into the B+Tree.
    /// Uses preemptive splitting (top-down) to ensure nodes are never full during insertion.
    ///
    /// # Arguments
    ///
    /// * `key` - Node ID to insert
    /// * `value` - Page ID associated with the node
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Insert successful
    /// * `Err(...)` - Error during insert
    pub fn insert(&mut self, key: i64, value: u64) -> NativeResult<()> {
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .btree_insert_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Handle empty tree case (EMPTY_TREE_ROOT or 0 for uninitialized)
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            return self.insert_into_empty_tree(key, value);
        }

        let search_key = key as u64;

        // Check if root needs splitting first
        let root_page = self.load_page(self.root_page_id)?;
        if root_page.needs_split_internal() || root_page.needs_split_leaf() {
            self.split_root()?;
        }

        // Descend with preemptive splitting
        self.insert_non_full(self.root_page_id, search_key, value)
    }

    /// Insert into a non-full page, splitting children as needed during descent
    fn insert_non_full(&mut self, page_id: u64, key: u64, value: u64) -> NativeResult<()> {
        let page = self.load_page(page_id)?;

        #[cfg(debug_assertions)]
        page.verify_invariants()?;

        let is_root = page.is_root();

        match page {
            IndexPage::Leaf {
                mut entries,
                page_id: pid,
                next_leaf,
                checksum,
                ..
            } => {
                // Check if key already exists (update case)
                match IndexPage::binary_search_leaf(&entries, key) {
                    Ok(idx) => {
                        entries[idx] = (key, value);
                    }
                    Err(idx) => {
                        entries.insert(idx, (key, value));
                    }
                }

                // Reconstruct page and write
                let updated_page = IndexPage::Leaf {
                    page_id: pid,
                    entries,
                    next_leaf,
                    checksum,
                    is_root,
                };
                self.write_page(&updated_page)?;
                Ok(())
            }
            IndexPage::Internal { keys, children, .. } => {
                // Find which child to descend to
                let child_idx = IndexPage::find_child_index(&keys, key);
                let child_id = children[child_idx];

                // Load the child and check if it needs splitting
                let child_page = self.load_page(child_id)?;

                if child_page.needs_split_internal() || child_page.needs_split_leaf() {
                    // Split the child
                    let (_new_child_id, separator_key) = self.split_child(page_id, child_idx)?;

                    // Reload the parent (it was modified by split_child)
                    let updated_parent = self.load_page(page_id)?;

                    // Determine which child to use after split
                    let new_child_idx = if key >= separator_key {
                        child_idx + 1
                    } else {
                        child_idx
                    };

                    if let IndexPage::Internal {
                        children: new_children,
                        ..
                    } = &updated_parent
                    {
                        let next_child_id = new_children[new_child_idx];
                        return self.insert_non_full(next_child_id, key, value);
                    }
                }

                // Descend to the child
                self.insert_non_full(child_id, key, value)
            }
        }
    }

    /// Split the root page, creating a new root
    fn split_root(&mut self) -> NativeResult<()> {
        let old_root_id = self.root_page_id;
        let old_root = self.load_page(old_root_id)?;

        // Allocate new root (internal node)
        let new_root_id = self.allocator.write().allocate()?;
        let mut new_root = IndexPage::new_internal_root(new_root_id);

        // Allocate new sibling page
        let sibling_id = self.allocator.write().allocate()?;

        match &old_root {
            IndexPage::Internal { keys, children, .. } => {
                // For internal nodes, split at (len - 1) / 2 to ensure both halves
                // have at least MIN_KEYS after removing the separator
                let split_idx = (keys.len() - 1) / 2;
                let separator_key = keys[split_idx];

                // Create the new sibling internal node
                let mut sibling = IndexPage::new_internal(sibling_id);
                if let IndexPage::Internal {
                    keys: sib_keys,
                    children: sib_children,
                    ..
                } = &mut sibling
                {
                    // Move upper half to sibling (excluding separator)
                    *sib_keys = keys[split_idx + 1..].to_vec();
                    *sib_children = children[split_idx + 1..].to_vec();
                }

                // Truncate old root (keep lower half, excluding separator)
                let mut truncated_old_root = IndexPage::new_internal(old_root_id);
                if let IndexPage::Internal {
                    keys: old_keys,
                    children: old_children,
                    ..
                } = &mut truncated_old_root
                {
                    *old_keys = keys[..split_idx].to_vec();
                    *old_children = children[..split_idx + 1].to_vec();
                }

                // Set up new root
                if let IndexPage::Internal {
                    keys: root_keys,
                    children: root_children,
                    ..
                } = &mut new_root
                {
                    root_keys.push(separator_key);
                    root_children.push(old_root_id);
                    root_children.push(sibling_id);
                }

                // Write all pages
                self.write_page(&truncated_old_root)?;
                self.write_page(&sibling)?;
                self.write_page(&new_root)?;

                // Update root tracking
                self.root_page_id = new_root_id;
                self.tree_height += 1;
            }
            IndexPage::Leaf {
                entries, next_leaf, ..
            } => {
                let split_idx = entries.len() / 2;
                let separator_key = entries[split_idx].0;

                // Create the new sibling leaf
                let mut sibling = IndexPage::new_leaf(sibling_id);
                if let IndexPage::Leaf {
                    entries: sib_entries,
                    next_leaf: sib_next,
                    ..
                } = &mut sibling
                {
                    *sib_entries = entries[split_idx..].to_vec();
                    *sib_next = *next_leaf;
                }

                // Truncate old root
                let mut truncated_old_root = IndexPage::new_leaf(old_root_id);
                if let IndexPage::Leaf {
                    entries: old_entries,
                    next_leaf: old_next,
                    ..
                } = &mut truncated_old_root
                {
                    *old_entries = entries[..split_idx].to_vec();
                    *old_next = sibling_id;
                }

                // Set up new root (internal node with one key)
                if let IndexPage::Internal {
                    keys: root_keys,
                    children: root_children,
                    ..
                } = &mut new_root
                {
                    root_keys.push(separator_key);
                    root_children.push(old_root_id);
                    root_children.push(sibling_id);
                }

                // Write all pages
                self.write_page(&truncated_old_root)?;
                self.write_page(&sibling)?;
                self.write_page(&new_root)?;

                // Update root tracking
                self.root_page_id = new_root_id;
                self.tree_height += 1;
            }
        }

        // Log to WAL
        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            wal_guard.page_allocate(new_root_id)?;
            let page_bytes = self.load_page(new_root_id)?.pack()?;
            wal_guard.page_write(new_root_id, 0, page_bytes.to_vec())?;
        }

        Ok(())
    }

    /// Split a child page during descent
    /// Returns (new_child_id, separator_key)
    fn split_child(&mut self, parent_id: u64, child_idx: usize) -> NativeResult<(u64, u64)> {
        let parent = self.load_page(parent_id)?;
        let child_id = match &parent {
            IndexPage::Internal { children, .. } => children[child_idx],
            _ => {
                return Err(NativeBackendError::InvalidHeader {
                    field: "btree_split_child".to_string(),
                    reason: "parent is not an internal node".to_string(),
                });
            }
        };

        let child = self.load_page(child_id)?;
        let new_page_id = self.allocator.write().allocate()?;

        match &child {
            IndexPage::Internal { keys, children, .. } => {
                // For internal nodes, split at (len - 1) / 2 to ensure both halves
                // have at least MIN_KEYS after removing the separator
                let split_idx = (keys.len() - 1) / 2;
                let separator_key = keys[split_idx];

                // Create sibling internal node
                let mut sibling = IndexPage::new_internal(new_page_id);
                if let IndexPage::Internal {
                    keys: sib_keys,
                    children: sib_children,
                    ..
                } = &mut sibling
                {
                    *sib_keys = keys[split_idx + 1..].to_vec();
                    *sib_children = children[split_idx + 1..].to_vec();
                }

                // Truncate original child (keep lower half)
                let mut truncated_child = IndexPage::new_internal(child_id);
                if let IndexPage::Internal {
                    keys: child_keys,
                    children: child_children,
                    ..
                } = &mut truncated_child
                {
                    *child_keys = keys[..split_idx].to_vec();
                    *child_children = children[..split_idx + 1].to_vec();
                }

                // Update parent - insert separator and new child
                let mut updated_parent = parent.clone();
                if let IndexPage::Internal {
                    keys: p_keys,
                    children: p_children,
                    ..
                } = &mut updated_parent
                {
                    p_keys.insert(child_idx, separator_key);
                    p_children.insert(child_idx + 1, new_page_id);
                }

                // Write all modified pages
                self.write_page(&truncated_child)?;
                self.write_page(&sibling)?;
                self.write_page(&updated_parent)?;

                // Log to WAL
                if let Some(ref wal) = self.wal {
                    let mut wal_guard = wal.write();
                    wal_guard.btree_split(child_id, new_page_id, separator_key, false)?;
                }

                Ok((new_page_id, separator_key))
            }
            IndexPage::Leaf {
                entries, next_leaf, ..
            } => {
                let split_idx = entries.len() / 2;
                let separator_key = entries[split_idx].0;

                // Create sibling leaf node
                let mut sibling = IndexPage::new_leaf(new_page_id);
                if let IndexPage::Leaf {
                    entries: sib_entries,
                    next_leaf: sib_next,
                    ..
                } = &mut sibling
                {
                    *sib_entries = entries[split_idx..].to_vec();
                    *sib_next = *next_leaf;
                }

                // Truncate original child (keep lower half)
                let mut truncated_child = IndexPage::new_leaf(child_id);
                if let IndexPage::Leaf {
                    entries: child_entries,
                    next_leaf: child_next,
                    ..
                } = &mut truncated_child
                {
                    *child_entries = entries[..split_idx].to_vec();
                    *child_next = new_page_id;
                }

                // Update parent - insert separator and new child
                let mut updated_parent = parent.clone();
                if let IndexPage::Internal {
                    keys: p_keys,
                    children: p_children,
                    ..
                } = &mut updated_parent
                {
                    p_keys.insert(child_idx, separator_key);
                    p_children.insert(child_idx + 1, new_page_id);
                }

                // Write all modified pages
                self.write_page(&truncated_child)?;
                self.write_page(&sibling)?;
                self.write_page(&updated_parent)?;

                // Log to WAL
                if let Some(ref wal) = self.wal {
                    let mut wal_guard = wal.write();
                    wal_guard.btree_split(child_id, new_page_id, separator_key, true)?;
                }

                Ok((new_page_id, separator_key))
            }
        }
    }

    /// Delete key from B+Tree
    ///
    /// Removes a key->value mapping from the B+Tree.
    /// Returns true if the key was found and deleted, false otherwise.
    ///
    /// # Arguments
    ///
    /// * `key` - Node ID to delete
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Key was found and deleted
    /// * `Ok(false)` - Key was not found
    /// * `Err(...)` - Error during delete
    pub fn delete(&mut self, key: i64) -> NativeResult<bool> {
        if self.root_page_id == EMPTY_TREE_ROOT || self.root_page_id == 0 {
            return Ok(false);
        }

        let search_key = key as u64;
        let leaf_page_id = self.find_leaf(self.root_page_id, search_key)?;

        let mut leaf_page = self.load_page(leaf_page_id)?;

        if let IndexPage::Leaf { entries, .. } = &mut leaf_page {
            match IndexPage::binary_search_leaf(entries, search_key) {
                Ok(idx) => {
                    entries.remove(idx);
                    self.write_page(&leaf_page)?;
                    Ok(true)
                }
                Err(_) => Ok(false), // Key not found
            }
        } else {
            Err(NativeBackendError::InvalidHeader {
                field: "btree_delete".to_string(),
                reason: "expected leaf page".to_string(),
            })
        }
    }

    //========================================================================
    // Private helper methods
    //========================================================================

    /// Insert into empty tree (create first leaf page as root)
    fn insert_into_empty_tree(&mut self, key: i64, value: u64) -> NativeResult<()> {
        // Allocate a new page for the root
        let page_id = self.allocator.write().allocate()?;

        // Create a new leaf page as root
        let mut leaf = IndexPage::new_leaf_root(page_id);

        // Add the entry
        if let IndexPage::Leaf { entries, .. } = &mut leaf {
            entries.push((key as u64, value));
        }

        // Write the page
        self.write_page(&leaf)?;

        // Update root
        self.root_page_id = page_id;
        self.tree_height = 1;

        // Log to WAL if enabled
        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            wal_guard.page_allocate(page_id)?;
            let page_bytes = leaf.pack()?;
            wal_guard.page_write(page_id, 0, page_bytes.to_vec())?;
        }

        Ok(())
    }

    /// Find leaf page for key (without tracking path)
    fn find_leaf(&self, root_page_id: u64, search_key: u64) -> NativeResult<u64> {
        let mut current_page_id = root_page_id;
        let mut depth = 0;

        while depth < MAX_TREE_HEIGHT as usize {
            let page = self.load_page(current_page_id)?;

            match &page {
                IndexPage::Leaf { .. } => {
                    return Ok(current_page_id);
                }
                IndexPage::Internal { keys, children, .. } => {
                    let child_idx = IndexPage::find_child_index(keys, search_key);
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
}
