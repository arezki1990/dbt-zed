//! NodeStore V3 - B+Tree Lookup Integration (Task 66-01)
//!
//! This module implements NodeStore for O(log n) node_id -> page_id lookups
//! using B+Tree index traversal. Includes TraversalCache for per-traversal
//! page caching.
//!
//! # Architecture
//!
//! 1. **NodeStore**: B+Tree-based node lookup with page caching
//! 2. **TraversalCache**: LRU cache for NodePage instances
//! 3. **Page Loading**: Loads NodePage from disk using page_id
//! 4. **Decompression**: Extracts NodeRecordV3 from compressed page data
//!
//! # B+Tree Lookup Algorithm
//!
//! The B+Tree provides O(log n) lookup:
//! - Start at root page
//! - For internal pages: binary search keys, descend to child
//! - For leaf pages: binary search entries for exact match

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::types::NodeFlags;
use crate::backend::native::v3::allocator::PageAllocator;
use crate::backend::native::v3::btree::BTreeManager;
use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
use crate::backend::native::v3::file_coordinator::FileCoordinator;
#[cfg(feature = "v3-forensics")]
use crate::backend::native::v3::forensics::{FORENSIC_COUNTERS, PageType, Subsystem};
use crate::backend::native::v3::header::PersistentHeaderV3;
use crate::backend::native::v3::index::IndexPage;
use crate::backend::native::v3::node::{NodePage, NodeRecordV3};
use crate::backend::native::v3::wal::WALWriter;
use lru::LruCache;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

mod async_lookup;
mod lookup_support;
mod mutation_support;
mod page_loader;
#[cfg(test)]
mod tests;
mod traversal_cache;

pub use async_lookup::lookup_node_async;
pub use page_loader::PageLoader;
pub use traversal_cache::{
    DEFAULT_CACHE_CAPACITY, MAX_CACHE_CAPACITY, MIN_CACHE_CAPACITY, TraversalCache,
    TraversalCacheBuilder,
};

//=============================================================================
// Constants
//=============================================================================

/// Maximum B+Tree height for safety
const MAX_TREE_HEIGHT: u32 = 10;

/// Page cache size for NodeStore.
///
/// 1024 pages = 4 MiB at the 4 KiB default page size. This holds the working
/// set of graphs up to ~16k nodes without thrashing. The previous value of 64
/// pages (256 KiB) evicted constantly on any graph larger than a few hundred
/// nodes. Eviction is LRU (least-recently-used), so hot pages stay resident
/// even when the working set exceeds capacity.
pub const PAGE_CACHE_SIZE: usize = 1024;

/// Construct a page-cache-sized `LruCache` for `PAGE_CACHE_SIZE` entries.
fn new_page_cache<K: std::hash::Hash + Eq, V>() -> LruCache<K, V> {
    LruCache::new(NonZeroUsize::new(PAGE_CACHE_SIZE).expect("PAGE_CACHE_SIZE > 0"))
}

/// Construct an `LruCache` of an explicit capacity (used by `with_capacity`).
fn new_lru<K: std::hash::Hash + Eq, V>(capacity: usize) -> LruCache<K, V> {
    LruCache::new(NonZeroUsize::new(capacity.max(1)).expect("capacity must be >= 1"))
}

//=============================================================================
// NodeStore: B+Tree Lookup Integration
//=============================================================================

/// NodeStore for B+Tree-based node lookup
pub struct NodeStore {
    db_path: PathBuf,
    /// Coordinated file handle for all main DB I/O (optional for backward compatibility)
    /// When set, all file writes go through this coordinator to prevent race conditions
    file_coordinator: Option<Arc<FileCoordinator>>,
    root_page_id: u64,
    tree_height: u32,
    /// Thread-safe node page cache - accessible from both read and write contexts
    /// This fixes the cache bypass bug where read-only lookups couldn't populate the cache.
    /// Bounded LRU cache: hot pages stay resident, least-recently-used evicted on overflow.
    page_cache: Arc<RwLock<LruCache<u64, Vec<u8>>>>,
    /// Cache of unpacked NodePages - avoids repeated unpacking on cache hits
    /// This is a separate cache from page_cache because unpacked pages are more expensive to reconstruct.
    /// Bounded LRU cache (same capacity as `page_cache`).
    unpacked_page_cache: Arc<RwLock<LruCache<u64, Arc<NodePage>>>>,
    cache_capacity: usize,
    /// Block ID of the most recently accessed page (for block-aware eviction)
    /// PROTOTYPE: Track current access block to prefer retaining same-block pages
    current_access_block: std::sync::atomic::AtomicI64,
    /// Block-to-preferred-pages mapping for physical placement prototype
    /// PROTOTYPE: In-memory only, biases same-block nodes to same pages
    /// Maps block_id → list of page_ids preferred for that block
    block_preferred_pages: HashMap<i64, Vec<u64>>,
    /// Maximum preferred pages to track per block (tunable)
    max_preferred_pages_per_block: usize,
    index_cache: LruCache<u64, IndexPage>,
    /// B+Tree manager for index operations
    btree_manager: Option<BTreeManager>,
    /// Page allocator for page management (shared with BTreeManager)
    page_allocator: Option<Arc<RwLock<PageAllocator>>>,
    /// Optional WAL writer for durability
    wal_writer: Option<WALWriter>,
    /// Next available node ID
    next_node_id: i64,
    /// Dirty page buffer for batch writes (page_id -> NodePage)
    dirty_pages: HashMap<u64, NodePage>,
    /// Whether batch mode is active (defer disk writes)
    batch_mode: bool,
}

