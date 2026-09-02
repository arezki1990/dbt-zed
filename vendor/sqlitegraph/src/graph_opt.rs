use ahash::AHashSet;
use serde_json::Value;

use crate::{
    SqliteGraphError,
    cache::CacheStats,
    fault_injection::{self, FaultPoint},
    graph::{ConnectionWrapper, SqliteGraph},
};

#[derive(Clone, Debug)]
pub struct GraphEntityCreate {
    pub kind: String,
    pub name: String,
    pub file_path: Option<String>,
    pub data: Value,
}

#[derive(Clone, Debug)]
pub struct GraphEdgeCreate {
    pub from_id: i64,
    pub to_id: i64,
    pub edge_type: String,
    pub data: Value,
}

/// Transaction safety wrapper for automatic rollback on errors
///
/// This wrapper holds onto a connection wrapper for the duration of a transaction.
/// Note: Due to lifetime constraints, this is typically used within a function scope.
pub struct TransactionGuard<'a> {
    conn: ConnectionWrapper<'a>,
    committed: bool,
}

impl<'a> TransactionGuard<'a> {
    /// Start a new transaction with IMMEDIATE mode for better write performance
    pub fn new(conn: ConnectionWrapper<'a>) -> Result<Self, SqliteGraphError> {
        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        Ok(Self {
            conn,
            committed: false,
        })
    }

    /// Commit the transaction with cache invalidation and snapshot update
    pub fn commit(mut self, graph: &SqliteGraph) -> Result<(), SqliteGraphError> {
        self.conn
            .execute("COMMIT", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        graph.invalidate_caches();
        graph.update_snapshot();
        self.committed = true;
        Ok(())
    }

    /// Get reference to the underlying connection wrapper
    pub fn conn(&self) -> &ConnectionWrapper<'a> {
        &self.conn
    }

    /// Execute a function with automatic rollback on error
    pub fn execute<F, R>(mut self, graph: &SqliteGraph, f: F) -> Result<R, SqliteGraphError>
    where
        F: FnOnce(&ConnectionWrapper<'a>) -> Result<R, SqliteGraphError>,
    {
        match f(&self.conn) {
            Ok(result) => {
                self.commit(graph)?;
                Ok(result)
            }
            Err(err) => {
                // Don't rollback here - Drop will handle it automatically
                self.committed = false; // Ensure Drop knows to rollback
                Err(err)
            }
        }
    }
}

impl<'a> Drop for TransactionGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            // Auto-rollback if not explicitly committed
            let _ = self.conn.execute("ROLLBACK", []);
        }
    }
}

/// Configuration for batch operations
pub struct BatchConfig {
    pub max_batch_size: usize,
    pub enable_chunking: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 1000, // Conservative default for WAL mode
            enable_chunking: true,
        }
    }
}

/// Execute a batch operation with automatic chunking for large datasets
pub fn execute_batch<T, F, R>(
    items: &[T],
    config: &BatchConfig,
    mut operation: F,
) -> Result<Vec<R>, SqliteGraphError>
where
    F: FnMut(&[T]) -> Result<Vec<R>, SqliteGraphError>,
{
    if !config.enable_chunking || items.len() <= config.max_batch_size {
        return operation(items);
    }

    let mut all_results = Vec::with_capacity(items.len());

    // Process in deterministic chunks to maintain ordering
    for chunk in items.chunks(config.max_batch_size) {
        let chunk_results = operation(chunk)?;
        all_results.extend(chunk_results);
    }

    Ok(all_results)
}

pub fn bulk_insert_entities(
    graph: &SqliteGraph,
    entries: &[GraphEntityCreate],
) -> Result<Vec<i64>, SqliteGraphError> {
    bulk_insert_entities_with_config(graph, entries, &BatchConfig::default())
}

pub fn bulk_insert_entities_with_config(
    graph: &SqliteGraph,
    entries: &[GraphEntityCreate],
    config: &BatchConfig,
) -> Result<Vec<i64>, SqliteGraphError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    execute_batch(entries, config, |chunk| {
        let conn = graph.connection();
        TransactionGuard::new(conn)?.execute(graph, |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO graph_entities(kind,name,file_path,data) VALUES(?1,?2,?3,?4)",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            let mut ids = Vec::new();
            for entry in chunk {
                validate_entity_create(entry)?;
                let payload = serde_json::to_string(&entry.data)
                    .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                stmt.execute(rusqlite::params![
                    entry.kind,
                    entry.name,
                    entry.file_path,
                    payload
                ])
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                ids.push(conn.last_insert_rowid());
            }

            // Check for fault injection before commit
            fault_injection::check_fault(FaultPoint::BulkInsertEntitiesBeforeCommit)?;
            Ok(ids)
        })
    })
}

