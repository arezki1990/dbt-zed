use super::V3Backend;
use crate::SqliteGraphError;
use rusqlite::ToSql;
use serde_json::Value;

impl V3Backend {
    pub(crate) fn persist_inserted_node_properties(
        &self,
        node_id: i64,
        kind: &str,
        name: &str,
        data: &Value,
        next_version: u64,
    ) -> Result<(), SqliteGraphError> {
        let data_json = serde_json::to_string(data).unwrap_or_default();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| {
                SqliteGraphError::connection(format!(
                    "System clock before UNIX epoch during node property insert: {}",
                    e
                ))
            })?
            .as_secs() as i64;
        let conn = self.sqlite_conn.lock();
        conn.execute(
            "INSERT INTO node_properties (node_id, kind, name, data, created_at, created_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [
                &node_id as &dyn ToSql,
                &kind as &dyn ToSql,
                &name as &dyn ToSql,
                &data_json as &dyn ToSql,
                &timestamp as &dyn ToSql,
                &next_version as &dyn ToSql,
            ],
        ).map_err(|e| SqliteGraphError::connection(format!("Failed to insert node properties: {}", e)))?;
        Ok(())
    }

    pub(crate) fn persist_inserted_edge_attributes(
        &self,
        from: i64,
        to: i64,
        edge_type: &str,
        data: &Value,
        next_version: u64,
    ) -> Result<(), SqliteGraphError> {
        let data_json = serde_json::to_string(data).unwrap_or_default();
        let conn = self.sqlite_conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO edge_attributes (src, dst, attr_name, attr_value, created_version) VALUES (?1, ?2, ?3, ?4, ?5)",
            [
                &from as &dyn ToSql,
                &to as &dyn ToSql,
                &edge_type as &dyn ToSql,
                &data_json as &dyn ToSql,
                &next_version as &dyn ToSql,
            ],
        ).map_err(|e| SqliteGraphError::connection(format!("Failed to insert edge attributes: {}", e)))?;
        Ok(())
    }

    pub(crate) fn persist_updated_node_properties(
        &self,
        node_id: i64,
        kind: &str,
        name: &str,
        data: &Value,
        next_version: u64,
    ) -> Result<(), SqliteGraphError> {
        let data_json = serde_json::to_string(data).unwrap_or_default();
        let conn = self.sqlite_conn.lock();
        conn.execute(
            "UPDATE node_properties SET kind = ?1, name = ?2, data = ?3, updated_version = ?4 WHERE node_id = ?5 AND deleted_version IS NULL",
            [
                &kind as &dyn ToSql,
                &name as &dyn ToSql,
                &data_json as &dyn ToSql,
                &next_version as &dyn ToSql,
                &node_id as &dyn ToSql,
            ],
        ).map_err(|e| SqliteGraphError::connection(format!("Failed to update node properties: {}", e)))?;
        Ok(())
    }

    pub(crate) fn persist_deleted_node_marker(
        &self,
        node_id: i64,
        next_version: u64,
    ) -> Result<(), SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        conn.execute(
            "UPDATE node_properties SET deleted_version = ?1 WHERE node_id = ?2 AND deleted_version IS NULL",
            [&next_version as &dyn ToSql, &node_id as &dyn ToSql],
        ).map_err(|e| SqliteGraphError::connection(format!("Failed to mark node deleted: {}", e)))?;
        Ok(())
    }
}