impl NodeStore {
    pub fn new(header: &PersistentHeaderV3, db_path: PathBuf) -> Self {
        NodeStore {
            db_path,
            file_coordinator: None,
            root_page_id: header.root_index_page,
            tree_height: header.btree_height,
            page_cache: Arc::new(RwLock::new(new_page_cache())),
            unpacked_page_cache: Arc::new(RwLock::new(new_page_cache())),
            cache_capacity: PAGE_CACHE_SIZE,
            current_access_block: std::sync::atomic::AtomicI64::new(-1),
            block_preferred_pages: HashMap::new(),
            max_preferred_pages_per_block: 3,
            index_cache: new_page_cache(),
            btree_manager: None,
            page_allocator: None,
            wal_writer: None,
            next_node_id: 1, // Start from 1
            dirty_pages: HashMap::new(),
            batch_mode: false,
        }
    }

    pub fn btree_manager(&self) -> &Option<BTreeManager> {
        &self.btree_manager
    }

    pub fn page_cache_ref(&self) -> &Arc<RwLock<LruCache<u64, Vec<u8>>>> {
        &self.page_cache
    }

    pub fn unpacked_page_cache_ref(&self) -> &Arc<RwLock<LruCache<u64, Arc<NodePage>>>> {
        &self.unpacked_page_cache
    }

    /// Configured page-cache capacity (pages).
    pub fn cache_capacity(&self) -> usize {
        self.cache_capacity
    }

    /// Current number of resident entries in the raw-bytes page cache.
    pub fn page_cache_len(&self) -> usize {
        self.page_cache.read().len()
    }

    /// Current number of resident entries in the unpacked-page cache.
    pub fn unpacked_page_cache_len(&self) -> usize {
        self.unpacked_page_cache.read().len()
    }

    /// Whether `page_id` is currently resident in the raw-bytes page cache.
    /// Uses `peek` (no recency update) so the check itself doesn't perturb LRU order.
    pub fn page_cache_contains(&self, page_id: u64) -> bool {
        self.page_cache.read().peek(&page_id).is_some()
    }

    /// Resolve the page_id for a given node_id via the btree index.
    /// Read-only (no cache side effects). Used by cache-residency tests.
    pub fn page_id_for_node(&self, node_id: i64) -> NativeResult<Option<u64>> {
        self.lookup_page_ro(node_id)
    }

    /// Clear both the raw-bytes page cache and the unpacked-page cache, and the
    /// index cache. Used by tests and on cache invalidation.
    pub fn clear_all_caches(&mut self) {
        self.page_cache.write().clear();
        self.unpacked_page_cache.write().clear();
        self.index_cache.clear();
    }

    pub fn with_capacity(
        header: &PersistentHeaderV3,
        db_path: PathBuf,
        cache_capacity: usize,
    ) -> Self {
        NodeStore {
            db_path,
            file_coordinator: None,
            root_page_id: header.root_index_page,
            tree_height: header.btree_height,
            page_cache: Arc::new(RwLock::new(new_lru(cache_capacity))),
            unpacked_page_cache: Arc::new(RwLock::new(new_lru(cache_capacity))),
            cache_capacity,
            current_access_block: std::sync::atomic::AtomicI64::new(-1),
            block_preferred_pages: HashMap::new(),
            max_preferred_pages_per_block: 3,
            index_cache: new_lru(cache_capacity),
            btree_manager: None,
            page_allocator: None,
            wal_writer: None,
            next_node_id: 1,
            dirty_pages: HashMap::new(),
            batch_mode: false,
        }
    }

    /// Initialize the store with BTreeManager, PageAllocator and optional WAL
    pub fn initialize(
        &mut self,
        btree_manager: BTreeManager,
        page_allocator: Arc<RwLock<PageAllocator>>,
        wal_writer: Option<WALWriter>,
    ) {
        self.btree_manager = Some(btree_manager);
        self.page_allocator = Some(page_allocator);
        self.wal_writer = wal_writer;
    }

    /// Set the WAL writer
    pub fn set_wal_writer(&mut self, wal: WALWriter) {
        self.wal_writer = Some(wal);
    }

    /// Set the file coordinator for coordinated I/O
    ///
    /// When set, all file reads/writes go through this coordinator to prevent
    /// race conditions and avoid per-cache-miss file open/close overhead.
    /// The coordinator is also propagated to the internal BTreeManager so
    /// index-page lookups share the same persistent handle.
    pub fn set_file_coordinator(&mut self, coordinator: Arc<FileCoordinator>) {
        if let Some(ref mut btree) = self.btree_manager {
            btree.set_file_coordinator(Arc::clone(&coordinator));
        }
        self.file_coordinator = Some(coordinator);
    }

