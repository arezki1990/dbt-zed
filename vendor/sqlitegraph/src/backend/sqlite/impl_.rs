//! SQLite backend implementation for GraphBackend trait.
//!
//! This module contains the SqliteGraphBackend implementation that wraps
//! SqliteGraph and provides the concrete implementation of all GraphBackend methods.

use parking_lot::RwLock;
use rusqlite::params;
use std::sync::Arc;

use crate::{
    SqliteGraphError,
    backend::sqlite::types::{BackendDirection, EdgeSpec, NeighborQuery, NodeSpec},
    backend::{PubSubEvent, SubscriptionFilter},
    bfs::{bfs_neighbors, bfs_neighbors_filtered, shortest_path, shortest_path_filtered},
    graph::{GraphEdge, GraphEntity, SqliteGraph},
    multi_hop,
    pattern::{self, PatternMatch, PatternQuery},
};

/// Simple in-memory publisher for SQLite backend pub/sub
///
/// Similar to V3's Publisher but simplified for SQLite backend use
struct Publisher {
    subscribers: RwLock<
        Vec<(
            u64,
            std::sync::mpsc::Sender<PubSubEvent>,
            SubscriptionFilter,
        )>,
    >,
    next_id: std::sync::atomic::AtomicU64,
}

impl Publisher {
    fn new() -> Self {
        Self {
            subscribers: RwLock::new(Vec::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn subscribe(
        &self,
        filter: SubscriptionFilter,
    ) -> (u64, std::sync::mpsc::Receiver<PubSubEvent>) {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();

        self.subscribers.write().push((id, tx, filter));
        (id, rx)
    }

    fn unsubscribe(&self, subscriber_id: u64) -> bool {
        let mut subs = self.subscribers.write();
        if let Some(pos) = subs.iter().position(|(id, _, _)| *id == subscriber_id) {
            subs.remove(pos);
            true
        } else {
            false
        }
    }

    fn emit(&self, event: PubSubEvent) {
        let subs = self.subscribers.read();
        for (_, sender, filter) in subs.iter() {
            if Self::should_send(&event, filter) {
                // Best-effort delivery - ignore send failures
                let _ = sender.send(event.clone());
            }
        }
    }

    fn should_send(event: &PubSubEvent, filter: &SubscriptionFilter) -> bool {
        match event {
            PubSubEvent::NodeChanged { .. } => filter.node_changes,
            PubSubEvent::EdgeChanged { .. } => filter.edge_changes,
            PubSubEvent::KvChanged { .. } => filter.kv_changes,
            PubSubEvent::SnapshotCommitted { .. } => filter.snapshot_commits,
        }
    }
}

/// Validate snapshot_id for SQLite backend
///
/// SQLite backend does not support historical snapshot isolation.
/// Only SnapshotId(0) is supported, which is returned by SnapshotId::current().
///
/// # Snapshot ID Contract
///
/// - `SnapshotId::current()` returns SnapshotId(0) - the "current" sentinel
/// - Any non-zero snapshot ID represents a historical snapshot (not supported)
///
/// Historical snapshot isolation would require:
/// - WAL-based versioning with timestamp/LSN indexing
/// - AS OF queries or point-in-time recovery mechanisms
/// - Multi-version concurrency control (MVCC) extensions
///
/// Resolved snapshot view for a read operation.
///
/// Returned by [`SqliteGraphBackend::resolve_snapshot`]. `Live` reads hit the
/// current graph + SQLite. `Historical` reads serve adjacency from the MVCC
/// version chain's immutable [`SnapshotState`].
#[derive(Debug, Clone)]
pub enum SnapshotView {
    /// Read the live graph (SnapshotId(0) — current committed state).
    Live,
    /// Read a historical version from the MVCC version chain.
    Historical(Arc<crate::mvcc::SnapshotState>),
}

impl SqliteGraphBackend {
    /// Resolve a [`SnapshotId`] into a [`SnapshotView`].
    ///
    /// - `SnapshotId(0)` → `Live` (unchanged behavior: reads the current graph).
    /// - `SnapshotId(N)` where N > 0 → `Historical` if version N exists in the
    ///   snapshot manager's bounded history chain; otherwise an error.
    /// - `SnapshotId::invalid()` → always an error.
    ///
    /// This replaces the old `validate_snapshot_for_sqlite` which accepted only
    /// `SnapshotId(0)` and rejected everything else. Now historical reads are
    /// served from the in-memory [`crate::mvcc::VersionedSnapshot`] chain.
    fn resolve_snapshot(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
    ) -> Result<SnapshotView, SqliteGraphError> {
        let lsn = snapshot_id.as_lsn();

        // Invalid sentinel.
        if lsn == u64::MAX {
            return Err(SqliteGraphError::query(
                "invalid snapshot id (u64::MAX sentinel)",
            ));
        }

        // Current / live.
        if lsn == 0 {
            return Ok(SnapshotView::Live);
        }

        // Historical: look up version N in the MVCC chain.
        let versioned = self.graph.snapshot_manager.as_of(lsn).ok_or_else(|| {
            SqliteGraphError::query(format!(
                "snapshot version {} is not available (never checkpointed or evicted by \
                     bounded retention; current range: {:?}–{:?})",
                lsn,
                self.graph.snapshot_manager.oldest_version(),
                self.graph.snapshot_manager.newest_version()
            ))
        })?;

        Ok(SnapshotView::Historical(versioned.state))
    }

    /// Require a live snapshot for methods that don't support historical reads.
    ///
    /// Adjacency-only methods (neighbors, bfs) serve historical reads from the
    /// MVCC version chain. Methods that need entity properties, typed edges, or
    /// key-value data can only read the live graph — this helper rejects
    /// historical snapshots with an honest error.
    fn require_live(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        method_name: &str,
    ) -> Result<(), SqliteGraphError> {
        match self.resolve_snapshot(snapshot_id)? {
            SnapshotView::Live => Ok(()),
            SnapshotView::Historical(_) => Err(SqliteGraphError::query(format!(
                "historical snapshots are not supported for {}() — only neighbors() and bfs() \
                 serve the MVCC version chain (adjacency-only). Use SnapshotId::current() for \
                 live reads.",
                method_name
            ))),
        }
    }

    /// Resolve a NeighborQuery against a historical [`SnapshotState`].
    ///
    /// The version chain stores untyped adjacency (`HashMap<i64, Vec<i64>>`),
    /// so typed-edge queries (`edge_type = Some(..)`) are not supported and
    /// return an error. Untyped directional queries are served directly.
    fn historical_neighbors(
        &self,
        state: &crate::mvcc::SnapshotState,
        node: i64,
        direction: BackendDirection,
        edge_type: &Option<String>,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if edge_type.is_some() {
            return Err(SqliteGraphError::query(
                "typed-edge queries are not supported for historical snapshots (the MVCC version \
                 chain stores untyped adjacency only)",
            ));
        }
        Ok(match direction {
            BackendDirection::Outgoing => state.get_outgoing(node).cloned().unwrap_or_default(),
            BackendDirection::Incoming => state.get_incoming(node).cloned().unwrap_or_default(),
        })
    }
}

/// SQLite-backed implementation of the GraphBackend trait.
///
/// This struct wraps a SqliteGraph instance and implements all GraphBackend methods
/// by delegating to the underlying SQLite-based graph operations.
pub struct SqliteGraphBackend {
    graph: SqliteGraph,
    /// In-memory publisher for pub/sub support (lazy initialized)
    publisher: RwLock<Option<Publisher>>,
}

impl SqliteGraphBackend {
    /// Create a new SQLite backend with an in-memory database.
    pub fn in_memory() -> Result<Self, SqliteGraphError> {
        Ok(Self {
            graph: SqliteGraph::open_in_memory()?,
            publisher: RwLock::new(None),
        })
    }

    /// Create a new SQLite backend from an existing SqliteGraph instance.
    pub fn from_graph(graph: SqliteGraph) -> Self {
        Self {
            graph,
            publisher: RwLock::new(None),
        }
    }

    /// Get a reference to the underlying SqliteGraph instance.
    pub fn graph(&self) -> &SqliteGraph {
        &self.graph
    }

    /// Create a new HNSW vector storage using this SQLite backend.
    ///
    /// **Always returns `None`.** `SQLiteVectorStorage` requires an owned
    /// `rusqlite::Connection`, but `SqliteGraphBackend` only holds a
    /// pooled reference. There is no safe way to mint a fresh owned
    /// connection from `&self` without exposing the underlying DB path.
    ///
    /// Use [`crate::hnsw::storage::SQLiteVectorStorage::new`] directly
    /// with a separately-opened `Connection` instead.
    ///
    /// This method is preserved for trait-object call sites and may
    /// gain a working implementation if `SqliteGraph` ever exposes its
    /// path (see issue tracker).
    ///
    /// # Arguments
    ///
    /// * `_index_name` - Currently ignored.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use sqlitegraph::hnsw::storage::SQLiteVectorStorage;
    /// let conn = rusqlite::Connection::open("vectors.db")?;
    /// let storage = SQLiteVectorStorage::new(conn, "my_index")?;
    /// ```
    pub fn create_hnsw_storage(
        &self,
        _index_name: impl Into<String>,
    ) -> Option<Box<dyn crate::hnsw::storage::VectorStorage>> {
        None
    }

    /// Get all entity IDs from the graph.
    pub fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.graph.all_entity_ids()
    }

    /// Ensure the kv_store table exists
    fn ensure_kv_table(&self) -> Result<(), SqliteGraphError> {
        let conn = self.graph.connection();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                ttl_seconds INTEGER,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create kv_store table: {}", e))
        })?;

        Ok(())
    }

