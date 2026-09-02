use rusqlite::{OptionalExtension, params};

use crate::{backend::BackendDirection, errors::SqliteGraphError, graph::SqliteGraph};

/// Check if an edge exists between two nodes with specified type.
///
/// This validates cache data against the authoritative SQL source.
pub fn edge_exists_with_type(
    graph: &SqliteGraph,
    from_id: i64,
    to_id: i64,
    edge_type: &str,
    _direction: BackendDirection,
) -> Result<bool, SqliteGraphError> {
    let conn = graph.connection();

    // Always use the actual database direction (from_id -> to_id)
    let sql =
        "SELECT 1 FROM graph_edges WHERE from_id = ?1 AND to_id = ?2 AND edge_type = ?3 LIMIT 1";

    let exists: Option<i32> = conn
        .query_row(sql, params![from_id, to_id, edge_type], |row| row.get(0))
        .optional()
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;

    Ok(exists.is_some())
}

/// Get the edge ID for a specific edge.
///
/// Retrieves the actual edge ID from SQL for accurate results.
pub fn get_edge_id(
    graph: &SqliteGraph,
    from_id: i64,
    to_id: i64,
    edge_type: &str,
) -> Result<i64, SqliteGraphError> {
    let conn = graph.connection();

    let edge_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_edges WHERE from_id = ?1 AND to_id = ?2 AND edge_type = ?3 ORDER BY id LIMIT 1",
            params![from_id, to_id, edge_type],
            |row| row.get(0),
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;

    Ok(edge_id)
}
