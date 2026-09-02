//! V3Backend - Native V3 GraphBackend implementation
//!
//! This module implements the GraphBackend trait for V3 storage format with:
//! - B+Tree node index for unlimited capacity
//! - Page-based node storage
//! - Delta/varint compression
//! - Page allocator for dynamic page allocation
//! - Write-Ahead Logging for crash recovery
//!
//! ## Architecture
//!
//! ```text
//! V3Backend {
//!     db_path: PathBuf,           // Database file path
//!     btree: RwLock<BTreeManager>, // B+Tree for node_id → page_id
//!     node_store: RwLock<NodeStore>, // Node storage operations
//!     edge_store: RwLock<V3EdgeStore>, // Edge storage (compat layer)
//!     allocator: RwLock<PageAllocator>, // Page allocation
//!     wal: Option<RwLock<WALWriter>>, // Optional WAL for durability
//!     header: RwLock<PersistentHeaderV3>, // Persistent header
//! }
//! ```

// Note: map_v3_error is defined in graph_validation but not exported.
// We'll define a local mapping function for V3 errors.
#![allow(
    clippy::manual_async_fn,
    reason = "AsyncGraphBackend trait requires impl Future"
)]
#[path = "async_backend.rs"]
mod async_backend;
#[path = "batch_guard.rs"]
mod batch_guard;
#[path = "chain_support.rs"]
mod chain_support;
#[path = "csr_support.rs"]
mod csr_support;
#[path = "entity_support.rs"]
mod entity_support;
#[path = "hnsw_support.rs"]
mod hnsw_support;
#[path = "index_rebuild.rs"]
mod index_rebuild;
#[path = "kv_pubsub_support.rs"]
mod kv_pubsub_support;
#[path = "lifecycle_support.rs"]
mod lifecycle_support;
#[path = "maintenance_support.rs"]
mod maintenance_support;
#[path = "mutation_support.rs"]
mod mutation_support;
#[path = "neighbor_support.rs"]
mod neighbor_support;
#[path = "pattern_support.rs"]
mod pattern_support;
#[path = "property_support.rs"]
mod property_support;
#[path = "query_support.rs"]
mod query_support;
#[path = "read_support.rs"]
mod read_support;
#[path = "snapshot_io_support.rs"]
mod snapshot_io_support;
#[path = "snapshot_meta.rs"]
mod snapshot_meta;
#[path = "sql_exec.rs"]
mod sql_exec;
#[path = "sql_value.rs"]
mod sql_value;
#[path = "sqlite_schema.rs"]
mod sqlite_schema;
#[path = "transaction_control.rs"]
mod transaction_control;
#[path = "transaction_guard.rs"]
mod transaction_guard;
#[path = "transaction_state.rs"]
mod transaction_state;
#[path = "traversal_support.rs"]
mod traversal_support;