    /// Execute optimized neighbor queries based on direction and edge type filtering.
    fn query_neighbors(
        &self,
        node: i64,
        direction: BackendDirection,
        edge_type: &Option<String>,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        match (direction, edge_type) {
            (BackendDirection::Outgoing, None) => self.graph.fetch_outgoing(node),
            (BackendDirection::Incoming, None) => self.graph.fetch_incoming(node),
            (BackendDirection::Outgoing, Some(edge_type)) => {
                let conn = self.graph.connection();
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT to_id FROM graph_edges WHERE from_id=?1 AND edge_type=?2 ORDER BY to_id, id",
                    )
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let rows = stmt
                    .query_map(params![node, edge_type], |row| row.get(0))
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let mut values = Vec::new();
                for value in rows {
                    values.push(value.map_err(|e| SqliteGraphError::query(e.to_string()))?);
                }
                Ok(values)
            }
            (BackendDirection::Incoming, Some(edge_type)) => {
                let conn = self.graph.connection();
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT from_id FROM graph_edges WHERE to_id=?1 AND edge_type=?2 ORDER BY from_id, id",
                    )
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let rows = stmt
                    .query_map(params![node, edge_type], |row| row.get(0))
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let mut values = Vec::new();
                for value in rows {
                    values.push(value.map_err(|e| SqliteGraphError::query(e.to_string()))?);
                }
                Ok(values)
            }
        }
    }
}

