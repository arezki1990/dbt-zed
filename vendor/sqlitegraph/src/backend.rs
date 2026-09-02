//! Backend trait bridging sqlitegraph with higher-level graph consumers.
//!
//! This module contains the core GraphBackend trait and redirects to modular
//! backend implementations. SQLite-specific implementations are in the sqlite submodule.

// Include the modular backend structure
mod combined;
mod sqlite;

// Include native backend storage layer (no GraphBackend implementation yet)
pub mod native;

// Re-export from sqlite submodule
pub use combined::CombinedGraphBackend;
pub use sqlite::SqliteGraphBackend;

// Re-export from native submodule
pub use native::NativeGraphBackend;

// Re-export types for external users
pub use crate::multi_hop::ChainStep;
pub use sqlite::types::{BackendDirection, EdgeSpec, NeighborQuery, NodeSpec};

// KV store types (re-exported for public API)
pub use crate::backend::native::types::{KvStoreError, KvValue};

/// Types of pub/sub events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PubSubEventType {
    /// Node created or modified
    NodeChanged = 1,
    /// Edge created or modified
    EdgeChanged = 2,
    /// KV entry created, modified, or deleted
    KvChanged = 3,
    /// Transaction committed
    SnapshotCommitted = 4,
    /// All event types
    All = 255,
}

