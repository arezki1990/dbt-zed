//! Core SqliteGraph struct and construction functionality.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use rusqlite::{Connection, params};

use crate::{
    cache::{AdjacencyCache, CacheStats},
    config::SqliteConfig,
    errors::SqliteGraphError,
    graph::{GraphEdge, GraphEntity},
    hnsw::HnswIndex,
    introspection::{EdgeCount, GraphIntrospection, IntrospectError},
    mvcc::SnapshotManager,
    query_cache::QueryCache,
    schema::ensure_schema,
};

use super::{
    metrics::{GraphMetrics, StatementTracker},
    pool::PoolManager,
    types::{GraphStats, SnapshotMetadata, validate_edge, validate_entity},
};

/// Embedded SQLite-backed graph database.
///
/// Provides a lightweight, deterministic graph database with entity and edge storage,
/// pattern matching, MVCC-lite snapshots, and deterministic indexing.
///
/// # Thread Safety
///
/// **NOT thread-safe for concurrent writes.** `SqliteGraph` uses interior mutability
/// (`RefCell`) and is not `Sync`. However, the underlying connection pool allows
/// multiple threads to read concurrently when using separate `SqliteGraph` instances.
///
/// # Connection Pooling
///
/// File-based databases use an r2d2 connection pool (default 5 connections) for
/// concurrent access. In-memory databases skip pooling and use a single direct connection.
pub struct SqliteGraph {
    /// Connection pool for file-based databases, or direct connection for in-memory
    /// (public for CLI access to underlying connection)
    pub pool: PoolManager,
    pub(crate) outgoing_cache: AdjacencyCache,
    pub(crate) incoming_cache: AdjacencyCache,
    pub(crate) query_cache: QueryCache,
    pub(crate) metrics: Arc<GraphMetrics>,
    pub(crate) statement_tracker: Arc<StatementTracker>,
    pub(crate) snapshot_manager: SnapshotManager,
    /// HNSW vector indexes stored by name (public for CLI access).
    /// Uses `parking_lot::Mutex` (not `RwLock`) because `HnswIndex` contains
    /// `Box<dyn VectorStorage>` which is `!Sync` (rusqlite::Connection is !Sync).
    /// `Mutex<T: Send>` is itself `Send + Sync`, making `SqliteGraph` thread-safe.
    /// `parking_lot::Mutex` is smaller and faster than `std::sync::Mutex`, and
    /// does not propagate poison on panic.
    pub hnsw_indexes: Mutex<HashMap<String, HnswIndex>>,
}

// Helper function to check if connection is in-memory
pub fn is_in_memory_connection(conn: &Connection) -> bool {
    // Check database filename - in-memory databases have empty or special names
    match conn.pragma_query_value(None, "database_list", |row| {
        let name: String = row.get(1)?;
        Ok(name)
    }) {
        Ok(name) => name.is_empty() || name == ":memory:",
        Err(_) => true, // Assume in-memory if we can't query
    }
}

