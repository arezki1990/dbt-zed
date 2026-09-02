//! Block-aware TraversalCache prototype
//!
//! This module implements a block-locality aware cache for NodePage instances.
//! The goal is to test whether logical block grouping improves cache hit rates
//! for graph traversal operations.
//!
//! # Prototype Design (METADATA-ONLY)
//!
//! ## Block Assignment
//! - Computed from node_id: `block_id = (node_id - 1) / BLOCK_SIZE`
//! - No storage format changes
//! - Deterministic and stable across runs
//!
//! ## Cache Behavior
//! - Track which block each cached page belongs to
//! - When cache is full, prefer evicting pages from "cold" blocks
//! - Pages from recently accessed blocks are retained longer
//!
//! ## Limitations (intentional for this prototype)
//! - No physical placement changes
//! - No insert path modifications
//! - No quadtree/spatial grouping
//! - No persistent block metadata
//!
//! # Future Work
//!
//! If this prototype shows improvement, consider:
//! - Adding block_id to NodePage header for persistent tracking
//! - Block-aware insert placement
//! - Spatial/group-based block assignment
//! - Prefetch based on block membership

use super::NodePage;
use std::collections::HashMap;
use std::sync::Arc;

/// Block size in nodes
///
/// Each block contains approximately this many node IDs.
/// Chosen to align with typical page capacity (~50 nodes/page).
pub const BLOCK_SIZE: i64 = 128;

/// Minimum capacity for TraversalCache
pub const MIN_CACHE_CAPACITY: usize = 1;

/// Maximum capacity for TraversalCache
pub const MAX_CACHE_CAPACITY: usize = 256;

/// Default capacity for TraversalCache
/// Default of 64 pages was determined by cache capacity sweep benchmark
pub const DEFAULT_CACHE_CAPACITY: usize = 64;

/// Compute block_id from node_id
///
/// Block 0: nodes 1-128
/// Block 1: nodes 129-256
/// etc.
#[inline]
pub fn node_id_to_block(node_id: i64) -> i64 {
    if node_id < 1 {
        return 0;
    }
    (node_id - 1) / BLOCK_SIZE
}

/// Cache entry with block tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    /// The cached page
    page: Arc<NodePage>,
    /// Which block this page belongs to (computed from page contents)
    block_id: i64,
    /// Access counter for LRU within the block
    access_count: u64,
}

/// Block-aware LRU cache for NodePage instances during graph traversal
///
/// This cache tracks which block each page belongs to and prefers
/// retaining pages from recently-accessed blocks.
pub struct BlockAwareTraversalCache {
    /// Primary cache: page_id -> cached entry with block info
    cache: HashMap<u64, CacheEntry>,

    /// Per-block access tracking: block_id -> last access time
    ///
    /// This is used to determine which blocks are "hot" (recently accessed)
    /// and should be retained during eviction.
    block_access: HashMap<i64, u64>,

    /// Global access counter for ordering
    global_access_counter: u64,

    /// Maximum number of pages to cache
    capacity: usize,

    /// Statistics
    hits: u64,
    misses: u64,
    /// Block-aware evictions (evicted from different block than current access)
    block_aware_evictions: u64,
}

impl BlockAwareTraversalCache {
    pub fn new(capacity: usize) -> Self {
        assert!((MIN_CACHE_CAPACITY..=MAX_CACHE_CAPACITY).contains(&capacity));
        Self {
            cache: HashMap::with_capacity(capacity),
            block_access: HashMap::new(),
            global_access_counter: 0,
            capacity,
            hits: 0,
            misses: 0,
            block_aware_evictions: 0,
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CACHE_CAPACITY)
    }

    /// Infer block_id from a page by examining the node IDs it contains
    fn infer_block_id(page: &NodePage) -> i64 {
        if let Some(first_node) = page.nodes.first() {
            node_id_to_block(first_node.id())
        } else {
            0
        }
    }

