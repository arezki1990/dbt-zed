use super::V3Backend;
use super::sql_value::{SqlRow, SqlValue};
use crate::SqliteGraphError;
use rusqlite::ToSql;

impl V3Backend {
    /// Parse node data from compact format: [kind_len: u8][kind bytes][name_len: u8][name bytes][json data]
    pub(super) fn parse_node_data(data: &[u8], id: i64) -> (String, String, serde_json::Value) {
        if data.len() < 2 {
            return (
                "Node".to_string(),
                format!("node_{}", id),
                serde_json::json!({}),
            );
        }

        let kind_len = data[0] as usize;
        if data.len() < 1 + kind_len + 1 {
            return (
                "Node".to_string(),
                format!("node_{}", id),
                serde_json::json!({}),
            );
        }
        let kind = String::from_utf8_lossy(&data[1..1 + kind_len]).to_string();

        let name_len_pos = 1 + kind_len;
        let name_len = data[name_len_pos] as usize;
        if data.len() < name_len_pos + 1 + name_len {
            return (kind, format!("node_{}", id), serde_json::json!({}));
        }
        let name_start = name_len_pos + 1;
        let name = String::from_utf8_lossy(&data[name_start..name_start + name_len]).to_string();

        let data_start = name_start + name_len;
        let json_data = if data_start < data.len() {
            serde_json::from_slice(&data[data_start..]).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        (kind, name, json_data)
    }

    /// Execute raw SQL query against V3Backend's SQL Layer.
    ///
    /// Returns result as vector of rows (each row = vector of columns).
    /// Column order matches SELECT clause.
    pub fn execute_sql(&self, query: &str) -> Result<Vec<SqlRow>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to prepare SQL: {}", e)))?;

        let column_count = stmt.column_count();
        let mut rows = Vec::new();

        let mut query_rows = stmt
            .query([])
            .map_err(|e| SqliteGraphError::connection(format!("Failed to execute SQL: {}", e)))?;

        while let Some(row) = query_rows
            .next()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to fetch row: {}", e)))?
        {
            let mut sql_row = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value_ref = row.get_ref(i).map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to get column: {}", e))
                })?;
                sql_row.push(SqlValue::from_sqlite_value_ref(&value_ref));
            }
            rows.push(sql_row);
        }

        Ok(rows)
    }

    /// Execute parameterized SQL query safely.
    pub fn execute_sql_params(
        &self,
        query: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<SqlRow>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to prepare SQL: {}", e)))?;

        let column_count = stmt.column_count();
        let mut rows = Vec::new();

        let query_params: Vec<&dyn ToSql> = params.to_vec();
        let mut query_rows = stmt
            .query(&*query_params)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to execute SQL: {}", e)))?;

        while let Some(row) = query_rows
            .next()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to fetch row: {}", e)))?
        {
            let mut sql_row = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value_ref = row.get_ref(i).map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to get column: {}", e))
                })?;
                sql_row.push(SqlValue::from_sqlite_value_ref(&value_ref));
            }
            rows.push(sql_row);
        }

        Ok(rows)
    }

    /// Execute SQL statement that doesn't return rows (INSERT/UPDATE/DELETE).
    pub fn execute_sql_update(&self, query: &str) -> Result<usize, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        conn.execute(query, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to execute SQL update: {}", e))
        })
    }

    /// Execute parameterized SQL update safely.
    pub fn execute_sql_update_params(
        &self,
        query: &str,
        params: &[&dyn ToSql],
    ) -> Result<usize, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        let query_params: Vec<&dyn ToSql> = params.to_vec();
        conn.execute(query, &*query_params).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to execute SQL update: {}", e))
        })
    }
}