pub fn bulk_insert_edges(
    graph: &SqliteGraph,
    entries: &[GraphEdgeCreate],
) -> Result<Vec<i64>, SqliteGraphError> {
    bulk_insert_edges_with_config(graph, entries, &BatchConfig::default())
}

pub fn bulk_insert_edges_with_config(
    graph: &SqliteGraph,
    entries: &[GraphEdgeCreate],
    config: &BatchConfig,
) -> Result<Vec<i64>, SqliteGraphError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    execute_batch(entries, config, |chunk| {
        let conn = graph.connection();
        TransactionGuard::new(conn)?.execute(graph, |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO graph_edges(from_id,to_id,edge_type,data) VALUES(?1,?2,?3,?4)",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            let mut ids = Vec::new();
            let mut seen = AHashSet::new();
            for entry in chunk {
                validate_edge_create(entry)?;
                if !seen.insert((entry.from_id, entry.to_id, entry.edge_type.clone())) {
                    continue;
                }
                validate_endpoints_exist(conn, entry.from_id, entry.to_id)?;
                let payload = serde_json::to_string(&entry.data)
                    .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                stmt.execute(rusqlite::params![
                    entry.from_id,
                    entry.to_id,
                    entry.edge_type,
                    payload
                ])
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                ids.push(conn.last_insert_rowid());
            }

            // Check for fault injection before commit
            fault_injection::check_fault(FaultPoint::BulkInsertEdgesBeforeCommit)?;
            Ok(ids)
        })
    })
}

pub fn adjacency_fetch_outgoing_batch(
    graph: &SqliteGraph,
    ids: &[i64],
) -> Result<Vec<(i64, Vec<i64>)>, SqliteGraphError> {
    let mut results = Vec::new();
    for &id in ids {
        results.push((id, graph.fetch_outgoing(id)?));
    }
    results.sort_by_key(|a| a.0);
    Ok(results)
}

pub fn adjacency_fetch_incoming_batch(
    graph: &SqliteGraph,
    ids: &[i64],
) -> Result<Vec<(i64, Vec<i64>)>, SqliteGraphError> {
    let mut results = Vec::new();
    for &id in ids {
        results.push((id, graph.fetch_incoming(id)?));
    }
    results.sort_by_key(|a| a.0);
    Ok(results)
}

pub fn cache_clear_ranges(graph: &SqliteGraph, ids: &[i64]) {
    for &id in ids {
        graph.outgoing_cache_ref().remove(id);
        graph.incoming_cache_ref().remove(id);
    }
}

pub fn cache_stats(graph: &SqliteGraph) -> CacheStats {
    let outgoing = graph.outgoing_cache_ref().stats();
    let incoming = graph.incoming_cache_ref().stats();
    CacheStats {
        hits: outgoing.hits + incoming.hits,
        misses: outgoing.misses + incoming.misses,
        entries: outgoing.entries + incoming.entries,
    }
}

fn validate_entity_create(entry: &GraphEntityCreate) -> Result<(), SqliteGraphError> {
    if entry.kind.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("entity kind must be set"));
    }
    if entry.name.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("entity name must be set"));
    }
    Ok(())
}

fn validate_edge_create(entry: &GraphEdgeCreate) -> Result<(), SqliteGraphError> {
    if entry.edge_type.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("edge type must be set"));
    }
    if entry.from_id <= 0 || entry.to_id <= 0 {
        return Err(SqliteGraphError::invalid_input(
            "edge endpoints must be positive ids",
        ));
    }
    Ok(())
}

fn validate_endpoints_exist(
    conn: &ConnectionWrapper<'_>,
    from: i64,
    to: i64,
) -> Result<(), SqliteGraphError> {
    let mut stmt = conn
        .prepare_cached("SELECT COUNT(1) FROM graph_entities WHERE id IN (?1, ?2)")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let count: i64 = stmt
        .query_row(rusqlite::params![from, to], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    if count < 2 {
        return Err(SqliteGraphError::invalid_input("edge endpoints must exist"));
    }
    Ok(())
}
