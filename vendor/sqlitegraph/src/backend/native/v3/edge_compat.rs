//! V3 Edge Compatibility Layer
//!
//! This module provides a compatibility layer for using V2 EdgeCluster format
//! within V3's page-based storage system. This compat layer bridges
//! edge layout until NodeStore/B+Tree/allocator/WAL are fully settled.
//! NodeStore/B+Tree/allocator/WAL are still settling.
//!
//! # Design Principles
//!
//! 1. **Logical NodeIDs only**: EdgeCluster references NodeID, not V2 slot assumptions
//!    Resolution is via B+Tree → page.
//!
//! 2. **V3 pages + allocator**: Edge storage lives in V3 pages allocated by V3 allocator.
//!    Only the record format is reused from V2.
//!
//! 3. **Separate PageType**: Edges get their own PageType::EDGE_CLUSTER.
//!    Node pages never embed edge blobs.
//!
//! 4. **WAL-first**: Write path is WAL'd (insert_edge/delete_edge/update adjacency)
//!    before any compaction/relocation.
//!
//! # Edge Type Storage Model
//!
//! ## Durable Storage
//! Edge types are stored durably in the edge_data field of CompactEdgeRecord using
//! an inline encoding format: `[type_len: u8][type_bytes]`. This ensures edge types
//! survive reopen/recovery.
//!
//! ## In-Memory Index
//! The `edge_types: HashMap<(src, dst, dir), String>` field provides O(1) filtering.
//! This HashMap is rebuilt from durable storage on cache miss via `load_neighbors_from_disk()`.
//!
//! ## SEMANTIC CONSTRAINT (Known Limitation)
//!
//! The edge_types HashMap is keyed by `(src, dst, dir)`, NOT by edge_id. This means:
//!
//! - **Only ONE edge type can exist between a given (src, dst, dir) tuple**
//! - Inserting a second edge between same endpoints with a different type OVERWRITES the previous type
//! - This is intentional for V3's simple tuple-key model
//! - If multi-edge support (same endpoints, different types) is needed, the key model must change to use edge_id
//!
//! Example of the aliasing behavior:
//! ```ignore
//! insert_edge(1, 2, Outgoing, "CALLS")  // edge_types: {(1,2,Out) -> "CALLS"}
//! insert_edge(1, 2, Outgoing, "USES")   // edge_types: {(1,2,Out) -> "USES"} ← OVERWRITES!
//! ```
//!
//! # Architecture
//!
//! ```text
//! EdgeCluster { src: NodeId, dsts: Vec<NodeId>, dir: Out|In, metadata }
//!
//! B+Tree index: key = (src, dir) → value = edge_page_id
//!
//! Neighbor query: lookup_edge_page(src) → decode cluster → return iterator
//!
//! Insert edge: load cluster (or create), append, maybe split if page full
//! ```

#![allow(
    clippy::type_complexity,
    clippy::collapsible_if,
    reason = "V3 cache types are complex by design; collapsed let-chains reduce readability"
)]
#[path = "edge_compat/core_support.rs"]
mod core_support;
#[path = "edge_cluster_support.rs"]
mod edge_cluster_support;
#[path = "edge_disk_support.rs"]
mod edge_disk_support;
#[path = "edge_mutation_support.rs"]
mod edge_mutation_support;
#[path = "edge_query_support.rs"]
mod edge_query_support;
#[cfg(feature = "v3-forensics")]
use crate::backend::native::v3::forensics::{
    FORENSIC_COUNTERS, PageType as ForensicPageType, Subsystem,
};
use crate::backend::native::v3::{
    allocator::PageAllocator, btree::BTreeManager, file_coordinator::FileCoordinator,
    header::PersistentHeaderV3, wal::WALWriter,
};
use crate::backend::native::{
    types::{NativeBackendError, NativeResult},
    v3::compact_edge_record::Direction as V2Direction,
};
pub(crate) use core_support::edge_key;
pub use core_support::{Direction, PageType};
pub use edge_cluster_support::V3EdgeCluster;
pub(crate) use edge_cluster_support::{decode_edge_cluster_page_header, decode_packed_edge_page};
use edge_cluster_support::{encode_edge_cluster_pages, encode_packed_edge_page};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

pub(crate) use core_support::{
    EDGE_CLUSTER_PAGE_HEADER_SIZE, EDGE_CLUSTER_PAGE_MAGIC, PACKED_EDGE_PAGE_HEADER_SIZE,
    PACKED_EDGE_PAGE_MAGIC, PACKED_EDGE_PAGE_SLOT_SIZE,
};

pub struct V3EdgeStore {
    /// B+Tree index: (src, dir) → page_id
    #[cfg(test)]
    pub btree: Arc<parking_lot::RwLock<BTreeManager>>,
    #[cfg(not(test))]
    btree: Arc<parking_lot::RwLock<BTreeManager>>,
    /// Optional WAL writer for durability (shared with V3Backend via Arc)
    #[cfg(test)]
    pub wal: Option<Arc<RwLock<WALWriter>>>,
    #[cfg(not(test))]
    wal: Option<Arc<RwLock<WALWriter>>>,
    /// In-memory cache of neighbor lists - using Arc<[i64]> for zero-copy reads
    /// This matches SQLite's AdjacencyCache pattern but with Arc for zero-copy
    cache: Arc<RwLock<HashMap<(i64, Direction), Arc<[i64]>>>>,
    /// In-memory cache of weighted neighbors
    cache_weighted: Arc<RwLock<HashMap<(i64, Direction), Arc<[(i64, f32)]>>>>,
    /// Edge type storage: (src, dst, dir) -> edge_type string
    ///
    /// SEMANTIC CONSTRAINT: Key is (src, dst, dir), NOT edge_id.
    /// This means only ONE edge type can exist between a given (src, dst, dir) tuple.
    /// Inserting multiple edges between same endpoints with different types will
    /// cause aliasing - the last type wins. See module-level docs for details.
    ///
    /// This HashMap is rebuilt from durable edge_data on cache miss via
    /// load_neighbors_from_disk(), ensuring edge types survive reopen/recovery.
    edge_types: Arc<RwLock<HashMap<(i64, i64, Direction), String>>>,
    /// Performance counters
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    /// Hit time accumulator (nanoseconds) - for profiling
    hit_time_ns: AtomicU64,
    /// Miss time accumulator (nanoseconds) - for profiling
    miss_time_ns: AtomicU64,
    /// Track dirty clusters that need to be flushed
    #[cfg(test)]
    pub dirty_clusters: RwLock<HashMap<(i64, Direction), V3EdgeCluster>>,
    #[cfg(not(test))]
    dirty_clusters: RwLock<HashMap<(i64, Direction), V3EdgeCluster>>,
    /// Path to database file for writing pages
    db_path: Option<PathBuf>,
    /// Page allocator for edge page allocation
    /// CRITICAL: Shared with NodeStore to prevent page ID collisions
    allocator: Arc<RwLock<PageAllocator>>,
    /// Page size for I/O operations (detected from storage media)
    page_size: u32,
    /// Coordinated file handle for all main DB I/O (optional for backward compat)
    /// When set, all file writes go through this coordinator to prevent race conditions
    file_coordinator: Option<Arc<FileCoordinator>>,
}

#[cfg(test)]
#[path = "edge_compat_tests.rs"]
mod tests;
