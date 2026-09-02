//! SQL property store for node metadata persistence.
//!
//! `PropertyStore` provides SQLite-based storage for token properties including
//! text, embeddings, and custom metadata.

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use crate::errors::SqliteGraphError;

/// Property store for node metadata using SQLite.
///
/// Stores token properties including text, embeddings, and custom JSON metadata.
pub struct PropertyStore {
    /// SQLite connection
    conn: Connection,
}

impl PropertyStore {
    /// Create a new property store at the given path.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to SQLite database file
    ///
    /// # Returns
    ///
    /// `Ok(PropertyStore)` if database opened and initialized successfully
    /// `Err(SqliteGraphError)` if database creation or initialization fails
    pub fn new(db_path: &Path) -> Result<Self, SqliteGraphError> {
        let conn = Connection::open(db_path).map_err(|e| {
            SqliteGraphError::ConnectionError(format!("Failed to open property store: {}", e))
        })?;

        // Initialize schema
        PropertyStore::init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Create in-memory property store (for testing).
    pub fn in_memory() -> Result<Self, SqliteGraphError> {
        let conn = Connection::open_in_memory().map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to open in-memory property store: {}",
                e
            ))
        })?;

        PropertyStore::init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Initialize database schema.
    fn init_schema(conn: &Connection) -> Result<(), SqliteGraphError> {
        // Main properties table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS token_properties (
                token_id INTEGER PRIMARY KEY,
                token_text TEXT,
                embedding_vector BLOB,
                metadata TEXT,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create token_properties table: {}", e))
        })?;

        // Content table for FTS5
        conn.execute(
            "CREATE TABLE IF NOT EXISTS token_content (
                token_id INTEGER PRIMARY KEY,
                content_type TEXT NOT NULL,
                content_text TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create token_content table: {}", e))
        })?;

        // FTS5 virtual table
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS token_content_fts USING fts5(
                token_id,
                content_type,
                content_text,
                tokenize='porter'
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create FTS5 table: {}", e))
        })?;

        // Triggers for FTS5 sync
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS token_content_insert AFTER INSERT ON token_content BEGIN
              INSERT INTO token_content_fts(rowid, token_id, content_type, content_text)
              VALUES (new.rowid, new.token_id, new.content_type, new.content_text);
             END",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create insert trigger: {}", e))
        })?;

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS token_content_delete AFTER DELETE ON token_content BEGIN
              DELETE FROM token_content_fts WHERE rowid = old.rowid;
             END",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create delete trigger: {}", e))
        })?;

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS token_content_update AFTER UPDATE ON token_content BEGIN
              DELETE FROM token_content_fts WHERE rowid = old.rowid;
              INSERT INTO token_content_fts(rowid, token_id, content_type, content_text)
              VALUES (new.rowid, new.token_id, new.content_type, new.content_text);
             END",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create update trigger: {}", e))
        })?;

        // Typed attributes table for rich filtering
        conn.execute(
            "CREATE TABLE IF NOT EXISTS token_attributes (
                token_id INTEGER NOT NULL,
                attr_name TEXT NOT NULL,
                attr_type TEXT NOT NULL,
                attr_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (token_id, attr_name)
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create token_attributes table: {}", e))
        })?;

        // Indexes for common query patterns
        conn.execute(
            "CREATE INDEX IF NOT EXISTS attrs_name ON token_attributes(attr_name)",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::SchemaError(format!("Failed to create attrs_name index: {}", e))
        })?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS attrs_type_value ON token_attributes(attr_name, attr_value)",
            [],
        ).map_err(|e| SqliteGraphError::SchemaError(
            format!("Failed to create attrs_type_value index: {}", e)
        ))?;

        Ok(())
    }

    /// Set token text property.
    ///
    /// # Arguments
    ///
    /// * `token_id` - Token node ID
    /// * `text` - Text representation of the token
    pub fn set_token_text(&mut self, token_id: u32, text: &str) -> Result<(), SqliteGraphError> {
        let updated_at = chrono::Utc::now().timestamp();

        self.conn.execute(
            "INSERT INTO token_properties (token_id, token_text, embedding_vector, metadata, updated_at)
             VALUES (?1, ?2, NULL, NULL, ?3)
             ON CONFLICT(token_id) DO UPDATE SET
                token_text = ?2,
                updated_at = ?3",
            params![token_id, text, updated_at],
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to set token text for {}: {}", token_id, e)
        ))?;

        Ok(())
    }

    /// Get token text property.
    ///
    /// Returns `None` if token has no text property.
    pub fn get_token_text(&self, token_id: u32) -> Result<Option<String>, SqliteGraphError> {
        let mut stmt = self
            .conn
            .prepare("SELECT token_text FROM token_properties WHERE token_id = ?1")
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to prepare token text query: {}", e))
            })?;

        let text = stmt
            .query_row(params![token_id], |row| row.get(0))
            .optional()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to query token text for {}: {}",
                    token_id, e
                ))
            })?;

        Ok(text)
    }

    /// Set embedding vector for a token.
    ///
    /// # Arguments
    ///
    /// * `token_id` - Token node ID
    /// * `embedding` - Vector of f32 values (will be stored as BLOB)
    pub fn set_embedding(
        &mut self,
        token_id: u32,
        embedding: &[f32],
    ) -> Result<(), SqliteGraphError> {
        // Convert f32 slice to bytes
        let blob: Vec<u8> = embedding.iter().flat_map(|&f| f.to_le_bytes()).collect();

        let updated_at = chrono::Utc::now().timestamp();

        self.conn.execute(
            "INSERT INTO token_properties (token_id, token_text, embedding_vector, metadata, updated_at)
             VALUES (?1, NULL, ?2, NULL, ?3)
             ON CONFLICT(token_id) DO UPDATE SET
                embedding_vector = ?2,
                updated_at = ?3",
            params![token_id, blob, updated_at],
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to set embedding for {}: {}", token_id, e)
        ))?;

        Ok(())
    }

    /// Get embedding vector for a token.
    ///
    /// Returns `None` if token has no embedding.
    pub fn get_embedding(&self, token_id: u32) -> Result<Option<Vec<f32>>, SqliteGraphError> {
        let mut stmt = self
            .conn
            .prepare("SELECT embedding_vector FROM token_properties WHERE token_id = ?1")
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to prepare embedding query: {}", e))
            })?;

        let blob = stmt
            .query_row(params![token_id], |row| row.get::<_, Vec<u8>>(0))
            .optional()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to query embedding for {}: {}",
                    token_id, e
                ))
            })?;

        if let Some(blob) = blob {
            // Convert bytes back to f32 slice
            let embedding: Vec<f32> = blob
                .chunks(4)
                .map(|chunk| {
                    let arr = [chunk[0], chunk[1], chunk[2], chunk[3]];
                    f32::from_le_bytes(arr)
                })
                .collect();

            Ok(Some(embedding))
        } else {
            Ok(None)
        }
    }

    /// Set custom metadata for a token.
    ///
    /// # Arguments
    ///
    /// * `token_id` - Token node ID
    /// * `metadata` - JSON string with custom metadata
    pub fn set_metadata(&mut self, token_id: u32, metadata: &str) -> Result<(), SqliteGraphError> {
        let updated_at = chrono::Utc::now().timestamp();

        self.conn.execute(
            "INSERT INTO token_properties (token_id, token_text, embedding_vector, metadata, updated_at)
             VALUES (?1, NULL, NULL, ?2, ?3)
             ON CONFLICT(token_id) DO UPDATE SET
                metadata = ?2,
                updated_at = ?3",
            params![token_id, metadata, updated_at],
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to set metadata for {}: {}", token_id, e)
        ))?;

        Ok(())
    }

    /// Get custom metadata for a token.
    ///
    /// Returns `None` if token has no metadata.
    pub fn get_metadata(&self, token_id: u32) -> Result<Option<String>, SqliteGraphError> {
        let mut stmt = self
            .conn
            .prepare("SELECT metadata FROM token_properties WHERE token_id = ?1")
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to prepare metadata query: {}", e))
            })?;

        let metadata = stmt
            .query_row(params![token_id], |row| row.get(0))
            .optional()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to query metadata for {}: {}",
                    token_id, e
                ))
            })?;

        Ok(metadata)
    }

    /// Get all properties for a token at once.
    ///
    /// Returns `(token_text, embedding, metadata)` tuple or `None` if token not found.
    pub fn get_all_properties(
        &self,
        token_id: u32,
    ) -> Result<Option<(String, Vec<f32>, String)>, SqliteGraphError> {
        let mut stmt = self.conn.prepare(
            "SELECT token_text, embedding_vector, metadata FROM token_properties WHERE token_id = ?1"
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to prepare properties query: {}", e)
        ))?;

        let result = stmt
            .query_row(params![token_id], |row| {
                let text: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let metadata: String = row.get(2)?;

                // Convert embedding blob back to f32 vector
                let embedding: Vec<f32> = blob
                    .chunks(4)
                    .map(|chunk| {
                        let arr = [chunk[0], chunk[1], chunk[2], chunk[3]];
                        f32::from_le_bytes(arr)
                    })
                    .collect();

                Ok((text, embedding, metadata))
            })
            .optional()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to query properties for {}: {}",
                    token_id, e
                ))
            })?;

        Ok(result)
    }

    /// Delete all properties for a token.
    pub fn delete_token(&mut self, token_id: u32) -> Result<(), SqliteGraphError> {
        self.conn
            .execute(
                "DELETE FROM token_properties WHERE token_id = ?1",
                params![token_id],
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to delete token {}: {}", token_id, e))
            })?;

        Ok(())
    }

    /// Count total number of tokens in the store.
    pub fn token_count(&self) -> Result<usize, SqliteGraphError> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM token_properties")
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to prepare count query: {}", e))
            })?;

        let count: i64 = stmt
            .query_row([], |row| row.get(0))
            .map_err(|e| SqliteGraphError::QueryError(format!("Failed to count tokens: {}", e)))?;

        Ok(count as usize)
    }

    // ========== FTS5 Methods (Spec: fts5_spec.rs) ==========

    /// Index content for full-text search.
    ///
    /// Spec: fts5_spec.rs
    pub fn index_content(
        &mut self,
        token_id: u32,
        content_type: &str,
        text: &str,
    ) -> Result<(), SqliteGraphError> {
        let updated_at = chrono::Utc::now().timestamp();

        self.conn
            .execute(
                "INSERT INTO token_content (token_id, content_type, content_text, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(token_id) DO UPDATE SET
                content_type = ?2,
                content_text = ?3,
                updated_at = ?4",
                params![token_id, content_type, text, updated_at],
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to index content for {}: {}",
                    token_id, e
                ))
            })?;

        Ok(())
    }

    /// Search content using FTS5 full-text search.
    ///
    /// Spec: fts5_spec.rs
    pub fn search_content(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::sharding::fts5_spec::ContentMatch>, SqliteGraphError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.token_id, bm25(token_content_fts) AS score, c.content_type,
                    snippet(token_content_fts, 2, '<marker>', '</marker>', '...', 32) AS snippet
             FROM token_content c
             INNER JOIN token_content_fts f ON c.token_id = f.token_id
             WHERE token_content_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to prepare FTS5 search: {}", e))
            })?;

        let results = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(crate::sharding::fts5_spec::ContentMatch {
                    token_id: row.get(0)?,
                    score: row.get(1)?,
                    content_type: row.get(2)?,
                    snippet: row.get(3)?,
                })
            })
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to execute FTS5 search: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!("Failed to collect FTS5 results: {}", e))
            })?;

        Ok(results)
    }

    /// Search content with type filter.
    pub fn search_content_type(
        &self,
        query: &str,
        content_type: &str,
        limit: usize,
    ) -> Result<Vec<crate::sharding::fts5_spec::ContentMatch>, SqliteGraphError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.token_id, bm25(token_content_fts) AS score, c.content_type,
                    snippet(token_content_fts, 2, '<marker>', '</marker>', '...', 32) AS snippet
             FROM token_content c
             INNER JOIN token_content_fts f ON c.token_id = f.token_id
             WHERE token_content_fts MATCH ?1 AND c.content_type = ?2
             ORDER BY score
             LIMIT ?3",
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to prepare FTS5 search with type filter: {}",
                    e
                ))
            })?;

        let results = stmt
            .query_map(params![query, content_type, limit as i64], |row| {
                Ok(crate::sharding::fts5_spec::ContentMatch {
                    token_id: row.get(0)?,
                    score: row.get(1)?,
                    content_type: row.get(2)?,
                    snippet: row.get(3)?,
                })
            })
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to execute FTS5 search with type filter: {}",
                    e
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to collect FTS5 results with type filter: {}",
                    e
                ))
            })?;

        Ok(results)
    }

    /// Delete content from FTS5 index.
    pub fn delete_content(&mut self, token_id: u32) -> Result<(), SqliteGraphError> {
        self.conn
            .execute(
                "DELETE FROM token_content WHERE token_id = ?1",
                params![token_id],
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to delete content for {}: {}",
                    token_id, e
                ))
            })?;

        Ok(())
    }

    // ========== Property Filter Methods (Spec: filters_spec.rs) ==========

    /// Set typed attribute for a token.
    ///
    /// Spec: filters_spec.rs
    pub fn set_attribute(
        &mut self,
        token_id: u32,
        name: &str,
        value: crate::sharding::filters_spec::Attribute,
    ) -> Result<(), SqliteGraphError> {
        let updated_at = chrono::Utc::now().timestamp();

        let (attr_type, attr_value): (String, String) = match value {
            crate::sharding::filters_spec::Attribute::String(s) => ("string".to_string(), s),
            crate::sharding::filters_spec::Attribute::Integer(i) => {
                ("integer".to_string(), i.to_string())
            }
            crate::sharding::filters_spec::Attribute::Float(f) => {
                ("float".to_string(), f.to_string())
            }
            crate::sharding::filters_spec::Attribute::Boolean(b) => {
                ("boolean".to_string(), b.to_string())
            }
        };

        self.conn.execute(
            "INSERT INTO token_attributes (token_id, attr_name, attr_type, attr_value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(token_id, attr_name) DO UPDATE SET
                attr_type = ?3,
                attr_value = ?4,
                updated_at = ?5",
            params![token_id, name, attr_type, attr_value, updated_at],
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to set attribute {} for {}: {}", name, token_id, e)
        ))?;

        Ok(())
    }

    /// Get typed attribute for a token.
    ///
    /// Spec: filters_spec.rs
    pub fn get_attribute(
        &self,
        token_id: u32,
        name: &str,
    ) -> Result<Option<crate::sharding::filters_spec::Attribute>, SqliteGraphError> {
        let mut stmt = self.conn.prepare(
            "SELECT attr_type, attr_value FROM token_attributes WHERE token_id = ?1 AND attr_name = ?2"
        ).map_err(|e| SqliteGraphError::QueryError(
            format!("Failed to prepare attribute query: {}", e)
        ))?;

        let result = stmt
            .query_row(params![token_id, name], |row| {
                let attr_type: String = row.get(0)?;
                let attr_value: String = row.get(1)?;

                let attr = match attr_type.as_str() {
                    "string" => crate::sharding::filters_spec::Attribute::String(attr_value),
                    "integer" => {
                        let i = attr_value.parse::<i64>().map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(
                                format!("Invalid integer: {}", e).into(),
                            )
                        })?;
                        crate::sharding::filters_spec::Attribute::Integer(i)
                    }
                    "float" => {
                        let f = attr_value.parse::<f64>().map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(
                                format!("Invalid float: {}", e).into(),
                            )
                        })?;
                        crate::sharding::filters_spec::Attribute::Float(f)
                    }
                    "boolean" => {
                        let b = attr_value.parse::<bool>().map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(
                                format!("Invalid boolean: {}", e).into(),
                            )
                        })?;
                        crate::sharding::filters_spec::Attribute::Boolean(b)
                    }
                    _ => {
                        return Err(rusqlite::Error::ToSqlConversionFailure(
                            format!("Unknown attribute type: {}", attr_type).into(),
                        ));
                    }
                };

                Ok(attr)
            })
            .optional()
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to query attribute {} for {}: {}",
                    name, token_id, e
                ))
            })?;

        Ok(result)
    }

    /// Query tokens by attribute filter.
    ///
    /// Spec: filters_spec.rs
    pub fn query_filtered(
        &self,
        filter: crate::sharding::filters_spec::AttrFilter,
    ) -> Result<Vec<u32>, SqliteGraphError> {
        // Handle AND/OR separately to avoid parameter conflicts
        match &filter {
            crate::sharding::filters_spec::AttrFilter::And(left, right) => {
                let left_results = self.query_filtered((**left).clone())?;
                let right_results = self.query_filtered((**right).clone())?;
                // Intersection
                Ok(left_results
                    .into_iter()
                    .filter(|id| right_results.contains(id))
                    .collect())
            }
            crate::sharding::filters_spec::AttrFilter::Or(left, right) => {
                let left_results = self.query_filtered((**left).clone())?;
                let right_results = self.query_filtered((**right).clone())?;
                // Union (deduplicated)
                let mut set = std::collections::HashSet::new();
                set.extend(left_results);
                set.extend(right_results);
                Ok(set.into_iter().collect())
            }
            _ => {
                let (sql, params) = self.build_filter_query(&filter)?;

                let mut stmt = self.conn.prepare(&sql).map_err(|e| {
                    SqliteGraphError::QueryError(format!("Failed to prepare filter query: {}", e))
                })?;

                let results = stmt
                    .query_map(rusqlite::params_from_iter(params), |row| row.get(0))
                    .map_err(|e| {
                        SqliteGraphError::QueryError(format!(
                            "Failed to execute filter query: {}",
                            e
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| {
                        SqliteGraphError::QueryError(format!(
                            "Failed to collect filter results: {}",
                            e
                        ))
                    })?;

                Ok(results)
            }
        }
    }

    /// Delete attribute for a token.
    pub fn delete_attribute(&mut self, token_id: u32, name: &str) -> Result<(), SqliteGraphError> {
        self.conn
            .execute(
                "DELETE FROM token_attributes WHERE token_id = ?1 AND attr_name = ?2",
                params![token_id, name],
            )
            .map_err(|e| {
                SqliteGraphError::QueryError(format!(
                    "Failed to delete attribute {} for {}: {}",
                    name, token_id, e
                ))
            })?;

        Ok(())
    }

    /// Build SQL query from filter expression.
    fn build_filter_query(
        &self,
        filter: &crate::sharding::filters_spec::AttrFilter,
    ) -> Result<(String, Vec<Box<dyn rusqlite::ToSql>>), SqliteGraphError> {
        match filter {
            crate::sharding::filters_spec::AttrFilter::Equals(name, value) => {
                let (attr_type, val_str) = self.attr_to_sql(value);
                let sql = "SELECT DISTINCT token_id FROM token_attributes WHERE attr_name = ?1 AND attr_type = ?2 AND attr_value = ?3".to_string();
                let params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                    Box::new(name.to_string()),
                    Box::new(attr_type),
                    Box::new(val_str),
                ];
                Ok((sql, params))
            }
            crate::sharding::filters_spec::AttrFilter::GreaterThan(name, value) => {
                let sql = "SELECT DISTINCT token_id FROM token_attributes WHERE attr_name = ?1 AND attr_type = 'float' AND CAST(attr_value AS REAL) > ?2".to_string();
                let params: Vec<Box<dyn rusqlite::ToSql>> =
                    vec![Box::new(name.to_string()), Box::new(*value)];
                Ok((sql, params))
            }
            crate::sharding::filters_spec::AttrFilter::LessThan(name, value) => {
                let sql = "SELECT DISTINCT token_id FROM token_attributes WHERE attr_name = ?1 AND attr_type IN ('float', 'integer') AND CAST(attr_value AS REAL) < ?2".to_string();
                let params: Vec<Box<dyn rusqlite::ToSql>> =
                    vec![Box::new(name.to_string()), Box::new(*value)];
                Ok((sql, params))
            }
            crate::sharding::filters_spec::AttrFilter::In(name, values) => {
                let mut sql =
                    "SELECT DISTINCT token_id FROM token_attributes WHERE attr_name = ?1 AND ("
                        .to_string();
                let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(name.to_string())];
                let mut conditions = Vec::new();

                for (i, value) in values.iter().enumerate() {
                    let (attr_type, val_str) = self.attr_to_sql(value);
                    conditions.push(format!(
                        "(attr_type = ?{} AND attr_value = ?{})",
                        i * 2 + 2,
                        i * 2 + 3
                    ));
                    params.push(Box::new(attr_type));
                    params.push(Box::new(val_str));
                }

                sql.push_str(&conditions.join(" OR "));
                sql.push(')');

                Ok((sql, params))
            }
            // And/Or are handled in query_filtered, should not reach here
            _ => Err(SqliteGraphError::QueryError(
                "AND/OR filters should be handled in query_filtered, not build_filter_query"
                    .to_string(),
            )),
        }
    }

    /// Convert Attribute to SQL type and value string.
    fn attr_to_sql(&self, attr: &crate::sharding::filters_spec::Attribute) -> (String, String) {
        match attr {
            crate::sharding::filters_spec::Attribute::String(s) => {
                ("string".to_string(), s.clone())
            }
            crate::sharding::filters_spec::Attribute::Integer(i) => {
                ("integer".to_string(), i.to_string())
            }
            crate::sharding::filters_spec::Attribute::Float(f) => {
                ("float".to_string(), f.to_string())
            }
            crate::sharding::filters_spec::Attribute::Boolean(b) => {
                ("boolean".to_string(), b.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_store_creation() {
        let store = PropertyStore::in_memory().unwrap();
        assert_eq!(store.token_count().unwrap(), 0);
    }

    #[test]
    fn test_set_and_get_token_text() {
        let mut store = PropertyStore::in_memory().unwrap();

        store.set_token_text(100, "hello").unwrap();
        let text = store.get_token_text(100).unwrap();

        assert_eq!(text, Some("hello".to_string()));
    }

    #[test]
    fn test_token_text_persistence() {
        let mut store = PropertyStore::in_memory().unwrap();

        store.set_token_text(200, "world").unwrap();

        // Verify it persists
        let text = store.get_token_text(200).unwrap();
        assert_eq!(text, Some("world".to_string()));
    }

    #[test]
    fn test_embedding_roundtrip() {
        let mut store = PropertyStore::in_memory().unwrap();

        let embedding = vec![0.1f32, 0.2, 0.3, 0.4];
        store.set_embedding(300, &embedding).unwrap();

        let retrieved = store.get_embedding(300).unwrap().unwrap();
        assert_eq!(retrieved.len(), 4);

        for i in 0..4 {
            assert!((retrieved[i] - embedding[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn test_metadata_roundtrip() {
        let mut store = PropertyStore::in_memory().unwrap();

        let metadata = r#"{"type": "noun", "freq": 100}"#;
        store.set_metadata(400, metadata).unwrap();

        let retrieved = store.get_metadata(400).unwrap().unwrap();
        assert_eq!(retrieved, metadata);
    }

    #[test]
    fn test_get_all_properties() {
        let mut store = PropertyStore::in_memory().unwrap();

        store.set_token_text(500, "test").unwrap();
        store.set_embedding(500, &[0.5, 0.6, 0.7]).unwrap();
        store.set_metadata(500, r#"{"key": "value"}"#).unwrap();

        let (text, embedding, metadata) = store.get_all_properties(500).unwrap().unwrap();

        assert_eq!(text, "test");
        assert_eq!(embedding.len(), 3);
        assert_eq!(metadata, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_delete_token() {
        let mut store = PropertyStore::in_memory().unwrap();

        store.set_token_text(600, "delete me").unwrap();
        assert!(store.get_token_text(600).unwrap().is_some());

        store.delete_token(600).unwrap();
        assert!(store.get_token_text(600).unwrap().is_none());
    }

    #[test]
    fn test_token_count() {
        let mut store = PropertyStore::in_memory().unwrap();

        assert_eq!(store.token_count().unwrap(), 0);

        store.set_token_text(700, "one").unwrap();
        store.set_token_text(800, "two").unwrap();

        assert_eq!(store.token_count().unwrap(), 2);

        store.delete_token(700).unwrap();
        assert_eq!(store.token_count().unwrap(), 1);
    }
}
