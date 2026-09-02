//! Forensic instrumentation for V3 backend performance analysis
//!
//! This module provides atomic counters for tracking internal operations
//! to identify performance amplification issues.
//!
//! # Usage
//!
//! Enable with the `v3-forensics` feature flag:
//! ```toml
//! [features]
//! v3-forensics = []
//! ```
//!
//! # Counters Tracked
//!
//! ## Write Path
//! - `logical_insert_node_calls`: Number of insert_node API calls
//! - `logical_insert_edge_calls`: Number of insert_edge API calls
//! - `btree_insert_calls`: Number of B+Tree insert operations
//! - `btree_split_count`: Number of B+Tree page splits
//! - `page_allocate_count`: Number of page allocations
//! - `page_write_count`: Total number of page write operations
//! - `wal_append_count`: Number of WAL records appended
//! - `wal_flush_count`: Number of WAL flush operations
//! - `node_encode_count`: Number of node encodings
//! - `edge_encode_count`: Number of edge encodings
//! - `sync_data_count`: Number of sync_data() calls
//! - `sync_all_count`: Number of sync_all() calls
//!
//! ## Read Path
//! - `logical_get_node_calls`: Number of get_node API calls
//! - `logical_neighbors_calls`: Number of neighbors API calls
//! - `btree_lookup_calls`: Number of B+Tree lookup operations
//! - `page_read_count`: Total number of page read operations
//! - `node_decode_count`: Number of node decodings
//! - `edge_decode_count`: Number of edge decodings
//!
//! # Phase 3: Page Ownership Tracking
//!
//! This module now includes a page ownership registry that tracks:
//! - Which subsystem allocated each page first
//! - All writes to each page (page_id, page_type, subsystem, sequence)
//! - Page type mismatches (e.g., B+Tree writing to a node page)
//! - Conflicting writes from different subsystems to the same page
//!
//! The page types tracked are:
//! - `NODE`: NodePage (node data storage)
//! - `BTREE`: IndexPage (B+Tree index page)
//! - `EDGE`: Edge cluster page
//! - `HEADER`: Persistent header
//! - `UNKNOWN`: Unidentified page type

use std::sync::atomic::{AtomicU64, Ordering};

mod ownership;
mod snapshot_support;
pub use ownership::{
    ConflictType, PAGE_OWNERSHIP, PageConflict, PageCorruption, PageOwnershipRecord,
    PageOwnershipRegistry, PageScanReport, PageType, Subsystem, get_first_conflict_page,
    get_page_registry, has_page_conflicts, print_page_ownership_map, print_page_ownership_report,
    reset_page_ownership, scan_database_pages,
};
pub use snapshot_support::{ForensicDelta, ForensicSnapshot};

/// Global forensic counters
pub static FORENSIC_COUNTERS: ForensicCounters = ForensicCounters {
    // Write path
    logical_insert_node_calls: AtomicU64::new(0),
    logical_insert_edge_calls: AtomicU64::new(0),
    btree_insert_calls: AtomicU64::new(0),
    btree_split_count: AtomicU64::new(0),
    page_allocate_count: AtomicU64::new(0),
    page_write_count: AtomicU64::new(0),
    wal_append_count: AtomicU64::new(0),
    wal_flush_count: AtomicU64::new(0),
    checkpoint_count: AtomicU64::new(0),
    node_encode_count: AtomicU64::new(0),
    edge_encode_count: AtomicU64::new(0),
    sync_data_count: AtomicU64::new(0),
    sync_all_count: AtomicU64::new(0),

    // Read path
    logical_get_node_calls: AtomicU64::new(0),
    logical_neighbors_calls: AtomicU64::new(0),
    btree_lookup_calls: AtomicU64::new(0),
    page_read_count: AtomicU64::new(0),
    node_decode_count: AtomicU64::new(0),
    edge_decode_count: AtomicU64::new(0),

    // Cache performance
    btree_cache_hit_count: AtomicU64::new(0),
    btree_cache_miss_count: AtomicU64::new(0),
    node_cache_hit_count: AtomicU64::new(0),
    node_cache_miss_count: AtomicU64::new(0),

    // Lock contention
    btree_read_lock_count: AtomicU64::new(0),
    btree_write_lock_count: AtomicU64::new(0),

    // Phase 2: Enhanced cache residency tracking
    dirty_page_hit_count: AtomicU64::new(0),
    node_page_cache_hit_count: AtomicU64::new(0),
    node_page_cache_miss_count: AtomicU64::new(0),
    redundant_page_reload_count: AtomicU64::new(0),

    // Phase 2: Edge path visibility
    edge_cache_hit_count: AtomicU64::new(0),
    edge_cache_miss_count: AtomicU64::new(0),
    edge_page_read_count: AtomicU64::new(0),

    // Phase 2: Page-ID tracing (for operation-scoped analysis)
    last_btree_pages_read: AtomicU64::new(0), // Bitmask of pages read in last op
    last_node_pages_read: AtomicU64::new(0),  // Bitmask of pages read in last op

    // Phase 3: Node unpack cost breakdown
    node_page_unpack_count: AtomicU64::new(0), // Number of NodePage::unpack() calls
    node_linear_scan_steps: AtomicU64::new(0), // Total nodes scanned during unpack (O(n) search)
    btree_traversal_depth_total: AtomicU64::new(0), // Total B+Tree levels traversed
};