/// Event delivered to subscribers
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PubSubEvent {
    /// Node created or modified
    NodeChanged {
        /// Node ID that changed
        node_id: i64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// Edge created or modified
    EdgeChanged {
        /// Edge ID that changed
        edge_id: i64,
        /// Source node ID
        from_node: i64,
        /// Target node ID
        to_node: i64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// KV entry changed
    KvChanged {
        /// Key hash for the KV entry
        key_hash: u64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// Transaction committed
    SnapshotCommitted {
        /// Snapshot ID that was committed
        snapshot_id: u64,
    },
}

/// Filter for subscriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubscriptionFilter {
    /// Subscribe to NodeChanged events
    pub node_changes: bool,
    /// Subscribe to EdgeChanged events
    pub edge_changes: bool,
    /// Subscribe to KvChanged events
    pub kv_changes: bool,
    /// Subscribe to SnapshotCommitted events
    pub snapshot_commits: bool,
}

impl SubscriptionFilter {
    /// Create a filter that receives all event types
    pub fn all() -> Self {
        Self {
            node_changes: true,
            edge_changes: true,
            kv_changes: true,
            snapshot_commits: true,
        }
    }
}

use crate::{
    SqliteGraphError,
    graph::GraphEntity,
    pattern::{PatternMatch, PatternQuery},
    pattern_engine::{PatternTriple, TripleMatch},
    snapshot::SnapshotId,
};
///
/// Each trait method delegates to backend-specific primitives while ensuring
/// deterministic behavior and a single integration surface for consumers.
///
/// # Snapshot Isolation
///
/// All read operations require a `snapshot_id: SnapshotId` parameter to enforce
/// ACID compliance. Reads only observe data committed at or before the snapshot.
pub trait GraphBackend {
    // Write operations (unchanged - commit returns SnapshotId in future)
    fn insert_node(&self, node: NodeSpec) -> Result<i64, SqliteGraphError>;

    /// Insert a node with a manually specified ID.
    ///
    /// Backends that support manual ID assignment (e.g. V3Backend, SqliteGraphBackend)
    /// will respect this ID. Default implementation injects "id" into the node spec data.
    fn insert_node_with_id(
        &self,
        mut node: NodeSpec,
        node_id: i64,
    ) -> Result<i64, SqliteGraphError> {
        if let Some(obj) = node.data.as_object_mut() {
            obj.insert("id".to_string(), serde_json::Value::Number(node_id.into()));
        } else {
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), serde_json::Value::Number(node_id.into()));
            node.data = serde_json::Value::Object(obj);
        }
        self.insert_node(node)
    }

    fn insert_edge(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError>;

    /// Insert many nodes atomically.
    ///
    /// Default implementation loops single-insert for each item; backends
    /// override with a single-transaction path. Returns the new node IDs in
    /// the same order as `nodes`. All-or-nothing: if any insert fails the
    /// caller should assume nothing was committed.
    fn insert_nodes_bulk(&self, nodes: &[NodeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        let mut ids = Vec::with_capacity(nodes.len());
        for n in nodes {
            ids.push(self.insert_node(n.clone())?);
        }
        Ok(ids)
    }

    /// Insert many edges atomically.
    ///
    /// Default implementation loops single-insert for each item; backends
    /// override with a single-transaction path. Returns the new edge IDs in
    /// the same order as `edges`. All-or-nothing semantics on the bulk path;
    /// the default fallback inherits whatever atomicity the single
    /// `insert_edge` provides.
    fn insert_edges_bulk(&self, edges: &[EdgeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        let mut ids = Vec::with_capacity(edges.len());
        for e in edges {
            ids.push(self.insert_edge(e.clone())?);
        }
        Ok(ids)
    }

    /// Update an existing node in place without allocating a new node ID
    ///
    /// This modifies the data associated with an existing node while preserving
    /// its node_id.
    ///
    /// # Arguments
    /// * `node_id` - The ID of the node to update (must exist)
    /// * `node` - New node specification (kind, name, file_path, data)
    ///
    /// # Returns
    /// The same `node_id` that was passed in, if update succeeded
    ///
    /// # Errors
    /// Returns `SqliteGraphError` if:
    /// - The node_id doesn't exist
    /// - The update operation fails
    /// - The backend doesn't support in-place updates
    ///
    /// # Behavior by Backend
    /// - **Native**: Uses WAL to update node data in place, preserving node_id
    /// - **SQLite**: Uses UPDATE SQL query on entities table
    ///
    /// # Example
    /// ```ignore
    /// // Create a node
    /// let node_id = backend.insert_node(NodeSpec {
    ///     kind: "File".to_string(),
    ///     name: "main.rs".to_string(),
    ///     file_path: Some("main.rs".to_string()),
    ///     data: serde_json::json!({"hash": "abc123"}),
    /// })?;
    ///
    /// // Update it - this does NOT allocate a new node_id
    /// let updated_id = backend.update_node(node_id, NodeSpec {
    ///     kind: "File".to_string(),
    ///     name: "main.rs".to_string(),
    ///     file_path: Some("main.rs".to_string()),
    ///     data: serde_json::json!({"hash": "def456", "modified": true}),
    /// })?;
    ///
    /// assert_eq!(updated_id, node_id); // Same ID!
    /// ```
    fn update_node(&self, node_id: i64, node: NodeSpec) -> Result<i64, SqliteGraphError>;

    /// Delete an entity (node) from the graph by ID
    ///
    /// This removes the entity and all associated edges from the graph.
    /// For SQLite backend: deletes from entities table and cascades to edges
    /// For Native backend: marks node as deleted and updates adjacency indexes
    fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError>;

    /// Get all entity IDs from the graph
    ///
    /// Returns a vector of all node IDs currently stored in the graph.
    /// For SQLite backend: queries all IDs from entities table
    /// For Native backend: iterates over node store
    fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError>;

    /// Alias for `entity_ids()` for backward compatibility with algorithms
    ///
    /// Default implementation delegates to `entity_ids()`.
    fn all_entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.entity_ids()
    }

    /// Get outgoing neighbors for a node (convenience method)
    ///
    /// Default implementation uses `neighbors` with Outgoing direction.
    fn fetch_outgoing(&self, node: i64) -> Result<Vec<i64>, SqliteGraphError> {
        self.neighbors(
            SnapshotId::current(),
            node,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
    }

    /// Get incoming neighbors for a node (convenience method)
    ///
    /// Default implementation uses `neighbors` with Incoming direction.
    fn fetch_incoming(&self, node: i64) -> Result<Vec<i64>, SqliteGraphError> {
        self.neighbors(
            SnapshotId::current(),
            node,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
    }

    // Read operations (require snapshot_id parameter)
    fn get_node(&self, snapshot_id: SnapshotId, id: i64) -> Result<GraphEntity, SqliteGraphError>;
    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn bfs(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn shortest_path(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError>;
    fn node_degree(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError>;
    fn k_hop(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn k_hop_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn bfs_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn shortest_path_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
        allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError>;
    fn chain_query(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        chain: &[crate::multi_hop::ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError>;
    fn pattern_search(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError>;

    /// Match triples based on a pattern triple
    ///
    /// Provides pattern matching capability for Cypher queries.
    /// Default implementation returns unsupported operation error.
    ///
    /// # Arguments
    /// * `pattern` - The pattern triple to match
    ///
    /// # Returns
    /// Vector of triple matches with start_id, end_id, and edge_id
    fn match_triples(&self, pattern: &PatternTriple) -> Result<Vec<TripleMatch>, SqliteGraphError> {
        Err(SqliteGraphError::unsupported(format!(
            "match_triples not implemented for pattern: {:?}",
            pattern
        )))
    }

    /// Search HNSW vector index for nearest neighbors
    ///
    /// Provides vector similarity search capability for Cypher queries.
    /// Default implementation returns unsupported operation error.
    ///
    /// # Arguments
    /// * `_snapshot` - Snapshot ID for read isolation
    /// * `index_name` - Name of the HNSW index to search
    /// * `_vector` - Query vector
    /// * `_k` - Number of nearest neighbors to return
    ///
    /// # Returns
    /// Vector of (node_id, distance) tuples for the k nearest neighbors
    fn vector_search(
        &self,
        _snapshot: SnapshotId,
        index_name: &str,
        _vector: &[f32],
        _k: usize,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        Err(SqliteGraphError::unsupported(format!(
            "vector_search not implemented for index '{}'",
            index_name
        )))
    }

    /// Get reference to underlying SqliteGraph (if available)
    ///
    /// Returns None for non-SQLite backends (e.g., native-v3).
    /// This method enables backend-agnostic introspection while
    /// maintaining compatibility with SQLite-specific features.
    ///
    /// # Returns
    /// Some(&SqliteGraph) for SQLite backends, None otherwise
    fn get_graph_ref(&self) -> Option<&crate::graph::SqliteGraph> {
        None
    }

    /// Trigger WAL checkpoint for backends that support write-ahead logging
    ///
    /// For Native backend with WAL: flushes WAL to graph file
    /// For SQLite backend: executes PRAGMA wal_checkpoint(TRUNCATE)
    /// For backends without WAL: returns Ok(()) as no-op
    fn checkpoint(&self) -> Result<(), SqliteGraphError>;

    /// Force immediate flush of WAL buffer to disk
    ///
    /// Ensures all buffered WAL records (including KV writes) are persisted
    /// immediately, making them visible to other processes.
    ///
    /// For Native backend with WAL: flushes WAL buffer to disk
    /// For SQLite backend: returns Ok(()) as no-op (SQLite handles sync)
    /// For backends without WAL: returns Ok(()) as no-op
    fn flush(&self) -> Result<(), SqliteGraphError>;

    /// Create a backup of the database
    ///
    /// Creates a consistent snapshot of the database including all data pages.
    ///
    /// # Arguments
    /// * `backup_dir` - Destination directory for backup files
    ///
    /// # Returns
    /// Backup result with paths, checksum, and metadata
    fn backup(&self, backup_dir: &std::path::Path) -> Result<BackupResult, SqliteGraphError>;

    /// Export database snapshot to the specified directory
    ///
    /// Creates a consistent snapshot of the current database state.
    /// For Native backend: uses snapshot format
    /// For SQLite backend: uses JSON dump format
    ///
    /// # Arguments
    /// * `export_dir` - Directory path where snapshot will be written
    ///
    /// # Returns
    /// Snapshot metadata including file paths and size information
    fn snapshot_export(
        &self,
        export_dir: &std::path::Path,
    ) -> Result<SnapshotMetadata, SqliteGraphError>;

    /// Import database snapshot from the specified directory
    ///
    /// Restores database state from a previously created snapshot.
    /// For Native backend: loads snapshot format
    /// For SQLite backend: loads JSON dump format
    ///
    /// # Arguments
    /// * `import_dir` - Directory path containing snapshot files
    ///
    /// # Returns
    /// Import metadata including number of records imported
    fn snapshot_import(
        &self,
        import_dir: &std::path::Path,
    ) -> Result<ImportMetadata, SqliteGraphError>;

    /// Get a value from the KV store at the given snapshot
    ///
    /// # Arguments
    /// * `snapshot_id` - Only return data committed at or before this snapshot
    /// * `key` - Key to retrieve (arbitrary bytes)
    ///
    /// # Returns
    /// The value if found and visible at snapshot, or None if not found
    fn kv_get(
        &self,
        snapshot_id: SnapshotId,
        key: &[u8],
    ) -> Result<Option<crate::backend::native::types::KvValue>, SqliteGraphError>;

    /// Set a value in the KV store
    ///
    /// This operation participates in the current transaction and will
    /// be committed atomically with other graph operations.
    ///
    /// # Arguments
    /// * `key` - Key to set (arbitrary bytes)
    /// * `value` - Value to store
    /// * `ttl_seconds` - Optional TTL in seconds (None = no expiration)
    fn kv_set(
        &self,
        key: Vec<u8>,
        value: crate::backend::native::types::KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), SqliteGraphError>;

    /// Delete a value from the KV store
    ///
    /// This operation participates in the current transaction and will
    /// be committed atomically with other graph operations.
    ///
    /// # Arguments
    /// * `key` - Key to delete
    fn kv_delete(&self, key: &[u8]) -> Result<(), SqliteGraphError>;

    // Pub/Sub operations (in-process event notification)

    /// Subscribe to graph change events
    ///
    /// Returns a subscriber ID and a receiver channel for events.
    /// The receiver will receive events that match the given filter.
    ///
    /// # Events
    ///
    /// Events are emitted ONLY on transaction commit:
    /// - `NodeChanged` - node created or modified
    /// - `EdgeChanged` - edge created or modified
    /// - `KvChanged` - KV entry created, modified, or deleted
    /// - `SnapshotCommitted` - transaction committed
    ///
    /// # Best-Effort Delivery
    ///
    /// - No persistence of events
    /// - If receiver is dropped, events are silently dropped
    /// - If channel is full, events are silently dropped
    /// - No delivery guarantees
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (sub_id, rx) = graph.subscribe(SubscriptionFilter::all());
    /// // In another thread/task:
    /// for event in rx {
    ///     match event {
    ///         PubSubEvent::NodeChanged { node_id, snapshot_id } => {
    ///             // Read node state at snapshot_id
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// }
    /// ```
    fn subscribe(
        &self,
        filter: SubscriptionFilter,
    ) -> Result<(u64, std::sync::mpsc::Receiver<PubSubEvent>), SqliteGraphError>;

    /// Unsubscribe from events
    ///
    /// Cancels the subscription and stops receiving events.
    /// Returns true if subscription existed and was removed.
    ///
    /// # Arguments
    /// * `subscriber_id` - The subscriber ID returned by subscribe()
    fn unsubscribe(&self, subscriber_id: u64) -> Result<bool, SqliteGraphError>;

    // ========== Pub/Sub Enhancement APIs (v1.4.0) ==========

    /// Scan all KV entries with a given prefix
    ///
    /// Returns all keys that start with the given prefix, along with their values.
    /// Results are in lexicographic order by key.
    ///
    /// # Arguments
    /// * `snapshot_id` - Only return data committed at or before this snapshot
    /// * `prefix` - Prefix to match (empty prefix returns all keys)
    ///
    /// # Returns
    /// Vector of (key, value) pairs for all matching keys
    fn kv_prefix_scan(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, crate::backend::native::types::KvValue)>, SqliteGraphError>;

    /// Query all nodes with a given kind
    ///
    /// Returns all node IDs where the node's kind equals the given string.
    /// Results are sorted by node ID for deterministic output.
    ///
    /// # Arguments
    /// * `snapshot_id` - Only return data committed at or before this snapshot
    /// * `kind` - Kind string to match (case-sensitive)
    ///
    /// # Returns
    /// Vector of node IDs with matching kind
    fn query_nodes_by_kind(
        &self,
        snapshot_id: SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, SqliteGraphError>;

    /// Query nodes by name pattern using glob matching
    ///
    /// Returns all node IDs where the node's label matches the glob pattern.
    /// Pattern syntax:
    /// - `*` matches any sequence of characters
    /// - `?` matches exactly one character
    ///
    /// # Arguments
    /// * `snapshot_id` - Only return data committed at or before this snapshot
    /// * `pattern` - Glob pattern to match against node labels
    ///
    /// # Returns
    /// Vector of node IDs with matching labels
    fn query_nodes_by_name_pattern(
        &self,
        snapshot_id: SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, SqliteGraphError>;
}

/// Metadata returned by snapshot export operations
#[derive(Debug, Clone)]
pub struct SnapshotMetadata {
    /// Path to the snapshot file
    pub snapshot_path: std::path::PathBuf,
    /// Snapshot size in bytes
    pub size_bytes: u64,
    /// Number of entities in snapshot
    pub entity_count: u64,
    /// Number of edges in snapshot
    pub edge_count: u64,
}

/// Metadata returned by snapshot import operations
#[derive(Debug, Clone)]
pub struct ImportMetadata {
    /// Path to the imported snapshot
    pub snapshot_path: std::path::PathBuf,
    /// Number of entities imported
    pub entities_imported: u64,
    /// Number of edges imported
    pub edges_imported: u64,
}

/// Result returned by backup operations
#[derive(Debug, Clone)]
pub struct BackupResult {
    /// Path to backup snapshot file
    pub snapshot_path: std::path::PathBuf,

    /// Path to backup manifest file
    pub manifest_path: std::path::PathBuf,

    /// Backup size in bytes
    pub size_bytes: u64,

    /// Backup checksum
    pub checksum: u64,

    /// Number of records in backup
    pub record_count: u64,

    /// Backup duration in seconds
    pub duration_secs: f64,

    /// Backup timestamp (Unix epoch)
    pub timestamp: u64,

    /// Whether checkpoint was performed before backup
    pub checkpoint_performed: bool,
}

/// Reference implementation for GraphBackend trait that works with references.
impl<B> GraphBackend for &B
where
    B: GraphBackend + ?Sized,
{
    fn insert_node(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        (*self).insert_node(node)
    }

    fn insert_node_with_id(&self, node: NodeSpec, node_id: i64) -> Result<i64, SqliteGraphError> {
        (*self).insert_node_with_id(node, node_id)
    }

    fn get_node(&self, snapshot_id: SnapshotId, id: i64) -> Result<GraphEntity, SqliteGraphError> {
        (*self).get_node(snapshot_id, id)
    }

    fn insert_edge(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        (*self).insert_edge(edge)
    }

    fn update_node(&self, node_id: i64, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        (*self).update_node(node_id, node)
    }

    fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError> {
        (*self).delete_entity(id)
    }

    fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).entity_ids()
    }

    fn insert_nodes_bulk(&self, nodes: &[NodeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).insert_nodes_bulk(nodes)
    }

    fn insert_edges_bulk(&self, edges: &[EdgeSpec]) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).insert_edges_bulk(edges)
    }

    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).neighbors(snapshot_id, node, query)
    }

    fn bfs(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).bfs(snapshot_id, start, depth)
    }

    fn shortest_path(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        (*self).shortest_path(snapshot_id, start, end)
    }

    fn node_degree(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        (*self).node_degree(snapshot_id, node)
    }

    fn k_hop(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).k_hop(snapshot_id, start, depth, direction)
    }

    fn k_hop_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).k_hop_filtered(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn bfs_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).bfs_filtered(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn shortest_path_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
        allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        (*self).shortest_path_filtered(snapshot_id, start, end, allowed_edge_types)
    }

    fn chain_query(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        chain: &[crate::multi_hop::ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).chain_query(snapshot_id, start, chain)
    }

    fn pattern_search(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        (*self).pattern_search(snapshot_id, start, pattern)
    }

    fn checkpoint(&self) -> Result<(), SqliteGraphError> {
        (*self).checkpoint()
    }

    fn flush(&self) -> Result<(), SqliteGraphError> {
        (*self).flush()
    }

    fn backup(&self, backup_dir: &std::path::Path) -> Result<BackupResult, SqliteGraphError> {
        (*self).backup(backup_dir)
    }

    fn snapshot_export(
        &self,
        export_dir: &std::path::Path,
    ) -> Result<SnapshotMetadata, SqliteGraphError> {
        (*self).snapshot_export(export_dir)
    }

    fn snapshot_import(
        &self,
        import_dir: &std::path::Path,
    ) -> Result<ImportMetadata, SqliteGraphError> {
        (*self).snapshot_import(import_dir)
    }

    fn kv_get(
        &self,
        snapshot_id: SnapshotId,
        key: &[u8],
    ) -> Result<Option<crate::backend::native::types::KvValue>, SqliteGraphError> {
        (*self).kv_get(snapshot_id, key)
    }

    fn kv_set(
        &self,
        key: Vec<u8>,
        value: crate::backend::native::types::KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), SqliteGraphError> {
        (*self).kv_set(key, value, ttl_seconds)
    }

    fn kv_delete(&self, key: &[u8]) -> Result<(), SqliteGraphError> {
        (*self).kv_delete(key)
    }

    fn subscribe(
        &self,
        filter: SubscriptionFilter,
    ) -> Result<(u64, std::sync::mpsc::Receiver<PubSubEvent>), SqliteGraphError> {
        (*self).subscribe(filter)
    }

    fn unsubscribe(&self, subscriber_id: u64) -> Result<bool, SqliteGraphError> {
        (*self).unsubscribe(subscriber_id)
    }

    fn kv_prefix_scan(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, crate::backend::native::types::KvValue)>, SqliteGraphError> {
        (*self).kv_prefix_scan(snapshot_id, prefix)
    }

    fn query_nodes_by_kind(
        &self,
        snapshot_id: SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).query_nodes_by_kind(snapshot_id, kind)
    }

    fn query_nodes_by_name_pattern(
        &self,
        snapshot_id: SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        (*self).query_nodes_by_name_pattern(snapshot_id, pattern)
    }

    fn match_triples(&self, pattern: &PatternTriple) -> Result<Vec<TripleMatch>, SqliteGraphError> {
        (*self).match_triples(pattern)
    }

    fn vector_search(
        &self,
        snapshot: SnapshotId,
        index_name: &str,
        vector: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        (*self).vector_search(snapshot, index_name, vector, k)
    }

    fn get_graph_ref(&self) -> Option<&crate::graph::SqliteGraph> {
        (*self).get_graph_ref()
    }
}

/// AsyncGraphBackend defines non-blocking graph operations.
/// Supports async/await natively using Rust 2024 edition features.
pub trait AsyncGraphBackend: Send + Sync {
    /// Retrieve a node asynchronously from the snapshot.
    fn get_node(
        &self,
        snapshot_id: SnapshotId,
        id: i64,
    ) -> impl std::future::Future<Output = Result<GraphEntity, SqliteGraphError>> + Send;

    /// Retrieve neighbor node IDs for a given source node asynchronously.
    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> impl std::future::Future<Output = Result<Vec<i64>, SqliteGraphError>> + Send;
}