impl crate::backend::GraphBackend for SqliteGraphBackend {
    fn insert_node(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let id = self.graph.insert_entity(&GraphEntity {
            id: 0,
            kind: node.kind,
            name: node.name,
            file_path: node.file_path,
            data: node.data,
        })?;

        // Emit event if publisher is initialized
        let pub_guard = self.publisher.read();
        if let Some(ref publisher) = *pub_guard {
            publisher.emit(PubSubEvent::NodeChanged {
                node_id: id,
                snapshot_id: 0, // SQLite doesn't use snapshot IDs
            });
        }

        Ok(id)
    }

    fn get_node(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        id: i64,
    ) -> Result<GraphEntity, SqliteGraphError> {
        self.require_live(snapshot_id, "get_node")?;
        self.graph.get_entity(id)
    }

    fn insert_edge(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        let id = self.graph.insert_edge(&GraphEdge {
            id: 0,
            from_id: edge.from,
            to_id: edge.to,
            edge_type: edge.edge_type,
            data: edge.data,
        })?;

        // Emit event if publisher is initialized
        let pub_guard = self.publisher.read();
        if let Some(ref publisher) = *pub_guard {
            publisher.emit(PubSubEvent::EdgeChanged {
                from_node: edge.from,
                to_node: edge.to,
                edge_id: id,
                snapshot_id: 0, // SQLite doesn't use snapshot IDs
            });
        }

        Ok(id)
    }

