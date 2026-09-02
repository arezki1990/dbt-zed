//! SQLite-specific helper functions for backend operations.
//!
//! This module contains helper functions and optimized query implementations
//! that are specific to the SQLite backend implementation.

use rusqlite::params;

use crate::{
    SqliteGraphError,
    backend::sqlite::types::{BackendDirection, NeighborQuery},
};

/// Trait providing helper methods for SqliteGraphBackend.
pub trait SqliteGraphBackendHelpers {
    /// Execute optimized neighbor queries based on direction and edge type filtering.
    fn query_neighbors(
        &self,
        node: i64,
        direction: BackendDirection,
        edge_type: &Option<String>,
    ) -> Result<Vec<i64>, SqliteGraphError>;
}

/// Implementation of helper methods for SqliteGraphBackend.
impl SqliteGraphBackendHelpers for crate::backend::sqlite::impl_::SqliteGraphBackend
{
    fn query_neighbors(
        &self,
        node: i64,
        direction: BackendDirection,
        edge_type: &Option<String>,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        let graph = self.as_ref();
        match (direction, edge_type) {
            (BackendDirection::Outgoing, None) => graph.fetch_outgoing(node),
            (BackendDirection::Incoming, None) => graph.fetch_incoming(node),
            (BackendDirection::Outgoing, Some(edge_type)) => {
                let conn = graph.connection();
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
                let conn = graph.connection();
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