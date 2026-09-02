//! Snapshot management functionality for SqliteGraph.

use std::sync::Arc;

use crate::errors::SqliteGraphError;

use super::SqliteGraph;

impl SqliteGraph {
    /// Update snapshot with current cache state
    /// This is called automatically after write operations
    pub(crate) fn update_snapshot(&self) {
        self.snapshot_manager.update_snapshot(
            &self.outgoing_cache_ref().inner(),
            &self.incoming_cache_ref().inner(),
        );
    }

    /// Acquire a deterministic snapshot of the current graph state
    ///
    /// Returns a read-only snapshot that provides isolated access to graph data.
    /// The snapshot contains cloned adjacency maps and uses a read-only SQLite connection.
    ///
    /// # MVCC-lite Snapshot Isolation
    ///
    /// Snapshots provide **MVCC-lite** isolation guarantees:
    /// - **Immutable**: Snapshot state never changes after creation
    /// - **Consistent**: Snapshot sees a point-in-time view of the graph
    /// - **Isolated**: Snapshot unaffected by subsequent writes
    /// - **Cloned Data**: Adjacency maps are fully cloned (not shared)
    ///
    /// # Cache Requirement
    ///
    /// **IMPORTANT**: Snapshots read from the in-memory adjacency cache, not the database.
    /// For accurate snapshots, the cache must be warmed first:
    ///
    /// ```ignore
    /// use sqlitegraph::SqliteGraph;
    ///
    /// let graph = SqliteGraph::open_in_memory()?;
    /// // ... perform writes ...
    ///
    /// // Warm cache before snapshot
    /// let entity_ids = graph.list_entity_ids()?;
    /// for &id in &entity_ids {
    ///     let _ = graph.query().outgoing(id);
    ///     let _ = graph.query().incoming(id);
    /// }
    ///
    /// // Now acquire snapshot
    /// let snapshot = graph.acquire_snapshot()?;
    /// assert!(snapshot.node_count() > 0);
    /// # Ok::<(), sqlitegraph::SqliteGraphError>(())
    /// ```
    ///
    /// Without cache warming, snapshots may appear empty even if the database has data.
    ///
    /// # Thread Safety
    ///
    /// The underlying `SnapshotManager` is thread-safe and uses lock-free `ArcSwap`.
    /// However, `SqliteGraph` itself is **NOT thread-safe** (contains `RefCell`, non-Sync types).
    ///
    /// For concurrent snapshot acquisition, wrap `SqliteGraph` in a `Mutex` or `RwLock`:
    ///
    /// ```rust
    /// use std::sync::{Arc, Mutex};
    /// use sqlitegraph::SqliteGraph;
    ///
    /// let graph = Arc::new(Mutex::new(SqliteGraph::open_in_memory()?));
    /// // Multiple threads can now safely acquire snapshots
    /// # Ok::<(), sqlitegraph::SqliteGraphError>(())
    /// ```
    ///
    /// # Performance
    ///
    /// - **Acquisition**: < 1ms typical (Arc::clone overhead)
    /// - **Memory**: O(N + E) where N = nodes, E = edges (full copy)
    /// - **Throughput**: > 10,000 snapshots/second single-threaded
    ///
    /// # Returns
    ///
    /// Result containing `GraphSnapshot` or error
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Read-only SQLite connection cannot be opened
    /// - Database connection fails
    pub fn acquire_snapshot(&self) -> Result<crate::mvcc::GraphSnapshot, SqliteGraphError> {
        // Update snapshot with current cache state
        self.update_snapshot();

        // Acquire snapshot state
        let snapshot_state = self.snapshot_manager.acquire_snapshot();

        // Use in-memory database for snapshot operations
        let db_path = ":memory:";

        crate::mvcc::GraphSnapshot::new(snapshot_state, db_path)
            .map_err(|e| SqliteGraphError::connection(e.to_string()))
    }