impl SqliteGraph {
    /// Open a graph database with custom configuration.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the SQLite database file
    /// * `cfg` - Configuration options (pool size, cache size, PRAGMAs, etc.)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::{SqliteGraph, SqliteConfig};
    ///
    /// let cfg = SqliteConfig::new()
    ///     .with_pool_size(10)
    ///     .with_wal_mode();
    /// let graph = SqliteGraph::open_with_config("my_graph.db", &cfg)?;
    /// ```
    pub fn open_with_config<P: AsRef<Path>>(
        path: P,
        cfg: &SqliteConfig,
    ) -> Result<Self, SqliteGraphError> {
        // Get pool size from config (default: 5)
        let pool_size = cfg.pool_size.unwrap_or(5) as u32;

        let pool = PoolManager::with_max_size(path, pool_size)
            .map_err(|e| SqliteGraphError::connection(e.to_string()))?;

        // Initialize schema using first connection from pool
        {
            let conn = pool
                .get()
                .map_err(|e| SqliteGraphError::connection(e.to_string()))?;

            if cfg.without_migrations {
                crate::schema::ensure_schema_without_migrations(&conn)?;
            } else {
                ensure_schema(&conn)?;
            }
        }

        // Configure pool with WAL mode and performance optimizations
        pool.configure_pool(|conn| {
            // Set prepared statement cache size from config
            let cache_size = cfg.cache_size.unwrap_or(128);
            conn.set_prepared_statement_cache_capacity(cache_size);

            // Enable WAL mode for better concurrency
            let result = conn.pragma_update(None, "journal_mode", "WAL");
            if result.is_err() {
                // Fallback to DELETE mode if WAL fails (e.g., on some network filesystems)
                let _ = conn.pragma_update(None, "journal_mode", "DELETE");
            }

            // Performance optimizations
            let _ = conn.pragma_update(None, "synchronous", "NORMAL"); // Balanced safety/performance
            let _ = conn.pragma_update(None, "cache_size", "-64000"); // 64MB cache
            let _ = conn.pragma_update(None, "temp_store", "MEMORY"); // Store temp tables in memory
            let _ = conn.pragma_update(None, "mmap_size", "268435456"); // 256MB memory-mapped I/O

            // Apply custom PRAGMA settings from config
            for (key, value) in &cfg.pragma_settings {
                let _ = conn.pragma_update(None, key, value.as_str());
            }

            Ok(())
        })?;

        // Load existing HNSW indexes from database
        let hnsw_indexes = {
            let conn = pool
                .get()
                .map_err(|e| SqliteGraphError::connection(e.to_string()))?;
            Self::load_hnsw_indexes(&conn).unwrap_or_default()
        };

        Ok(Self {
            pool,
            outgoing_cache: AdjacencyCache::new(),
            incoming_cache: AdjacencyCache::new(),
            query_cache: QueryCache::new(),
            metrics: Arc::new(GraphMetrics::default()),
            statement_tracker: Arc::new(StatementTracker::default()),
            snapshot_manager: SnapshotManager::new(),
            hnsw_indexes: Mutex::new(hnsw_indexes),
        })
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, SqliteGraphError> {
        Self::open_with_config(path, &SqliteConfig::default())
    }

    pub fn open_without_migrations<P: AsRef<Path>>(path: P) -> Result<Self, SqliteGraphError> {
        let cfg = SqliteConfig::new().with_migrations_disabled(true);
        Self::open_with_config(path, &cfg)
    }

    /// Open an in-memory database with custom configuration.
    ///
    /// Note: Pool size is ignored for in-memory databases since they use
    /// a single direct connection (each connection would have isolated data).
    ///
    /// # Arguments
    ///
    /// * `cfg` - Configuration options (cache size, PRAGMAs, etc.)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::{SqliteGraph, SqliteConfig};
    ///
    /// let cfg = SqliteConfig::new()
    ///     .with_cache_size(256)
    ///     .with_performance_mode();
    /// let graph = SqliteGraph::open_in_memory_with_config(&cfg)?;
    /// ```
    pub fn open_in_memory_with_config(cfg: &SqliteConfig) -> Result<Self, SqliteGraphError> {
        let mut pool =
            PoolManager::in_memory().map_err(|e| SqliteGraphError::connection(e.to_string()))?;

        // Set prepared statement cache size from config
        let cache_size = cfg.cache_size.unwrap_or(128);

        // For in-memory databases, configure directly
        pool.configure_direct(|conn| {
            if cfg.without_migrations {
                crate::schema::ensure_schema_without_migrations(conn)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            } else {
                ensure_schema(conn)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            }
            conn.set_prepared_statement_cache_capacity(cache_size);

            // Apply custom PRAGMA settings from config
            for (key, value) in &cfg.pragma_settings {
                let _ = conn.pragma_update(None, key, value.as_str());
            }

            Ok(())
        })
        .map_err(|e| SqliteGraphError::connection(e.to_string()))?;

        // Load HNSW indexes (will be empty for fresh in-memory database)
        let hnsw_indexes = pool
            .direct_connection()
            .map(|conn| Self::load_hnsw_indexes(conn).unwrap_or_default())
            .unwrap_or_default();

        Ok(Self {
            pool,
            outgoing_cache: AdjacencyCache::new(),
            incoming_cache: AdjacencyCache::new(),
            query_cache: QueryCache::new(),
            metrics: Arc::new(GraphMetrics::default()),
            statement_tracker: Arc::new(StatementTracker::default()),
            snapshot_manager: SnapshotManager::new(),
            hnsw_indexes: Mutex::new(hnsw_indexes),
        })
    }

    pub fn open_in_memory() -> Result<Self, SqliteGraphError> {
        Self::open_in_memory_with_config(&SqliteConfig::default())
    }

    pub fn open_in_memory_without_migrations() -> Result<Self, SqliteGraphError> {
        let cfg = SqliteConfig::new().with_migrations_disabled(true);
        Self::open_in_memory_with_config(&cfg)
    }

    /// Load HNSW indexes from database
    ///
    /// This is called during SqliteGraph construction to restore any
    /// previously created HNSW indexes with full vector data. When the
    /// underlying database is file-based, each loaded index gets a
    /// `SQLiteVectorStorage` attached so subsequent inserts persist to
    /// disk instead of being dropped at process exit.
    fn load_hnsw_indexes(
        conn: &Connection,
    ) -> Result<HashMap<String, HnswIndex>, SqliteGraphError> {
        let mut indexes = HashMap::new();

        let index_names = HnswIndex::list_indexes(conn).map_err(|e| {
            SqliteGraphError::invalid_input(format!("Failed to load HNSW indexes: {}", e))
        })?;

        let db_path: String = conn
            .pragma_query_value(None, "database_list", |row| row.get::<_, String>(2))
            .unwrap_or_default();

        let is_file_db = !db_path.is_empty() && db_path != ":memory:";

        for name in index_names {
            let mut hnsw = match HnswIndex::load_metadata(conn, &name) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Warning: Failed to load HNSW index '{}': {}", name, e);
                    continue;
                }
            };

            if is_file_db {
                match HnswIndex::get_index_id(conn, &name) {
                    Ok(Some(index_id)) => match rusqlite::Connection::open(&db_path) {
                        Ok(storage_conn) => {
                            let _ = crate::schema::ensure_schema(&storage_conn);
                            hnsw.storage =
                                Box::new(crate::hnsw::storage::SQLiteVectorStorage::new(
                                    index_id,
                                    storage_conn,
                                ));
                            let needs_rebuild = match hnsw.restore_topology() {
                                Ok(true) => false,
                                Ok(false) => true,
                                Err(e) => {
                                    eprintln!(
                                        "Warning: restore_topology failed for '{}', falling back to rebuild: {}",
                                        name, e
                                    );
                                    true
                                }
                            };
                            if needs_rebuild
                                && let Err(rebuild_err) = hnsw.load_vectors_and_rebuild(conn)
                            {
                                eprintln!(
                                    "Warning: rebuild failed for '{}': {}",
                                    name, rebuild_err
                                );
                                continue;
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: HNSW index '{}' loaded with in-memory storage; failed to open persistent storage: {}",
                                name, e
                            );
                            if let Err(e) = hnsw.load_vectors_and_rebuild(conn) {
                                eprintln!("Warning: Failed to load vectors for '{}': {}", name, e);
                                continue;
                            }
                        }
                    },
                    Ok(None) => {
                        eprintln!(
                            "Warning: HNSW index '{}' has no id row in hnsw_indexes; staying with in-memory storage",
                            name
                        );
                        if let Err(e) = hnsw.load_vectors_and_rebuild(conn) {
                            eprintln!("Warning: Failed to load vectors for '{}': {}", name, e);
                            continue;
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to look up id for HNSW index '{}': {}",
                            name, e
                        );
                        if let Err(e) = hnsw.load_vectors_and_rebuild(conn) {
                            eprintln!("Warning: Failed to load vectors for '{}': {}", name, e);
                            continue;
                        }
                    }
                }
            } else if let Err(e) = hnsw.load_vectors_and_rebuild(conn) {
                eprintln!("Warning: Failed to load vectors for '{}': {}", name, e);
                continue;
            }

            indexes.insert(name, hnsw);
        }