use crate::SqliteGraphError;
use crate::backend::native::types::NativeBackendError;
use crate::backend::native::v3::btree::BTreeManager;
use crate::backend::native::v3::edge_compat::Direction as EdgeDirection;
use crate::backend::native::v3::name_index::NameIndex;
use crate::backend::native::v3::wal::WALWriter;
use crate::backend::native::v3::{
    AsyncFileCoordinator, FileCoordinator, KindIndex, KvStore, NodeCache, NodeStore, PageAllocator,
    PersistentHeaderV3, Publisher, V3EdgeStore,
};
use crate::backend::{
    BackendDirection, ChainStep, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec, PatternMatch,
    PatternQuery,
};
use crate::graph::GraphEntity;
use crate::snapshot::SnapshotId;
pub use batch_guard::WriteBatchGuard;
use hnsw_support::HnswIndexMetadata;
pub use hnsw_support::HnswSearchConfig;
use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;
pub use sql_value::{SqlRow, SqlValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
pub use transaction_guard::{V3SavepointGuard, V3TransactionGuard};
use transaction_state::GraphTransactionState;

/// V3 Backend implementation with interior mutability
///
/// This struct implements the GraphBackend trait using V3's page-based
/// storage with B+Tree indexing for O(log n) node lookups.
///
/// ## Lazy Initialization
///
/// The KV store and Pub/Sub publisher are lazily initialized:
/// - `kv_store`: Created on first KV operation (get/set/delete)
/// - `publisher`: Created on first subscription
///
/// This reduces memory overhead for use cases that only need graph operations.
pub struct V3Backend {
    /// Database file path
    db_path: PathBuf,
    /// Shared file coordinator — persistent file handle pool that eliminates
    /// open/seek/read/close per cache miss on NodeStore, BTreeManager, and
    /// V3EdgeStore. All three subsystems share the SAME coordinator instance.
    file_coordinator: Option<Arc<FileCoordinator>>,
    /// Async coordinator for non-blocking I/O
    async_coordinator: std::sync::OnceLock<Arc<AsyncFileCoordinator>>,
    /// SQLite connection for property storage (SQL Layer)
    sqlite_conn: Arc<Mutex<Connection>>,
    /// BTreeManager for node_id → page_id lookups
    btree: RwLock<BTreeManager>,
    /// NodeStore for node operations
    node_store: RwLock<NodeStore>,
    /// EdgeStore for edge operations (compat layer)
    edge_store: RwLock<V3EdgeStore>,
    /// Page allocator for dynamic page allocation (shared between BTreeManager and NodeStore)
    allocator: Arc<RwLock<PageAllocator>>,
    /// Optional WAL writer for durability (shared with edge store via Arc)
    wal: Option<Arc<RwLock<WALWriter>>>,
    /// Persistent header
    header: RwLock<PersistentHeaderV3>,
    /// KV store for key-value operations (lazy initialized)
    kv_store: RwLock<Option<KvStore>>,
    /// Pub/Sub publisher for event notification (lazy initialized)
    publisher: RwLock<Option<Publisher>>,
    /// Kind index for O(1) kind-based lookups
    kind_index: KindIndex,
    /// Name index for fast name lookups (exact, prefix, substring)
    name_index: NameIndex,
    /// LRU cache for node records (2-3× point lookup improvement)
    node_cache: NodeCache,
    /// Current snapshot version (LSN-based)
    current_snapshot_version: RwLock<u64>,
    /// Snapshot metadata registry (name -> LSN mapping)
    snapshot_registry: RwLock<HashMap<String, u64>>,
    /// HNSW indexes for vector similarity search with turbovec metadata
    /// Each index tracks embedding count and turbovec state for automatic compression
    hnsw_indexes: RwLock<HashMap<String, HnswIndexMetadata>>,
    /// Native graph transaction/savepoint state for rollback parity with SQLite.
    graph_tx_state: Mutex<GraphTransactionState>,
}

impl Drop for V3Backend {
    fn drop(&mut self) {
        // Flush any pending data to disk
        // This ensures data is persisted when the backend is dropped.
        // Errors here are silent because we cannot panic in Drop, but we
        // surface them so callers don't silently lose the last transaction
        // (e.g. on disk-full / read-only-FS).
        if let Err(e) = self.flush_to_disk() {
            eprintln!("[sqlitegraph] V3Backend::drop: flush_to_disk failed: {e}");
        }

        // Sync header to ensure all metadata is written
        if let Err(e) = self.sync_header() {
            eprintln!("[sqlitegraph] V3Backend::drop: sync_header failed: {e}");
        }
    }
}

/// Map NativeBackendError to SqliteGraphError
fn map_v3_error(err: NativeBackendError) -> SqliteGraphError {
    match err {
        NativeBackendError::Io(e) => SqliteGraphError::connection(e.to_string()),
        NativeBackendError::SerializationError { context } => {
            SqliteGraphError::connection(format!("Serialization error: {}", context))
        }
        NativeBackendError::DeserializationError { context } => {
            SqliteGraphError::connection(format!("Deserialization error: {}", context))
        }
        NativeBackendError::InvalidNodeId { id, max_id } => {
            SqliteGraphError::query(format!("Invalid node ID: {} (max: {})", id, max_id))
        }
        NativeBackendError::InvalidEdgeId { id, max_id } => {
            SqliteGraphError::query(format!("Invalid edge ID: {} (max: {})", id, max_id))
        }
        NativeBackendError::CorruptNodeRecord { node_id, reason } => {
            SqliteGraphError::connection(format!("Corrupt node record {}: {}", node_id, reason))
        }
        NativeBackendError::CorruptEdgeRecord { edge_id, reason } => {
            SqliteGraphError::connection(format!("Corrupt edge record {}: {}", edge_id, reason))
        }
        NativeBackendError::InvalidMagic { expected, found } => SqliteGraphError::connection(
            format!("Invalid magic: expected {}, found {}", expected, found),
        ),
        NativeBackendError::UnsupportedVersion {
            version,
            supported_version,
        } => SqliteGraphError::connection(format!(
            "Unsupported version: {} (supported: {})",
            version, supported_version
        )),
        NativeBackendError::InvalidHeader { field, reason } => {
            SqliteGraphError::connection(format!("Invalid header field '{}': {}", field, reason))
        }
        NativeBackendError::InvalidChecksum { expected, found } => SqliteGraphError::connection(
            format!("Checksum mismatch: expected {}, found {}", expected, found),
        ),
        NativeBackendError::RecordTooLarge { size, max_size } => {
            SqliteGraphError::connection(format!("Record too large: {} (max: {})", size, max_size))
        }
        NativeBackendError::BincodeError(e) => {
            SqliteGraphError::connection(format!("Bincode error: {}", e))
        }
        _ => SqliteGraphError::connection(format!("Native backend error: {:?}", err)),
    }
}

impl V3Backend {
    /// Returns true if the shared FileCoordinator is wired into this backend.
    ///
    /// When true, cold-path reads reuse a persistent file handle instead of
    /// doing open/seek/read/close per cache miss. Used by tests to verify
    /// the coordinator wiring in open() and create().
    pub fn has_file_coordinator(&self) -> bool {
        self.file_coordinator.is_some()
    }

    /// Page-cache capacity (pages). Instance method delegating to NodeStore.
    pub fn node_page_cache_capacity(&self) -> usize {
        self.node_store.read().cache_capacity()
    }

    /// Check whether the page containing `node_id` is resident in the page
    /// cache. Performs a btree lookup to resolve node_id to page_id, then
    /// peeks the cache (no recency update). Used by LRU eviction tests.
    pub fn node_page_cache_resident_for(&self, node_id: i64) -> bool {
        let node_store = self.node_store.read();
        // lookup_node may populate the cache, so we use a peek-based check
        // on the index. The btree tells us the page_id for this node.
        if let Ok(Some(page_id)) = node_store.page_id_for_node(node_id) {
            return node_store.page_cache_contains(page_id);
        }
        false
    }

    /// Number of entries in the unpacked-page cache. Used by double-decode tests.
    pub fn node_unpacked_cache_len(&self) -> usize {
        self.node_store.read().unpacked_page_cache_len()
    }

    /// Clear all node page caches. Used by tests to start from a known state.
    pub fn clear_node_page_caches(&self) {
        self.node_store.write().clear_all_caches();
    }
}

impl GraphBackend for V3Backend {
    /// Check if snapshot is supported by V3 backend.
    ///
    /// V3 only supports SnapshotId::current() (which returns 0).
    /// Any historical/non-zero snapshot ID is rejected with a clear error.
    fn insert_node(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        self.insert_node_support(node)
    }

    fn insert_edge(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        self.insert_edge_support(edge)
    }

    fn update_node(&self, node_id: i64, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        self.update_node_support(node_id, node)
    }

    fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError> {
        self.delete_entity_support(id)
    }

    fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.entity_ids_support()
    }

    fn get_node(&self, snapshot_id: SnapshotId, id: i64) -> Result<GraphEntity, SqliteGraphError> {
        self.get_node_support(snapshot_id, id)
    }

    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        Ok(self.neighbors_shared(snapshot_id, node, query)?.to_vec())
    }

    fn bfs(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.bfs_traversal(snapshot_id, start, depth)
    }

    fn shortest_path(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.shortest_path_traversal(snapshot_id, start, end)
    }

    fn node_degree(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        self.node_degree_counts(snapshot_id, node)
    }

    fn k_hop(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.k_hop_traversal(snapshot_id, start, depth, direction)
    }

    fn k_hop_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.k_hop_filtered_traversal(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn bfs_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.bfs_filtered_traversal(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn shortest_path_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
        allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.shortest_path_filtered_traversal(snapshot_id, start, end, allowed_edge_types)
    }

    fn chain_query(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        chain: &[ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.chain_query_steps(snapshot_id, start, chain)
    }

    fn pattern_search(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        self.pattern_search_matches(snapshot_id, start, pattern)
    }

    fn checkpoint(&self) -> Result<(), SqliteGraphError> {
        self.checkpoint_support()
    }

    fn flush(&self) -> Result<(), SqliteGraphError> {
        self.flush_to_disk()
    }

    fn backup(&self, backup_dir: &Path) -> Result<crate::backend::BackupResult, SqliteGraphError> {
        self.backup_support(backup_dir)
    }

    fn snapshot_export(
        &self,
        export_dir: &Path,
    ) -> Result<crate::backend::SnapshotMetadata, SqliteGraphError> {
        self.snapshot_export_support(export_dir)
    }

    fn snapshot_import(
        &self,
        import_dir: &Path,
    ) -> Result<crate::backend::ImportMetadata, SqliteGraphError> {
        self.snapshot_import_support(import_dir)
    }

    fn query_nodes_by_kind(
        &self,
        snapshot_id: SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.query_nodes_by_kind_support(snapshot_id, kind)
    }

    fn query_nodes_by_name_pattern(
        &self,
        snapshot_id: SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.query_nodes_by_name_pattern_support(snapshot_id, pattern)
    }

    fn kv_get(
        &self,
        snapshot_id: SnapshotId,
        key: &[u8],
    ) -> Result<Option<crate::backend::native::types::KvValue>, SqliteGraphError> {
        self.kv_get_v2(snapshot_id, key)
    }

    fn kv_set(
        &self,
        key: Vec<u8>,
        value: crate::backend::native::types::KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), SqliteGraphError> {
        self.kv_set_v2(key, value, ttl_seconds)
    }

    fn kv_delete(&self, key: &[u8]) -> Result<(), SqliteGraphError> {
        self.kv_delete_v2(key)
    }

    fn subscribe(
        &self,
        filter: crate::backend::SubscriptionFilter,
    ) -> Result<(u64, std::sync::mpsc::Receiver<crate::backend::PubSubEvent>), SqliteGraphError>
    {
        self.subscribe_pubsub(filter)
    }

    fn unsubscribe(&self, subscriber_id: u64) -> Result<bool, SqliteGraphError> {
        self.unsubscribe_pubsub(subscriber_id)
    }

    fn kv_prefix_scan(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, crate::backend::native::types::KvValue)>, SqliteGraphError> {
        self.kv_prefix_scan_v2(snapshot_id, prefix)
    }

    fn match_triples(
        &self,
        pattern: &crate::pattern_engine::PatternTriple,
    ) -> Result<Vec<crate::pattern_engine::TripleMatch>, crate::SqliteGraphError> {
        self.match_triples_support(pattern)
    }

    fn vector_search(
        &self,
        _snapshot: crate::snapshot::SnapshotId,
        index_name: &str,
        vector: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, crate::SqliteGraphError> {
        self.hnsw_vector_search(index_name, vector, k)
    }

    fn get_graph_ref(&self) -> Option<&crate::graph::SqliteGraph> {
        None
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