/// Forensic counter structure
pub struct ForensicCounters {
    // Write path counters
    pub logical_insert_node_calls: AtomicU64,
    pub logical_insert_edge_calls: AtomicU64,
    pub btree_insert_calls: AtomicU64,
    pub btree_split_count: AtomicU64,
    pub page_allocate_count: AtomicU64,
    pub page_write_count: AtomicU64,
    pub wal_append_count: AtomicU64,
    pub wal_flush_count: AtomicU64,
    pub checkpoint_count: AtomicU64,
    pub node_encode_count: AtomicU64,
    pub edge_encode_count: AtomicU64,
    pub sync_data_count: AtomicU64,
    pub sync_all_count: AtomicU64,

    // Read path counters
    pub logical_get_node_calls: AtomicU64,
    pub logical_neighbors_calls: AtomicU64,
    pub btree_lookup_calls: AtomicU64,
    pub page_read_count: AtomicU64,
    pub node_decode_count: AtomicU64,
    pub edge_decode_count: AtomicU64,

    // Cache performance counters
    pub btree_cache_hit_count: AtomicU64,
    pub btree_cache_miss_count: AtomicU64,
    pub node_cache_hit_count: AtomicU64,
    pub node_cache_miss_count: AtomicU64,

    // Lock contention counters
    pub btree_read_lock_count: AtomicU64,
    pub btree_write_lock_count: AtomicU64,

    // Phase 2: Enhanced cache residency tracking
    pub dirty_page_hit_count: AtomicU64, // Hits on dirty_pages (no I/O)
    pub node_page_cache_hit_count: AtomicU64, // Node page cache hits (no I/O)
    pub node_page_cache_miss_count: AtomicU64, // Node page cache misses (disk I/O)
    pub redundant_page_reload_count: AtomicU64, // Page re-read in same logical op

    // Phase 2: Edge path visibility
    pub edge_cache_hit_count: AtomicU64, // Edge in-memory cache hits
    pub edge_cache_miss_count: AtomicU64, // Edge in-memory cache misses
    pub edge_page_read_count: AtomicU64, // Edge disk page reads

    // Phase 2: Page-ID tracing (for operation-scoped analysis)
    pub last_btree_pages_read: AtomicU64, // Bitmask of pages read in last op
    pub last_node_pages_read: AtomicU64,  // Bitmask of pages read in last op

    // Phase 3: Node unpack cost breakdown
    pub node_page_unpack_count: AtomicU64, // Number of NodePage::unpack() calls
    pub node_linear_scan_steps: AtomicU64, // Total nodes scanned during unpack (O(n) search)
    pub btree_traversal_depth_total: AtomicU64, // Total B+Tree levels traversed
}

impl ForensicCounters {
    /// Reset all counters to zero
    pub fn reset(&self) {
        // Write path
        self.logical_insert_node_calls.store(0, Ordering::Relaxed);
        self.logical_insert_edge_calls.store(0, Ordering::Relaxed);
        self.btree_insert_calls.store(0, Ordering::Relaxed);
        self.btree_split_count.store(0, Ordering::Relaxed);
        self.page_allocate_count.store(0, Ordering::Relaxed);
        self.page_write_count.store(0, Ordering::Relaxed);
        self.wal_append_count.store(0, Ordering::Relaxed);
        self.wal_flush_count.store(0, Ordering::Relaxed);
        self.checkpoint_count.store(0, Ordering::Relaxed);
        self.node_encode_count.store(0, Ordering::Relaxed);
        self.edge_encode_count.store(0, Ordering::Relaxed);
        self.sync_data_count.store(0, Ordering::Relaxed);
        self.sync_all_count.store(0, Ordering::Relaxed);

        // Read path
        self.logical_get_node_calls.store(0, Ordering::Relaxed);
        self.logical_neighbors_calls.store(0, Ordering::Relaxed);
        self.btree_lookup_calls.store(0, Ordering::Relaxed);
        self.page_read_count.store(0, Ordering::Relaxed);
        self.node_decode_count.store(0, Ordering::Relaxed);
        self.edge_decode_count.store(0, Ordering::Relaxed);

        // Cache performance
        self.btree_cache_hit_count.store(0, Ordering::Relaxed);
        self.btree_cache_miss_count.store(0, Ordering::Relaxed);
        self.node_cache_hit_count.store(0, Ordering::Relaxed);
        self.node_cache_miss_count.store(0, Ordering::Relaxed);

        // Lock contention
        self.btree_read_lock_count.store(0, Ordering::Relaxed);
        self.btree_write_lock_count.store(0, Ordering::Relaxed);

        // Phase 2: Enhanced tracking
        self.dirty_page_hit_count.store(0, Ordering::Relaxed);
        self.node_page_cache_hit_count.store(0, Ordering::Relaxed);
        self.node_page_cache_miss_count.store(0, Ordering::Relaxed);
        self.redundant_page_reload_count.store(0, Ordering::Relaxed);
        self.edge_cache_hit_count.store(0, Ordering::Relaxed);
        self.edge_cache_miss_count.store(0, Ordering::Relaxed);
        self.edge_page_read_count.store(0, Ordering::Relaxed);
        self.last_btree_pages_read.store(0, Ordering::Relaxed);
        self.last_node_pages_read.store(0, Ordering::Relaxed);

        // Phase 3: Node unpack cost breakdown
        self.node_page_unpack_count.store(0, Ordering::Relaxed);
        self.node_linear_scan_steps.store(0, Ordering::Relaxed);
        self.btree_traversal_depth_total.store(0, Ordering::Relaxed);
    }
}