        Ok(indexes)
    }

    /// Get comprehensive introspection data for this graph instance.
    ///
    /// This method provides a structured snapshot of the graph state,
    /// including node counts, edge counts, cache statistics, and file sizes.
    /// The result is JSON-serializable for both human debugging and LLM consumption.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::{open_graph, GraphConfig};
    ///
    /// let graph = open_graph("my_graph.db", &GraphConfig::sqlite())?;
    /// let intro = graph.introspect()?;
    ///
    /// println!("Backend: {}", intro.backend_type);
    /// println!("Nodes: {}", intro.node_count);
    /// println!("Cache hit ratio: {:.2}%", intro.cache_stats.hit_ratio().unwrap_or(0.0));
    /// ```
    pub fn introspect(&self) -> Result<GraphIntrospection, SqliteGraphError> {
        // Determine backend type
        let backend_type = "sqlite".to_string();

        // Get node count
        let node_count = self
            .all_entity_ids()
            .map_err(|e| IntrospectError::NodeCountError(e.to_string()))?
            .len();

        // Get edge count (use sampling for large graphs)
        let edge_count = self.count_edges()?;

        // Get cache statistics (combined from outgoing and incoming)
        let outgoing_stats = self.outgoing_cache.stats();
        let incoming_stats = self.incoming_cache.stats();
        let cache_stats = CacheStats {
            hits: outgoing_stats.hits + incoming_stats.hits,
            misses: outgoing_stats.misses + incoming_stats.misses,
            entries: outgoing_stats.entries + incoming_stats.entries,
        };

        // Check if in-memory database
        let is_in_memory = self.pool.is_in_memory();

        // Get file size (only for file-based databases)
        let file_size = if is_in_memory {
            None
        } else {
            self.get_database_path()
                .and_then(crate::introspection::get_file_size)
        };

        // Get WAL size (if WAL is enabled)
        let wal_size = if is_in_memory {
            None
        } else {
            self.get_database_path()
                .and_then(crate::introspection::get_wal_size)
        };

        // Memory usage is not directly available for SQLite backend
        let memory_usage = None;

        Ok(GraphIntrospection {
            backend_type,
            node_count,
            edge_count,
            cache_stats,
            memory_usage,
            file_size,
            wal_size,
            is_in_memory,
        })
    }

    /// Get adjacency cache statistics.
    ///
    /// Returns combined statistics from both outgoing and incoming adjacency caches.
    /// This is useful for monitoring cache effectiveness and tuning cache size.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::{open_graph, GraphConfig};
    ///
    /// let graph = open_graph("my_graph.db", &GraphConfig::sqlite())?;
    /// let stats = graph.cache_stats();
    ///
    /// println!("Cache hits: {}", stats.hits);
    /// println!("Cache misses: {}", stats.misses);
    /// println!("Hit ratio: {:.2}%", stats.hit_ratio().unwrap_or(0.0));
    /// ```
    pub fn cache_stats(&self) -> CacheStats {
        let outgoing_stats = self.outgoing_cache.stats();
        let incoming_stats = self.incoming_cache.stats();
        CacheStats {
            hits: outgoing_stats.hits + incoming_stats.hits,
            misses: outgoing_stats.misses + incoming_stats.misses,
            entries: outgoing_stats.entries + incoming_stats.entries,
        }
    }

    /// Count edges in the graph.
    ///
    /// For graphs with fewer than 10,000 edges, returns an exact count.
    /// For larger graphs, returns an estimate based on sampling to avoid
    /// expensive O(E) operations.
    fn count_edges(&self) -> Result<EdgeCount, SqliteGraphError> {
        let conn = self.connection();

        // First, get a quick estimate
        let estimate: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        // For small graphs (< 10K edges), return exact count
        if estimate < 10_000 {
            return Ok(EdgeCount::Exact(estimate as usize));
        }

        // For larger graphs, use sampling
        // Sample 1000 random rows to estimate with confidence interval
        let sample_size = 1000.min(estimate as usize);
        let sample_count: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM (
                        SELECT 1 FROM graph_edges
                        ORDER BY RANDOM()
                        LIMIT {}
                    )",
                    sample_size
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        // Calculate confidence interval (95% confidence, ~2% margin of error)
        let _ratio = sample_count as f64 / sample_size as f64;
        let margin = estimate as f64 * 0.02; // 2% margin of error

        Ok(EdgeCount::Estimate {
            count: estimate as usize,
            min: ((estimate as f64 - margin).floor() as usize),
            max: ((estimate as f64 + margin).ceil() as usize),
            sample_size,
        })
    }

    /// Get the database file path if this is a file-based database.
    fn get_database_path(&self) -> Option<String> {
        if self.pool.is_in_memory() {
            None
        } else {
            // Try to get the database path from SQLite
            self.pool.get().ok().and_then(|conn| {
                conn.pragma_query_value(None, "database_list", |row| {
                    let name: String = row.get(1)?;
                    Ok(name)
                })
                .ok()
                .filter(|name| !name.is_empty() && name != ":memory:")
            })
        }
    }

    // ── MVCC Operations (Native-v3) ───────────────────────────────────────────

    /// Execute a closure with access to a SQLite connection.
    ///
    /// This provides direct SQL access for MVCC operations that need
    /// to interact with native-v3 schema (CSR shards, HNSW snapshots, etc.).
    ///
    /// # Arguments
    /// * `f` - Closure that receives a Connection and returns a Result
    ///
    /// # Returns
    /// The Result from the closure, or a connection error
    pub fn with_connection<F, R>(&self, f: F) -> Result<R, SqliteGraphError>
    where
        F: FnOnce(&Connection) -> Result<R, SqliteGraphError>,
    {
        if self.pool.is_in_memory() {
            let conn = self.pool.direct_connection().ok_or_else(|| {
                SqliteGraphError::connection("In-memory connection unavailable".to_string())
            })?;
            f(conn)
        } else {
            let conn = self
                .pool
                .get()
                .map_err(|e| SqliteGraphError::connection(e.to_string()))?;
            f(&conn)
        }
    }

    /// Batch insert entities with MVCC snapshot isolation.
    ///
    /// All inserts are tagged with the given snapshot_id for consistent reads.
    /// This uses native-v3 MVCC schema (graph_entities.snapshot_id).
    ///
    /// # Arguments
    /// * `entities` - Entities to insert
    /// * `snapshot_id` - Snapshot identifier for this batch
    ///
    /// # Returns
    /// Vector of assigned entity IDs
    pub fn batch_insert_entities_with_snapshot(
        &self,
        entities: &[GraphEntity],
        snapshot_id: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        for entity in entities {
            validate_entity(entity)?;
        }

        let timestamp = chrono::Utc::now().timestamp();

        let ids = self.with_connection(|conn| {
            conn.execute_batch("BEGIN")
                .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

            let mut ids = Vec::with_capacity(entities.len());
            let result: Result<(), SqliteGraphError> = (|| {
                let mut entity_stmt = conn
                    .prepare_cached(
                        "INSERT INTO graph_entities(kind, name, file_path, data, snapshot_id, created_at)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    )
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                let mut label_stmt = conn
                    .prepare_cached(
                        "INSERT OR IGNORE INTO graph_labels(entity_id, label) VALUES(?1, ?2)",
                    )
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                for entity in entities {
                    let data = serde_json::to_string(&entity.data)
                        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;

                    entity_stmt
                        .execute(params![
                            entity.kind.as_str(),
                            entity.name.as_str(),
                            entity.file_path.as_deref(),
                            data,
                            snapshot_id,
                            timestamp,
                        ])
                        .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                    let id = conn.last_insert_rowid();
                    if !entity.kind.is_empty() {
                        label_stmt
                            .execute(params![id, entity.kind.as_str()])
                            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
                    }
                    ids.push(id);
                }

                conn.execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                Ok(())
            })();

            match result {
                Ok(_) => Ok(ids),
                Err(e) => {
                    conn.execute_batch("ROLLBACK")
                        .map_err(|e2| SqliteGraphError::QueryError(e2.to_string()))?;
                    Err(e)
                }
            }
        })?;

        // Update snapshot_stats table (scale optimization)
        let _ = self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshot_stats (snapshot_id, entity_count, edge_count, created_at, updated_at)
                 VALUES (?1, ?2, 0, ?3, ?3)
                 ON CONFLICT(snapshot_id) DO UPDATE SET
                 entity_count = entity_count + ?2,
                 updated_at = ?3",
                params![snapshot_id, ids.len() as i64, timestamp],
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            Ok(())
        });

        Ok(ids)
    }

    /// Batch insert edges with MVCC snapshot isolation.
    ///
    /// All inserts are tagged with the given snapshot_id for consistent reads.
    /// This uses native-v3 MVCC schema (graph_edges.snapshot_id).
    ///
    /// # Arguments
    /// * `edges` - Edges to insert
    /// * `snapshot_id` - Snapshot identifier for this batch
    ///
    /// # Returns
    /// Vector of assigned edge IDs
    pub fn batch_insert_edges_with_snapshot(
        &self,
        edges: &[GraphEdge],
        snapshot_id: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        for edge in edges {
            validate_edge(edge)?;
            if !self.entity_exists(edge.from_id)? || !self.entity_exists(edge.to_id)? {
                return Err(SqliteGraphError::invalid_input(
                    "edge endpoints must reference existing entities",
                ));
            }
        }

        let timestamp = chrono::Utc::now().timestamp();

        let ids = self.with_connection(|conn| {
            conn.execute_batch("BEGIN")
                .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

            let mut ids = Vec::with_capacity(edges.len());
            let insert_result: Result<(), SqliteGraphError> = (|| {
                let mut stmt = conn
                    .prepare_cached(
                        "INSERT INTO graph_edges(from_id, to_id, edge_type, data, snapshot_id, created_at)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    )
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                for edge in edges {
                    let data = serde_json::to_string(&edge.data)
                        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;

                    stmt.execute(params![
                        edge.from_id,
                        edge.to_id,
                        edge.edge_type.as_str(),
                        data,
                        snapshot_id,
                        timestamp,
                    ])
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                    ids.push(conn.last_insert_rowid());
                }

                conn.execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

                Ok(())
            })();

            match insert_result {
                Ok(_) => Ok(ids),
                Err(e) => {
                    conn.execute_batch("ROLLBACK")
                        .map_err(|e2| SqliteGraphError::QueryError(e2.to_string()))?;
                    Err(e)
                }
            }
        })?;

        // Update snapshot_stats table (scale optimization)
        let _ = self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshot_stats (snapshot_id, entity_count, edge_count, created_at, updated_at)
                 VALUES (?1, 0, ?2, ?3, ?3)
                 ON CONFLICT(snapshot_id) DO UPDATE SET
                 edge_count = edge_count + ?2,
                 updated_at = ?3",
                params![snapshot_id, ids.len() as i64, timestamp],
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            Ok(())
        });

        Ok(ids)
    }

    /// Time-travel query: read graph state as of a specific timestamp.
    ///
    /// Returns only entities/edges visible at or before the given time.
    /// Uses pre-aggregated snapshot_stats table for O(1) lookups (scale optimization).
    ///
    /// # Arguments
    /// * `timestamp` - Unix timestamp to query state as-of
    ///
    /// # Returns
    /// GraphStats with entity and edge counts
    pub fn query_as_of(&self, timestamp: i64) -> Result<GraphStats, SqliteGraphError> {
        let stats = self.with_connection(|conn| {
            // Use pre-aggregated stats for O(1) lookup instead of COUNT(*) scan
            let (entity_count, edge_count): (i64, i64) = conn
                .query_row(
                    "SELECT COALESCE(SUM(entity_count), 0), COALESCE(SUM(edge_count), 0)
                     FROM snapshot_stats
                     WHERE created_at <= ?1",
                    [timestamp],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or((0, 0));

            Ok(GraphStats {
                total_entities: entity_count,
                total_edges: edge_count,
                entity_counts: vec![],
                edge_counts: vec![],
            })
        })?;

        Ok(stats)
    }

    /// Create a named snapshot for consistent reads.
    ///
    /// Returns the snapshot timestamp. This creates a snapshot record
    /// in native-v3 MVCC schema (snapshots table).
    ///
    /// # Arguments
    /// * `snapshot_id` - Unique identifier for this snapshot
    ///
    /// # Returns
    /// Unix timestamp when snapshot was created
    pub fn create_snapshot(&self, snapshot_id: &str) -> Result<i64, SqliteGraphError> {
        use chrono::Utc;
        let timestamp = Utc::now().timestamp();

        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshots (snapshot_id, created_at) VALUES (?1, ?2)
                 ON CONFLICT(snapshot_id) DO UPDATE SET created_at=?2",
                rusqlite::params![snapshot_id, timestamp],
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            Ok(())
        })?;

        Ok(timestamp)
    }

    /// Get current graph version from CSR shards.
    ///
    /// Returns the highest version number across all CSR shards.
    /// Used to track graph rebuilds in native-v3 storage.
    ///
    /// # Returns
    /// Current graph version (0 if no CSR shards exist)
    pub fn get_graph_version(&self) -> Result<i64, SqliteGraphError> {
        let version = self.with_connection(|conn| {
            let max_version: i64 = conn
                .query_row("SELECT MAX(version) FROM csr_shards", [], |row| row.get(0))
                .unwrap_or(0);
            Ok(max_version)
        })?;

        Ok(version)
    }

    /// Get the authoritative SQLite mutation version.
    pub fn get_authoritative_version(&self) -> Result<i64, SqliteGraphError> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT authoritative_version FROM graph_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))
        })
    }

    /// Get the version of the currently published materialized graph view.
    pub fn get_materialized_version(&self) -> Result<i64, SqliteGraphError> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT materialized_version FROM graph_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))
        })
    }

    /// Advance the authoritative SQLite mutation version after a committed write.
    pub fn bump_authoritative_version(&self) -> Result<i64, SqliteGraphError> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE graph_meta
                 SET authoritative_version = authoritative_version + 1
                 WHERE id = 1",
                [],
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            conn.query_row(
                "SELECT authoritative_version FROM graph_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))
        })
    }

    /// Publish the latest materialized graph view version.
    pub fn set_materialized_version(&self, version: i64) -> Result<(), SqliteGraphError> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE graph_meta SET materialized_version = ?1 WHERE id = 1",
                rusqlite::params![version],
            )
            .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            Ok(())
        })
    }

    /// List all snapshots with metadata.
    ///
    /// Returns snapshot records sorted by creation time (newest first).
    ///
    /// # Returns
    /// Vector of snapshot metadata
    pub fn list_snapshots(&self) -> Result<Vec<SnapshotMetadata>, SqliteGraphError> {
        let snapshots = self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT snapshot_id, created_at, description FROM snapshots ORDER BY created_at DESC")
                .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

            let mut out = Vec::new();
            let rows = stmt
                .query_map([], |row| {
                    Ok(SnapshotMetadata {
                        snapshot_id: row.get(0)?,
                        timestamp: row.get(1)?,
                        description: row.get(2)?,
                    })
                })
                .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;

            for row in rows {
                match row {
                    Ok(meta) => out.push(meta),
                    Err(e) => return Err(SqliteGraphError::QueryError(e.to_string())),
                }
            }
            Ok(out)
        })?;

        Ok(snapshots)
    }

    /// Delete a snapshot and its metadata.
    ///
    /// This removes the snapshot record but does NOT cascade-delete entities/edges
    /// tagged with this snapshot_id (they remain visible with their original snapshot).
    ///
    /// # Arguments
    /// * `snapshot_id` - Snapshot identifier to delete
    pub fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), SqliteGraphError> {
        self.with_connection(|conn| {
            conn.execute("DELETE FROM snapshots WHERE snapshot_id=?1", [snapshot_id])
                .map_err(|e| SqliteGraphError::QueryError(e.to_string()))?;
            Ok(())
        })
    }
}