    fn insert_nodes_bulk(&self, nodes: &[NodeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        let entities: Vec<GraphEntity> = nodes
            .iter()
            .map(|node| GraphEntity {
                id: 0,
                kind: node.kind.clone(),
                name: node.name.clone(),
                file_path: node.file_path.clone(),
                data: node.data.clone(),
            })
            .collect();
        let ids = self.graph.insert_entities_bulk(&entities)?;

        // Emit per-row events after the commit, matching single-insert
        // observer semantics.
        let pub_guard = self.publisher.read();
        if let Some(ref publisher) = *pub_guard {
            for id in &ids {
                publisher.emit(PubSubEvent::NodeChanged {
                    node_id: *id,
                    snapshot_id: 0,
                });
            }
        }

        Ok(ids)
    }

    fn insert_edges_bulk(&self, edges: &[EdgeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        let graph_edges: Vec<GraphEdge> = edges
            .iter()
            .map(|edge| GraphEdge {
                id: 0,
                from_id: edge.from,
                to_id: edge.to,
                edge_type: edge.edge_type.clone(),
                data: edge.data.clone(),
            })
            .collect();
        let ids = self.graph.insert_edges_bulk(&graph_edges)?;

        let pub_guard = self.publisher.read();
        if let Some(ref publisher) = *pub_guard {
            for (id, edge) in ids.iter().zip(edges.iter()) {
                publisher.emit(PubSubEvent::EdgeChanged {
                    from_node: edge.from,
                    to_node: edge.to,
                    edge_id: *id,
                    snapshot_id: 0,
                });
            }
        }

        Ok(ids)
    }

    fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError> {
        self.graph.delete_entity(id)
    }

    fn update_node(&self, node_id: i64, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        // SQLite backend: Use UPDATE SQL query
        self.graph.update_entity(&GraphEntity {
            id: node_id,
            kind: node.kind,
            name: node.name,
            file_path: node.file_path,
            data: node.data,
        })?;
        Ok(node_id)
    }

    fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.graph.all_entity_ids()
    }

    fn neighbors(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        match self.resolve_snapshot(snapshot_id)? {
            SnapshotView::Live => self.query_neighbors(node, query.direction, &query.edge_type),
            SnapshotView::Historical(state) => {
                self.historical_neighbors(&state, node, query.direction, &query.edge_type)
            }
        }
    }

    fn bfs(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        match self.resolve_snapshot(snapshot_id)? {
            SnapshotView::Live => {
                // Check query cache first
                if let Some(cached_result) = self.graph.query_cache.get_bfs(start, depth) {
                    return Ok(cached_result);
                }
                // Cache miss - compute and cache the result
                let result = bfs_neighbors(&self.graph, start, depth)?;
                self.graph.query_cache.put_bfs(start, depth, result.clone());
                Ok(result)
            }
            SnapshotView::Historical(state) => {
                // BFS over historical adjacency — no caching (cold path).
                if !state.contains_node(start) {
                    return Err(SqliteGraphError::query(format!(
                        "node {} does not exist in historical snapshot",
                        start
                    )));
                }
                let mut visited = Vec::new();
                let mut seen = ahash::AHashSet::new();
                let mut queue = std::collections::VecDeque::new();
                queue.push_back((start, 0u32));
                seen.insert(start);
                while let Some((node, d)) = queue.pop_front() {
                    visited.push(node);
                    if d >= depth {
                        continue;
                    }
                    if let Some(adj) = state.get_outgoing(node) {
                        for &next in adj {
                            if seen.insert(next) {
                                queue.push_back((next, d + 1));
                            }
                        }
                    }
                }
                Ok(visited)
            }
        }
    }

    fn shortest_path(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.require_live(snapshot_id, "shortest_path")?;
        // Check query cache first
        if let Some(cached_result) = self.graph.query_cache.get_shortest_path(start, end) {
            return Ok(cached_result);
        }

        // Cache miss - compute and cache the result
        let result = shortest_path(&self.graph, start, end)?;
        self.graph
            .query_cache
            .put_shortest_path(start, end, result.clone());
        Ok(result)
    }

    fn node_degree(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        self.require_live(snapshot_id, "node_degree")?;
        let out = self.graph.fetch_outgoing(node)?.len();
        let incoming = self.graph.fetch_incoming(node)?.len();
        Ok((out, incoming))
    }

    fn k_hop(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.require_live(snapshot_id, "k_hop")?;
        // Check query cache first
        if let Some(cached_result) = self.graph.query_cache.get_k_hop(start, depth, direction) {
            return Ok(cached_result);
        }

        // Cache miss - compute and cache the result
        let result = multi_hop::k_hop(&self.graph, start, depth, direction)?;
        self.graph
            .query_cache
            .put_k_hop(start, depth, direction, result.clone());
        Ok(result)
    }

    fn k_hop_filtered(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.require_live(snapshot_id, "k_hop_filtered")?;
        // Check query cache first
        if let Some(cached_result) =
            self.graph
                .query_cache
                .get_k_hop_filtered(start, depth, direction, allowed_edge_types)
        {
            return Ok(cached_result);
        }

        // Cache miss - compute and cache the result
        let result =
            multi_hop::k_hop_filtered(&self.graph, start, depth, direction, allowed_edge_types)?;
        self.graph.query_cache.put_k_hop_filtered(
            start,
            depth,
            direction,
            allowed_edge_types,
            result.clone(),
        );
        Ok(result)
    }

    fn bfs_filtered(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.require_live(snapshot_id, "bfs_filtered")?;
        bfs_neighbors_filtered(&self.graph, start, depth, allowed_edge_types, direction)
    }

    fn shortest_path_filtered(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        end: i64,
        allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.require_live(snapshot_id, "shortest_path_filtered")?;
        shortest_path_filtered(&self.graph, start, end, allowed_edge_types)
    }

    fn chain_query(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        chain: &[crate::multi_hop::ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.require_live(snapshot_id, "chain_query")?;
        multi_hop::chain_query(&self.graph, start, chain)
    }

    fn pattern_search(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        self.require_live(snapshot_id, "pattern_search")?;
        pattern::execute_pattern(&self.graph, start, pattern)
    }

    fn checkpoint(&self) -> Result<(), SqliteGraphError> {
        // Execute SQLite WAL checkpoint
        let conn = self.graph.connection();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            // wal_checkpoint returns a row with 3 integers: (busy, log, checkpointed)
            // We don't need to use them, just execute the checkpoint
            let _busy: i32 = row.get(0)?;
            let _log: i32 = row.get(1)?;
            let _checkpointed: i32 = row.get(2)?;
            Ok(())
        })
        .map_err(|e| SqliteGraphError::connection(format!("WAL checkpoint failed: {}", e)))?;
        Ok(())
    }