    /// Enable batch mode (defer disk writes until commit)
    ///
    /// When batch mode is enabled, page writes are staged in memory
    /// and flushed to disk with a single fsync on commit.
    pub fn begin_batch(&mut self) {
        self.batch_mode = true;
        self.dirty_pages.clear();
    }

    /// Commit all staged dirty pages with single fsync
    ///
    /// Returns the number of pages flushed.
    pub fn commit_batch(&mut self) -> NativeResult<usize> {
        if !self.batch_mode {
            return Ok(0);
        }

        let page_count = self.dirty_pages.len();

        if page_count > 0 {
            // Use file_coordinator if available (FIXES 10K-node bug)
            if let Some(coordinator) = &self.file_coordinator {
                // Write all dirty pages through coordinator
                for (page_id, page) in &self.dirty_pages {
                    let page_bytes = page.pack()?;
                    coordinator.write_page(*page_id, &page_bytes)?;
                    // Update cache - convert array to Vec
                    self.page_cache_insert(*page_id, page_bytes.to_vec());
                }
                // Clear dirty pages
                self.dirty_pages.clear();
            } else {
                // Fallback to original behavior for backward compatibility
                // CRITICAL FIX: Do NOT use create(true) - it truncates the file!
                // Use create(false) to avoid truncating existing data
                let file_exists = self.db_path.exists();
                let mut file = OpenOptions::new()
                    .write(true)
                    .create(!file_exists) // Only create if file doesn't exist
                    .open(&self.db_path)
                    .map_err(|e| NativeBackendError::IoError {
                        context: format!(
                            "Failed to open database file for batch commit: {}",
                            self.db_path.display()
                        ),
                        source: e,
                    })?;

                // CRITICAL: Find the maximum offset needed and extend file once
                // This is more efficient than extending for each page
                let mut required_len = file.metadata().map(|m| m.len()).unwrap_or(0);

                for (page_id, page) in &self.dirty_pages {
                    let page_bytes = page.pack()?;
                    let offset = Self::page_offset(*page_id);
                    let page_end = offset + page_bytes.len() as u64;
                    if page_end > required_len {
                        required_len = page_end;
                    }
                }

                // Extend file if needed
                let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
                if required_len > current_len {
                    file.set_len(required_len)
                        .map_err(|e| NativeBackendError::IoError {
                            context: format!(
                                "Failed to extend file to {} bytes for batch commit",
                                required_len
                            ),
                            source: e,
                        })?;
                }

                // Write all dirty pages
                for (page_id, page) in &self.dirty_pages {
                    let page_bytes = page.pack()?;
                    let offset = Self::page_offset(*page_id);

                    file.seek(SeekFrom::Start(offset)).map_err(|e| {
                        NativeBackendError::IoError {
                            context: format!(
                                "Failed to seek to page {} offset {}",
                                page_id, offset
                            ),
                            source: e,
                        }
                    })?;

                    file.write_all(&page_bytes)
                        .map_err(|e| NativeBackendError::IoError {
                            context: format!(
                                "Failed to write page {} during batch commit",
                                page_id
                            ),
                            source: e,
                        })?;

                    // Update cache
                    self.page_cache_insert(*page_id, page_bytes.to_vec());
                }

                // Single fsync for all pages
                file.sync_all().map_err(|e| NativeBackendError::IoError {
                    context: "Failed to sync batch commit to disk".to_string(),
                    source: e,
                })?;

                // Clear dirty pages
                self.dirty_pages.clear();
            }
        }

        self.batch_mode = false;
        Ok(page_count)
    }

    /// Rollback batch - discard staged pages without writing
    pub fn rollback_batch(&mut self) {
        self.dirty_pages.clear();
        self.batch_mode = false;
    }

    /// Check if batch mode is active
    pub fn is_batch_mode(&self) -> bool {
        self.batch_mode
    }

    /// Get count of dirty pages staged for commit
    pub fn dirty_page_count(&self) -> usize {
        self.dirty_pages.len()
    }

    /// Get mutable reference to BTreeManager
    fn btree_manager_mut(&mut self) -> NativeResult<&mut BTreeManager> {
        self.btree_manager
            .as_mut()
            .ok_or_else(|| NativeBackendError::InvalidHeader {
                field: "btree_manager".to_string(),
                reason: "BTreeManager not initialized".to_string(),
            })
    }

    /// Get write lock on PageAllocator
    fn page_allocator_mut(&self) -> NativeResult<parking_lot::RwLockWriteGuard<'_, PageAllocator>> {
        self.page_allocator
            .as_ref()
            .ok_or_else(|| NativeBackendError::InvalidHeader {
                field: "page_allocator".to_string(),
                reason: "PageAllocator not initialized".to_string(),
            })
            .map(|arc| arc.write())
    }
}