    pub fn get(&mut self, page_id: u64) -> Option<Arc<NodePage>> {
        self.global_access_counter += 1;

        if let Some(entry) = self.cache.get_mut(&page_id) {
            // Cache hit
            self.hits += 1;
            entry.access_count = self.global_access_counter;

            // Update block access time
            *self.block_access.entry(entry.block_id).or_insert(0) = self.global_access_counter;

            Some(entry.page.clone())
        } else {
            // Cache miss
            self.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, page_id: u64, page: Arc<NodePage>) {
        let block_id = Self::infer_block_id(&page);

        // Check if we need to evict
        while self.cache.len() >= self.capacity {
            if let Some(to_evict) = self.select_eviction_candidate() {
                self.cache.remove(&to_evict);
            } else {
                break;
            }
        }

        // Update block access time
        *self.block_access.entry(block_id).or_insert(0) = self.global_access_counter;

        self.cache.insert(
            page_id,
            CacheEntry {
                page,
                block_id,
                access_count: self.global_access_counter,
            },
        );
    }

    /// Select a page to evict using block-aware policy
    ///
    /// Strategy:
    /// 1. Prefer pages from blocks that haven't been accessed recently
    /// 2. Among those, prefer least recently used pages
    fn select_eviction_candidate(&mut self) -> Option<u64> {
        // Find the block with the oldest access time
        let coldest_block = *self
            .block_access
            .iter()
            .min_by_key(|(_, time)| *time)
            .map(|(block, _)| block)?;

        // Find the least recently used page from the coldest block
        let mut coldest_page_in_block: Option<(u64, u64)> = None;

        for (&page_id, entry) in &self.cache {
            if entry.block_id == coldest_block {
                match &coldest_page_in_block {
                    None => {
                        coldest_page_in_block = Some((page_id, entry.access_count));
                    }
                    Some((_, oldest_access)) => {
                        if entry.access_count < *oldest_access {
                            coldest_page_in_block = Some((page_id, entry.access_count));
                        }
                    }
                }
            }
        }

        if let Some((page_id, _)) = coldest_page_in_block {
            // Remove the page from cache
            self.cache.remove(&page_id);

            // Check if this was the last page from this block
            let any_remaining = self.cache.values().any(|e| e.block_id == coldest_block);
            if !any_remaining {
                self.block_access.remove(&coldest_block);
            }

            self.block_aware_evictions += 1;
            Some(page_id)
        } else {
            // Fallback: simple LRU from any block
            let oldest = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.access_count)
                .map(|(&page_id, _)| page_id)?;

            self.cache.remove(&oldest);
            Some(oldest)
        }
    }

    pub fn invalidate(&mut self, page_id: u64) -> bool {
        if let Some(entry) = self.cache.remove(&page_id) {
            // Check if this was the last page from this block
            let any_remaining = self.cache.values().any(|e| e.block_id == entry.block_id);
            if !any_remaining {
                self.block_access.remove(&entry.block_id);
            }
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.block_access.clear();
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn contains(&self, page_id: &u64) -> bool {
        self.cache.contains_key(page_id)
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Get statistics about block distribution in cache
    pub fn block_stats(&self) -> BlockStats {
        let mut block_counts: HashMap<i64, usize> = HashMap::new();

        for entry in self.cache.values() {
            *block_counts.entry(entry.block_id).or_insert(0) += 1;
        }

        let unique_blocks = block_counts.len();
        let total_blocks = self.block_access.len();

        BlockStats {
            unique_blocks_in_cache: unique_blocks,
            tracked_blocks: total_blocks,
            pages_in_cache: self.cache.len(),
        }
    }

    /// Get the number of block-aware evictions
    pub fn block_aware_evictions(&self) -> u64 {
        self.block_aware_evictions
    }
}

/// Statistics about block distribution in cache
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStats {
    /// Number of unique blocks currently represented in cache
    pub unique_blocks_in_cache: usize,
    /// Total number of blocks with any access tracking
    pub tracked_blocks: usize,
    /// Total number of pages in cache
    pub pages_in_cache: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_to_block() {
        // Block 0: nodes 1-128
        assert_eq!(node_id_to_block(1), 0);
        assert_eq!(node_id_to_block(64), 0);
        assert_eq!(node_id_to_block(128), 0);

        // Block 1: nodes 129-256
        assert_eq!(node_id_to_block(129), 1);
        assert_eq!(node_id_to_block(200), 1);
        assert_eq!(node_id_to_block(256), 1);

        // Block 2: nodes 257-384
        assert_eq!(node_id_to_block(257), 2);
    }

    #[test]
    fn test_cache_basic_operations() {
        let mut cache = BlockAwareTraversalCache::new(4);

        assert!(cache.get(1).is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.len(), 0);

        // Note: Can't easily test insert without actual NodePage instances
        // This is tested at integration level
    }

    #[test]
    fn test_cache_hit_rate() {
        let mut cache = BlockAwareTraversalCache::new(4);

        // No accesses
        assert_eq!(cache.hit_rate(), 0.0);

        // All misses
        cache.get(1);
        cache.get(2);
        assert_eq!(cache.hit_rate(), 0.0);
        assert_eq!(cache.misses(), 2);
    }
}
