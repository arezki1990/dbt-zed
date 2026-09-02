//! Edge CRUD operations for SqliteGraph.

use rusqlite::params;

use crate::errors::SqliteGraphError;

use super::{
    SqliteGraph,
    types::{GraphEdge, row_to_edge, validate_edge},
};

impl SqliteGraph {
    pub fn insert_edge(&self, edge: &GraphEdge) -> Result<i64, SqliteGraphError> {
        validate_edge(edge)?;
        if !self.entity_exists(edge.from_id)? || !self.entity_exists(edge.to_id)? {
            return Err(SqliteGraphError::invalid_input(
                "edge endpoints must reference existing entities",
            ));
        }
        let data = serde_json::to_string(&edge.data)
            .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
        self.connection()
            .execute(
                "INSERT INTO graph_edges(from_id, to_id, edge_type, data) VALUES(?1, ?2, ?3, ?4)",
                params![edge.from_id, edge.to_id, edge.edge_type.as_str(), data],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        self.invalidate_caches();
        self.bump_authoritative_version()?;
        Ok(self.connection().last_insert_rowid())
    }

    /// Insert many edges atomically inside a single transaction.
    ///
    /// Returns the rowids of the inserted edges in the same order as the
    /// input. Empty input returns an empty vector without opening a
    /// transaction. On any error, the transaction is rolled back and the
    /// database is left untouched.
    pub fn insert_edges_bulk(&self, edges: &[GraphEdge]) -> Result<Vec<i64>, SqliteGraphError> {
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
        let conn = self.connection();
        conn.underlying()
            .execute_batch("BEGIN")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let mut ids = Vec::with_capacity(edges.len());
        let insert_result: Result<(), SqliteGraphError> = (|| {
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO graph_edges(from_id, to_id, edge_type, data) VALUES(?1, ?2, ?3, ?4)",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            for edge in edges {
                let data = serde_json::to_string(&edge.data)
                    .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                stmt.execute(params![
                    edge.from_id,
                    edge.to_id,
                    edge.edge_type.as_str(),
                    data,
                ])
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                ids.push(conn.last_insert_rowid());
            }
            Ok(())
        })();

        match insert_result {
            Ok(()) => {
                conn.underlying()
                    .execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                self.invalidate_caches();
                self.bump_authoritative_version()?;
                Ok(ids)
            }
            Err(err) => {
                let _ = conn.underlying().execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    pub fn get_edge(&self, id: i64) -> Result<GraphEdge, SqliteGraphError> {
        self.connection()
            .query_row(
                "SELECT id, from_id, to_id, edge_type, data FROM graph_edges WHERE id=?1",
                params![id],
                row_to_edge,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    SqliteGraphError::not_found(format!("edge {id}"))
                }
                other => SqliteGraphError::query(other.to_string()),
            })
    }

    pub fn delete_edge(&self, id: i64) -> Result<(), SqliteGraphError> {
        let affected = self
            .connection()
            .execute("DELETE FROM graph_edges WHERE id=?1", params![id])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        if affected == 0 {
            return Err(SqliteGraphError::not_found(format!("edge {id}")));
        }
        self.invalidate_caches();
        self.bump_authoritative_version()?;
        Ok(())
    }
}
