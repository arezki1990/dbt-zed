use super::V3Backend;
use crate::SqliteGraphError;
use rusqlite::Connection;

impl V3Backend {
    pub(super) fn init_sqlite_schema(conn: &Connection) -> Result<(), SqliteGraphError> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS node_properties (
                node_id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                name TEXT,
                data TEXT,
                created_at INTEGER NOT NULL,
                created_version INTEGER NOT NULL DEFAULT 1,
                updated_version INTEGER,
                deleted_version INTEGER
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create node_properties table: {}", e))
        })?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS node_props_kind ON node_properties(kind)",
            [],
        )
        .map_err(|e| SqliteGraphError::connection(format!("Failed to create kind index: {}", e)))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS edge_attributes (
                src INTEGER NOT NULL,
                dst INTEGER NOT NULL,
                attr_name TEXT NOT NULL,
                attr_value TEXT,
                created_version INTEGER NOT NULL DEFAULT 1,
                deleted_version INTEGER,
                PRIMARY KEY (src, dst, attr_name)
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create edge_attributes table: {}", e))
        })?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS layout_graph (
                table_name TEXT NOT NULL,
                column_name TEXT NOT NULL,
                rowgroup_id INTEGER,
                page_id INTEGER,
                parent_page_id INTEGER,
                PRIMARY KEY (table_name, column_name, rowgroup_id, page_id)
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create layout_graph table: {}", e))
        })?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS layout_parent ON layout_graph(parent_page_id)",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create parent index: {}", e))
        })?;

        Self::ensure_sqlite_column(
            conn,
            "node_properties",
            "created_version",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        Self::ensure_sqlite_column(conn, "node_properties", "updated_version", "INTEGER")?;
        Self::ensure_sqlite_column(conn, "node_properties", "deleted_version", "INTEGER")?;
        Self::ensure_sqlite_column(
            conn,
            "edge_attributes",
            "created_version",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        Self::ensure_sqlite_column(conn, "edge_attributes", "deleted_version", "INTEGER")?;

        conn.execute(
            "UPDATE node_properties SET created_version = 1 WHERE created_version IS NULL",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to backfill node created_version: {}", e))
        })?;
        conn.execute(
            "UPDATE edge_attributes SET created_version = 1 WHERE created_version IS NULL",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to backfill edge created_version: {}", e))
        })?;

        Ok(())
    }

    pub(super) fn ensure_sqlite_column(
        conn: &Connection,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), SqliteGraphError> {
        let pragma = format!("PRAGMA table_info({})", table);
        let mut stmt = conn.prepare(&pragma).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to inspect table {}: {}", table, e))
        })?;

        let mut rows = stmt.query([]).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to read table info for {}: {}", table, e))
        })?;

        while let Some(row) = rows.next().map_err(|e| {
            SqliteGraphError::connection(format!(
                "Failed to iterate table info for {}: {}",
                table, e
            ))
        })? {
            let existing: String = row.get(1).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to decode column info for {}: {}",
                    table, e
                ))
            })?;
            if existing == column {
                return Ok(());
            }
        }

        let alter = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition);
        conn.execute(&alter, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to migrate {}.{}: {}", table, column, e))
        })?;
        Ok(())
    }
}
