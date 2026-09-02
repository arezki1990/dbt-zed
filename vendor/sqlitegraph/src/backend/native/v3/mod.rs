//! V3 native backend implementation
//!
//! This module implements V3 storage format with:
//! - B+Tree node index for unlimited capacity
//! - Page-based node storage
//! - Delta/varint compression (Phase 63b)
//! - Page allocator (Phase 64)
//! - Write-Ahead Logging for crash recovery (Phase 65)

pub mod algorithm;
pub mod allocator;
pub mod async_io;
pub mod backend;
pub mod btree;
pub mod compact_edge_record;
pub mod compression;
pub mod constants;
pub mod edge_async;
pub mod edge_compat;
pub mod file_coordinator;
#[cfg(feature = "v3-forensics")]
pub mod forensics;
pub mod header;
pub mod index;
pub mod index_persistence;
pub mod kind_index;
pub mod kv_store;
pub mod name_index;
pub mod node;
pub mod pubsub;
pub mod storage;
pub mod string_table;
pub mod wal;
pub mod write_batch;

pub use async_io::coordinator::AsyncFileCoordinator;

// Re-export V3 types
pub use header::{PersistentHeaderV3, offset as header_offset, size as header_size};

/// V3 magic bytes for file format identification
pub use constants::V3_MAGIC;

/// V3 format version
pub use constants::V3_FORMAT_VERSION;

/// V3 header size
pub use constants::V3_HEADER_SIZE;

// Re-export index types
pub use index::{IndexPage, IndexPageType};

// Re-export node types
pub use node::{
    DEFAULT_CACHE_CAPACITY, FIXED_METADATA_SIZE, MAX_CACHE_CAPACITY, MAX_INLINE_DATA,
    MAX_NODE_CAPACITY, MAX_PAGE_SIZE as NODE_PAGE_SIZE, MIN_CACHE_CAPACITY, NodePage, NodeRecordV3,
    NodeStore, PAGE_HEADER_SIZE as NODE_PAGE_HEADER_SIZE, TraversalCache, TraversalCacheBuilder,
    USABLE_SIZE as NODE_PAGE_USABLE_SIZE,
};

// Re-export allocator types
pub use allocator::{FreePageHeader, PageAllocator, PageState};

// Re-export edge compatibility types
pub use edge_compat::{Direction as EdgeDirection, PageType, V3EdgeCluster, V3EdgeStore};

// Re-export file coordinator for coordinated I/O
pub use file_coordinator::FileCoordinator;

// Re-export KV store types
pub use kv_store::{KvEntry, KvMetadata, KvStore, KvValue, hash_key as kv_hash_key};

// Re-export pub/sub types
pub use pubsub::{PubSubEvent, PubSubEventType, Publisher, SubscriberId, SubscriptionFilter};

// Re-export WAL types
pub use wal::{V3WALRecord, WALRecovery, WALRecoveryStats, WALWriter};

// Re-export kind index
pub use kind_index::KindIndex;

// Re-export V3Backend
pub use backend::V3Backend;

// Re-export WriteBatch
pub use write_batch::WriteBatch;

// Performance improvements (Task 1: LRU cache)
pub use node::cache::NodeCache;
// Task 5: Parallel BFS algorithm
pub use algorithm::{BfsConfig, parallel_bfs};
// Task 4: Storage module
pub use storage::{AdaptivePageManager, MediaDetector, MediaType, PageConfig};

// Algorithm integration tests
#[cfg(test)]
mod tests;

// Lazy initialization tests
#[cfg(test)]
mod lazy_init_tests;