    fn flush(&self) -> Result<(), SqliteGraphError> {
        // SQLite handles sync automatically; this is a no-op
        Ok(())
    }

    fn backup(
        &self,
        backup_dir: &std::path::Path,
    ) -> Result<crate::backend::BackupResult, SqliteGraphError> {
        use std::fs;

        // Ensure backup directory exists
        fs::create_dir_all(backup_dir).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create backup directory: {}", e))
        })?;

        // Generate backup filename with timestamp
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup_path = backup_dir.join(format!("backup_{}.db", timestamp));
        let manifest_path = backup_dir.join(format!("backup_{}.json", timestamp));

        // Use SQLite's backup API via VACUUM INTO for a clean backup
        let conn = self.graph.connection();
        conn.execute(&format!("VACUUM INTO '{}'", backup_path.display()), [])
            .map_err(|e| SqliteGraphError::connection(format!("SQLite backup failed: {}", e)))?;

        // Get backup metadata
        let metadata = fs::metadata(&backup_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to read backup metadata: {}", e))
        })?;

        // Get entity count
        let entity_ids = self
            .graph
            .all_entity_ids()
            .map_err(|e| SqliteGraphError::query(format!("Failed to get entity count: {}", e)))?;

        // Create a simple manifest
        let manifest = serde_json::json!({
            "timestamp": timestamp,
            "backup_file": backup_path.display().to_string(),
            "size_bytes": metadata.len(),
            "entity_count": entity_ids.len(),
        });
        fs::write(&manifest_path, manifest.to_string()).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to write manifest: {}", e))
        })?;

        Ok(crate::backend::BackupResult {
            snapshot_path: backup_path,
            manifest_path,
            size_bytes: metadata.len() as u64,
            checksum: 0, // SQLite doesn't provide checksum
            record_count: entity_ids.len() as u64,
            duration_secs: 0.0, // Not tracked for SQLite backup
            timestamp,
            checkpoint_performed: false, // VACUUM INTO doesn't require explicit checkpoint
        })
    }

    fn snapshot_export(
        &self,
        export_dir: &std::path::Path,
    ) -> Result<crate::backend::SnapshotMetadata, SqliteGraphError> {
        use std::fs;

        // Ensure export directory exists
        fs::create_dir_all(export_dir).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create export directory: {}", e))
        })?;

        let snapshot_file = export_dir.join("snapshot.json");

        // Use existing dump_graph_to_path function
        crate::recovery::dump_graph_to_path(&self.graph, &snapshot_file)?;

        // Get metadata
        let metadata = fs::metadata(&snapshot_file).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to read snapshot metadata: {}", e))
        })?;

        let entity_ids = self
            .graph
            .all_entity_ids()
            .map_err(|e| SqliteGraphError::query(format!("Failed to get entity count: {}", e)))?;

        Ok(crate::backend::SnapshotMetadata {
            snapshot_path: snapshot_file,
            size_bytes: metadata.len(),
            entity_count: entity_ids.len() as u64,
            edge_count: 0, // SQLite dump doesn't separate edge count easily
        })
    }

    fn snapshot_import(
        &self,
        import_dir: &std::path::Path,
    ) -> Result<crate::backend::ImportMetadata, SqliteGraphError> {
        let snapshot_file = import_dir.join("snapshot.json");

        if !snapshot_file.exists() {
            return Err(SqliteGraphError::connection(format!(
                "Snapshot file not found: {}",
                snapshot_file.display()
            )));
        }

        // Get entity count before import
        let before_count = self
            .graph
            .all_entity_ids()
            .map_err(|e| SqliteGraphError::query(format!("Failed to get entity count: {}", e)))?
            .len();

        // Use existing load_graph_from_path function
        crate::recovery::load_graph_from_path(&self.graph, &snapshot_file)?;

        // Get entity count after import
        let after_count = self
            .graph
            .all_entity_ids()
            .map_err(|e| SqliteGraphError::query(format!("Failed to get entity count: {}", e)))?
            .len();

        Ok(crate::backend::ImportMetadata {
            snapshot_path: snapshot_file,
            entities_imported: (after_count - before_count) as u64,
            edges_imported: 0, // SQLite load doesn't separate edge count easily
        })
    }

    fn kv_get(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        key: &[u8],
    ) -> Result<Option<crate::backend::native::types::KvValue>, crate::SqliteGraphError> {
        self.require_live(snapshot_id, "kv_get")?;
        use std::time::SystemTime;

        // Initialize KV table if needed
        self.ensure_kv_table()?;

        // Convert key to string for storage (comma-separated bytes)
        let key_str = bytes_to_string(key);

        let conn = self.graph.connection();

        // Query the kv_store table
        let result = conn.query_row(
            "SELECT value_json, ttl_seconds, created_at FROM kv_store WHERE key = ?1",
            params![key_str],
            |row| {
                let value_json: String = row.get(0)?;
                let ttl_seconds: Option<u64> = row.get(1)?;
                let created_at: u64 = row.get(2)?;

                Ok((value_json, ttl_seconds, created_at))
            },
        );

        match result {
            Ok((value_json, ttl_seconds, created_at)) => {
                // Check TTL expiration
                if let Some(ttl) = ttl_seconds {
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    if now.saturating_sub(created_at) > ttl {
                        // Entry expired
                        return Ok(None);
                    }
                }

                // Parse JSON value back to KvValue
                let json_value: serde_json::Value =
                    serde_json::from_str(&value_json).map_err(|e| {
                        SqliteGraphError::connection(format!(
                            "Failed to parse KV value JSON: {}",
                            e
                        ))
                    })?;

                let kv_value = json_to_kv_value(json_value)?;
                Ok(Some(kv_value))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(SqliteGraphError::query(format!(
                "Failed to query KV store: {}",
                e
            ))),
        }
    }

    fn kv_set(
        &self,
        key: Vec<u8>,
        value: crate::backend::native::types::KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), crate::SqliteGraphError> {
        use std::time::SystemTime;

        // Initialize KV table if needed
        self.ensure_kv_table()?;

        // Convert key to string for storage
        let key_str = bytes_to_string(&key);

        // Serialize KvValue to JSON
        let json_value = kv_value_to_json(&value);
        let value_json = serde_json::to_string(&json_value).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to serialize KV value: {}", e))
        })?;

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let conn = self.graph.connection();

        // Check if key exists
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM kv_store WHERE key = ?1",
                params![key_str],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if count > 0 {
            // Update existing entry
            conn.execute(
                "UPDATE kv_store SET value_json = ?1, ttl_seconds = ?2, updated_at = ?3, version = version + 1 WHERE key = ?4",
                params![value_json, ttl_seconds, now, key_str],
            )
                .map_err(|e| SqliteGraphError::query(format!("Failed to update KV entry: {}", e)))?;
        } else {
            // Insert new entry
            conn.execute(
                "INSERT INTO kv_store (key, value_json, ttl_seconds, created_at, updated_at, version) VALUES (?1, ?2, ?3, ?4, ?4, 1)",
                params![key_str, value_json, ttl_seconds, now],
            )
                .map_err(|e| SqliteGraphError::query(format!("Failed to insert KV entry: {}", e)))?;
        }

        Ok(())
    }

    fn kv_delete(&self, key: &[u8]) -> Result<(), crate::SqliteGraphError> {
        // Initialize KV table if needed
        self.ensure_kv_table()?;

        // Convert key to string for storage
        let key_str = bytes_to_string(key);

        let conn = self.graph.connection();

        // Delete the entry (ignore if not found - idempotent)
        conn.execute("DELETE FROM kv_store WHERE key = ?1", params![key_str])
            .map_err(|e| SqliteGraphError::query(format!("Failed to delete KV entry: {}", e)))?;

        Ok(())
    }

    fn subscribe(
        &self,
        filter: crate::backend::SubscriptionFilter,
    ) -> Result<
        (u64, std::sync::mpsc::Receiver<crate::backend::PubSubEvent>),
        crate::SqliteGraphError,
    > {
        // Lazy initialize publisher
        let mut pub_guard = self.publisher.write();
        if pub_guard.is_none() {
            *pub_guard = Some(Publisher::new());
        }
        let (id, rx) = pub_guard
            .as_ref()
            .expect("invariant: publisher just initialized")
            .subscribe(filter);
        Ok((id, rx))
    }

    fn unsubscribe(&self, subscriber_id: u64) -> Result<bool, crate::SqliteGraphError> {
        let pub_guard = self.publisher.read();
        if let Some(ref publisher) = *pub_guard {
            Ok(publisher.unsubscribe(subscriber_id))
        } else {
            Ok(false) // Publisher not initialized, nothing to unsubscribe
        }
    }

    // ========== Pub/Sub Enhancement APIs (v1.4.0) ==========

    fn kv_prefix_scan(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, crate::backend::native::types::KvValue)>, crate::SqliteGraphError>
    {
        self.require_live(snapshot_id, "kv_prefix_scan")?;
        self.ensure_kv_table()?;
        let conn = self.graph.connection();

        // Convert prefix to string pattern for LIKE query
        // Escape special LIKE characters: % and _
        let prefix_str = String::from_utf8_lossy(prefix);
        let pattern = prefix_str.replace('%', "\\%").replace('_', "\\_") + "%";

        let mut stmt = conn
            .prepare_cached("SELECT key, value_json FROM kv_store WHERE key LIKE ?1 ESCAPE '\\'")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let mut results = Vec::new();
        let query_result = stmt.query_map([&pattern], |row| {
            let key: String = row.get(0)?;
            let value_json: String = row.get(1)?;
            Ok((key, value_json))
        });

        for row in query_result.map_err(|e| SqliteGraphError::query(e.to_string()))? {
            let (key, value_json) = row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
            let json_value: serde_json::Value = serde_json::from_str(&value_json)
                .map_err(|e| SqliteGraphError::query(format!("Failed to parse JSON: {}", e)))?;
            let kv_value = json_to_kv_value(json_value)?;
            results.push((key.into_bytes(), kv_value));
        }

        // Sort by key for deterministic output
        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    fn query_nodes_by_kind(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, crate::SqliteGraphError> {
        self.require_live(snapshot_id, "query_nodes_by_kind")?;
        let conn = self.graph.connection();
        let mut stmt = conn
            .prepare_cached("SELECT id FROM graph_entities WHERE kind = ?1")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let node_ids: Vec<i64> = stmt
            .query_map([kind], |row| row.get(0))
            .map_err(|e| SqliteGraphError::query(e.to_string()))?
            .collect::<Result<_, _>>()
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        Ok(node_ids)
    }

    fn query_nodes_by_name_pattern(
        &self,
        snapshot_id: crate::snapshot::SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, crate::SqliteGraphError> {
        self.require_live(snapshot_id, "query_nodes_by_name_pattern")?;
        let conn = self.graph.connection();
        let mut stmt = conn
            .prepare_cached("SELECT id FROM graph_entities WHERE name GLOB ?1")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let node_ids: Vec<i64> = stmt
            .query_map([pattern], |row| row.get(0))
            .map_err(|e| SqliteGraphError::query(e.to_string()))?
            .collect::<Result<_, _>>()
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        Ok(node_ids)
    }

    fn match_triples(
        &self,
        pattern: &crate::pattern_engine::PatternTriple,
    ) -> Result<Vec<crate::pattern_engine::TripleMatch>, crate::SqliteGraphError> {
        crate::pattern_engine::match_triples(&self.graph, pattern)
    }

    fn vector_search(
        &self,
        _snapshot: crate::snapshot::SnapshotId,
        index_name: &str,
        vector: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, crate::SqliteGraphError> {
        let raw_results = self
            .graph
            .get_hnsw_index_ref(index_name, |idx| idx.search(vector, k))
            .map_err(|e| {
                crate::SqliteGraphError::query(format!("HNSW index '{}': {}", index_name, e))
            })?
            .map_err(|e| crate::SqliteGraphError::query(format!("HNSW search failed: {}", e)))?;

        Ok(raw_results
            .into_iter()
            .map(|(id, dist)| (id as i64, dist))
            .collect())
    }

    fn get_graph_ref(&self) -> Option<&crate::graph::SqliteGraph> {
        Some(&self.graph)
    }
}

/// Convert KvValue to serde_json::Value for serialization
fn kv_value_to_json(value: &crate::backend::native::types::KvValue) -> serde_json::Value {
    use crate::backend::native::types::KvValue;

    match value {
        KvValue::Null => serde_json::json!({"type": "null"}),
        KvValue::Bytes(bytes) => {
            serde_json::json!({
                "type": "bytes",
                "data": bytes_to_string(bytes),
            })
        }
        KvValue::String(s) => {
            serde_json::json!({
                "type": "string",
                "data": s,
            })
        }
        KvValue::Integer(n) => {
            serde_json::json!({
                "type": "integer",
                "data": n,
            })
        }
        KvValue::Float(f) => {
            serde_json::json!({
                "type": "float",
                "data": f,
            })
        }
        KvValue::Boolean(b) => {
            serde_json::json!({
                "type": "boolean",
                "data": b,
            })
        }
        KvValue::Json(j) => {
            serde_json::json!({
                "type": "json",
                "data": j,
            })
        }
    }
}

/// Convert bytes to comma-separated string for storage
fn bytes_to_string(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 {
            result.push(',');
        }
        write!(result, "{}", byte).expect("infallible: writing to String");
    }
    result
}

/// Convert comma-separated string back to bytes
fn string_to_bytes(s: &str) -> Result<Vec<u8>, SqliteGraphError> {
    s.split(',')
        .map(|part| {
            part.trim()
                .parse::<u8>()
                .map_err(|_| SqliteGraphError::connection(format!("Invalid byte string: {}", s)))
        })
        .collect()
}

/// Convert serde_json::Value back to KvValue after deserialization
fn json_to_kv_value(
    json_value: serde_json::Value,
) -> Result<crate::backend::native::types::KvValue, SqliteGraphError> {
    use crate::backend::native::types::KvValue;

    let type_str = json_value
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            SqliteGraphError::connection("Missing type field in KV value JSON".to_string())
        })?;

    let data = json_value.get("data").ok_or_else(|| {
        SqliteGraphError::connection("Missing data field in KV value JSON".to_string())
    })?;

    match type_str {
        "bytes" => {
            let bytes_str = data.as_str().ok_or_else(|| {
                SqliteGraphError::connection("Invalid bytes data in KV value".to_string())
            })?;
            let bytes = string_to_bytes(bytes_str)?;
            Ok(KvValue::Bytes(bytes))
        }
        "string" => {
            let s = data.as_str().ok_or_else(|| {
                SqliteGraphError::connection("Invalid string data in KV value".to_string())
            })?;
            Ok(KvValue::String(s.to_string()))
        }
        "integer" => {
            let n = data.as_i64().ok_or_else(|| {
                SqliteGraphError::connection("Invalid integer data in KV value".to_string())
            })?;
            Ok(KvValue::Integer(n))
        }
        "float" => {
            let f = data.as_f64().ok_or_else(|| {
                SqliteGraphError::connection("Invalid float data in KV value".to_string())
            })?;
            Ok(KvValue::Float(f))
        }
        "boolean" => {
            let b = data.as_bool().ok_or_else(|| {
                SqliteGraphError::connection("Invalid boolean data in KV value".to_string())
            })?;
            Ok(KvValue::Boolean(b))
        }
        "json" => Ok(KvValue::Json(data.clone())),
        _ => Err(SqliteGraphError::connection(format!(
            "Unknown KV value type: {}",
            type_str
        ))),
    }
}