    /// Convenience alias for `acquire_snapshot()`
    ///
    /// This is a shorter name for acquiring snapshots, equivalent to:
    /// ```ignore
    /// # use sqlitegraph::SqliteGraph;
    /// let graph = SqliteGraph::open_in_memory()?;
    /// let snapshot = graph.snapshot()?;
    /// ```
    ///
    /// See `acquire_snapshot()` for full documentation.
    pub fn snapshot(&self) -> Result<crate::mvcc::GraphSnapshot, SqliteGraphError> {
        self.acquire_snapshot()
    }

    /// Get the current snapshot state without creating a new connection
    /// This is useful for internal operations and testing
    pub(crate) fn current_snapshot_state(&self) -> Arc<crate::mvcc::SnapshotState> {
        self.update_snapshot();
        self.snapshot_manager.current_snapshot()
    }

    /// Get the number of nodes in the current snapshot
    ///
    /// **Note**: This requires cache warming to return accurate results.
    /// See `acquire_snapshot()` documentation for details.
    pub fn snapshot_node_count(&self) -> usize {
        self.current_snapshot_state().node_count()
    }

    /// Get the number of edges in the current snapshot
    ///
    /// **Note**: This requires cache warming to return accurate results.
    /// See `acquire_snapshot()` documentation for details.
    pub fn snapshot_edge_count(&self) -> usize {
        self.current_snapshot_state().edge_count()
    }

    /// Check if a node exists in the current snapshot
    ///
    /// **Note**: This requires cache warming to return accurate results.
    /// See `acquire_snapshot()` documentation for details.
    pub fn snapshot_contains_node(&self, node_id: i64) -> bool {
        self.current_snapshot_state().contains_node(node_id)
    }

    // ── Temporal version chain ────────────────────────────────────────────

    /// Capture the current graph state as a numbered version in the MVCC
    /// version chain.
    ///
    /// This is the explicit "commit" step for temporal tracking. After calling
    /// `checkpoint()`, the returned version number can be passed to
    /// [`snapshot_as_of`](Self::snapshot_as_of) to retrieve an immutable view
    /// of the graph at this point in time. Historical reads via
    /// [`crate::snapshot::SnapshotId::from_lsn`] with this version number will
    /// serve adjacency from this snapshot.
    ///
    /// By default the version chain is empty (zero overhead). Call
    /// `checkpoint()` only when you want to retain a point-in-time view. The
    /// chain is bounded — when full, the oldest version is evicted (default
    /// capacity: 64 versions).
    ///
    /// # Cache Requirement
    ///
    /// Like [`acquire_snapshot`](Self::acquire_snapshot), this reads from the
    /// in-memory adjacency cache. Warm the cache first for accurate snapshots.
    ///
    /// # Returns
    ///
    /// The assigned version number (starts at 1, monotonically increasing).
    pub fn checkpoint(&self) -> u64 {
        self.update_snapshot();
        self.snapshot_manager.checkpoint()
    }

    /// Retrieve a historical snapshot by version number.
    ///
    /// Returns the [`VersionedSnapshot`](crate::mvcc::VersionedSnapshot) if the
    /// version exists in the retained chain, or `None` if it was never
    /// checkpointed or has been evicted by the bounded-retention policy.
    pub fn snapshot_as_of(&self, version: u64) -> Option<crate::mvcc::VersionedSnapshot> {
        self.snapshot_manager.as_of(version)
    }

    /// All retained versions (oldest first).
    pub fn snapshot_versions(&self) -> Vec<crate::mvcc::VersionedSnapshot> {
        self.snapshot_manager.versions()
    }

    /// Number of versions currently retained in the history chain.
    pub fn snapshot_version_count(&self) -> usize {
        self.snapshot_manager.version_count()
    }

    /// The oldest version number still retained, or `None` if the chain is empty.
    pub fn snapshot_oldest_version(&self) -> Option<u64> {
        self.snapshot_manager.oldest_version()
    }

    /// The newest version number still retained, or `None` if the chain is empty.
    pub fn snapshot_newest_version(&self) -> Option<u64> {
        self.snapshot_manager.newest_version()
    }

    /// Clear the version history (keeps the live state intact).
    pub fn clear_snapshot_history(&self) {
        self.snapshot_manager.clear_history();
    }
}
