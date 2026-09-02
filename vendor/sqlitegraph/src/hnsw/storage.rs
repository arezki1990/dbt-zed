//! HNSW Vector Storage Abstraction
//!
//! This module provides a unified storage abstraction for vector data that works
//! with both SQLite and Native backends. It handles vector persistence,
//! retrieval, and metadata management while maintaining consistency with the
//! existing SQLiteGraph architecture.
//!
//! # Architecture
//!
//! - **Unified Backend**: Works with both SQLite and Native backends transparently
//! - **Binary Storage**: Efficient vector data serialization using f32 arrays
//! - **Metadata Integration**: JSON metadata support alongside vectors
//! - **Error Handling**: Comprehensive error handling for vector operations
//!
//! # Storage Features
//!
//! ```text
//! // Store vector with metadata
//! let vector_id = storage.store_vector(
//!     &[1.0, 2.0, 3.0],
//!     Some(json!({"source": "embedding", "model": "text-ada-002"}))
//! )?;
//!
//! // Retrieve vector with metadata
//! let (vector, metadata) = storage.get_vector_with_metadata(vector_id)?;
//!
//! // Batch operations for performance
//! let vectors = storage.store_batch(vectors, metadatas)?;
//! ```
//!
//! # Backend Integration
//!
//! The storage abstraction automatically adapts to the active backend:
//! - **SQLite Backend**: Stores vectors in dedicated `hnsw_vectors` table using BLOB columns
//! - **Native Backend**: Stores vectors in binary format with clustering optimization
//! - **HNSW Integration**: Integration with similarity search capabilities

use crate::hnsw::errors::{HnswError, HnswStorageError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;

/// Vector storage record with metadata
///
/// Represents a stored vector with associated metadata and system information.
/// Vectors are stored as f32 arrays with JSON metadata for flexibility.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    /// Unique identifier for this vector
    pub id: u64,

    /// Vector dimension (length of the data array)
    pub dimension: usize,

    /// Vector data stored as f32 values
    pub data: Vec<f32>,

    /// Optional JSON metadata for additional information
    pub metadata: Option<Value>,

    /// Timestamp when vector was created (Unix timestamp)
    pub created_at: u64,

    /// Timestamp when vector was last updated (Unix timestamp)
    pub updated_at: u64,
}

impl VectorRecord {
    /// Create a new vector record
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier
    /// * `data` - Vector data
    /// * `metadata` - Optional JSON metadata
    ///
    /// # Returns
    ///
    /// New VectorRecord instance
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sqlitegraph::hnsw::storage::VectorRecord;
    /// use serde_json::json;
    ///
    /// let vector = vec![1.0, 2.0, 3.0];
    /// let metadata = Some(json!({"source": "embedding"}));
    ///
    /// let record = VectorRecord::new(42, vector, metadata);
    /// assert_eq!(record.id, 42);
    /// assert_eq!(record.dimension, 3);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(id: u64, data: Vec<f32>, metadata: Option<Value>) -> Self {
        let dimension = data.len();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            id,
            dimension,
            data,
            metadata,
            created_at: now,
            updated_at: now,
        }
    }

    /// Get vector ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get vector dimension
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Get vector data reference
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Get vector data as mutable reference
    pub fn data_mut(&mut self) -> &mut Vec<f32> {
        &mut self.data
    }

    /// Get metadata reference
    pub fn metadata(&self) -> Option<&Value> {
        self.metadata.as_ref()
    }

    /// Get creation timestamp
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Get update timestamp
    pub fn updated_at(&self) -> u64 {
        self.updated_at
    }

    /// Update the updated_at timestamp
    pub fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    /// Validate the vector record
    ///
    /// # Returns
    ///
    /// Ok(()) if valid, Err with validation error details
    pub fn validate(&self) -> Result<(), HnswError> {
        // Check dimension constraints
        if self.dimension == 0 {
            return Err(HnswError::Storage(HnswStorageError::InvalidDimension(
                self.dimension,
            )));
        }

        // Check data length matches dimension
        if self.data.len() != self.dimension {
            return Err(HnswError::Storage(HnswStorageError::DimensionMismatch {
                expected: self.dimension,
                actual: self.data.len(),
            }));
        }

        // Check for NaN or infinite values
        if self.data.iter().any(|&x| !x.is_finite()) {
            return Err(HnswError::Storage(HnswStorageError::InvalidVectorData));
        }

        Ok(())
    }

    /// Estimate memory usage in bytes
    ///
    /// # Returns
    ///
    /// Estimated memory usage including overhead
    pub fn memory_usage(&self) -> usize {
        let base_overhead = std::mem::size_of::<Self>();
        let data_size = self.data.len() * std::mem::size_of::<f32>();
        let metadata_size = self
            .metadata
            .as_ref()
            .map(|m| m.to_string().len())
            .unwrap_or(0);

        base_overhead + data_size + metadata_size
    }
}

/// Batch vector storage operation
///
/// Contains multiple vector records for bulk storage operations.
/// Used for efficient batch inserts and updates.
#[derive(Debug, Clone)]
pub struct VectorBatch {
    /// List of vector records to store
    pub vectors: Vec<VectorRecord>,
}

impl VectorBatch {
    /// Create a new batch from individual vectors and metadata
    ///
    /// # Arguments
    ///
    /// * `vectors` - Vector data
    /// * `metadatas` - Corresponding metadata (same length as vectors)
    ///
    /// # Returns
    ///
    /// New VectorBatch or error if lengths don't match
    pub fn new(vectors: Vec<Vec<f32>>, metadatas: Vec<Option<Value>>) -> Result<Self, HnswError> {
        if vectors.len() != metadatas.len() {
            return Err(HnswError::Storage(HnswStorageError::BatchSizeMismatch));
        }

        let records: Result<Vec<_>, _> = vectors
            .into_iter()
            .zip(metadatas)
            .enumerate()
            .map(|(index, (vector, metadata))| {
                Ok(VectorRecord::new(index as u64, vector, metadata))
            })
            .collect();

        match records {
            Ok(validated_records) => {
                // Validate all records
                for record in &validated_records {
                    record.validate()?;
                }
                Ok(Self {
                    vectors: validated_records,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Get batch size
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Get total estimated memory usage for the batch
    pub fn memory_usage(&self) -> usize {
        self.vectors.iter().map(|v| v.memory_usage()).sum()
    }
}

/// Vector storage backend abstraction
///
/// Provides unified interface for storing and retrieving vectors across different
/// storage backends. Automatically adapts to the active backend type.
/// All implementors must be `Send` so that `HnswIndex` (and its parent
/// `SqliteGraph`) can be safely transferred across threads (e.g. stored in
/// axum `AppState`). `Sync` is intentionally NOT required because
/// `rusqlite::Connection` (used by `SQLiteVectorStorage`) is `!Sync`.
pub trait VectorStorage: Send {
    /// Store a vector with optional metadata
    ///
    /// # Arguments
    ///
    /// * `vector` - Vector data to store
    /// * `metadata` - Optional JSON metadata
    ///
    /// # Returns
    ///
    /// Vector ID for future retrieval
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use serde_json::json;
    ///
    /// let vector = vec![1.0, 2.0, 3.0];
    /// let metadata = Some(json!({"source": "test"}));
    ///
    /// let vector_id = storage.store_vector(&vector, metadata)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    fn store_vector(&mut self, vector: &[f32], metadata: Option<Value>) -> Result<u64, HnswError>;

    /// Store vector with explicit ID
    ///
    /// # Arguments
    ///
    /// * `id` - Explicit vector ID
    /// * `vector` - Vector data
    /// * `metadata` - Optional metadata
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    fn store_vector_with_id(
        &mut self,
        id: u64,
        vector: Vec<f32>,
        metadata: Option<Value>,
    ) -> Result<(), HnswError>;

    /// Retrieve vector by ID
    ///
    /// # Arguments
    ///
    /// * `id` - Vector ID to retrieve
    ///
    /// # Returns
    ///
    /// Vector data if found
    fn get_vector(&self, id: u64) -> Result<Option<Vec<f32>>, HnswError>;

    /// Retrieve vector with metadata
    ///
    /// # Arguments
    ///
    /// * `id` - Vector ID to retrieve
    ///
    /// # Returns
    ///
    /// Vector and metadata if found
    fn get_vector_with_metadata(&self, id: u64) -> Result<Option<(Vec<f32>, Value)>, HnswError>;

    /// Store multiple vectors in batch
    ///
    /// # Arguments
    ///
    /// * `batch` - Batch of vectors to store
    ///
    /// # Returns
    ///
    /// Vector IDs for all stored vectors
    fn store_batch(&mut self, batch: VectorBatch) -> Result<Vec<u64>, HnswError>;

    /// Delete vector by ID
    ///
    /// # Arguments
    ///
    /// * `id` - Vector ID to delete
    ///
    /// # Returns
    ///
    /// Ok(()) if deleted or didn't exist
    fn delete_vector(&mut self, id: u64) -> Result<(), HnswError>;

    /// Get vector count
    ///
    /// # Returns
    ///
    /// Total number of stored vectors
    fn vector_count(&self) -> Result<usize, HnswError>;

    /// List all vector IDs
    ///
    /// # Returns
    ///
    /// List of all stored vector IDs
    fn list_vectors(&self) -> Result<Vec<u64>, HnswError>;

    /// Clear all vectors
    ///
    /// # Returns
    ///
    /// Ok(()) when cleared
    fn clear_vectors(&mut self) -> Result<(), HnswError>;

    /// Get storage statistics
    ///
    /// # Returns
    ///
    /// Storage statistics for monitoring
    fn get_statistics(&self) -> Result<VectorStorageStats, HnswError>;

    fn as_sqlite_connection(&self) -> Option<(&rusqlite::Connection, i64)> {
        None
    }

    /// Begin a bulk-insert transaction. No-op for non-SQLite backends.
    fn begin_bulk_insert(&mut self) -> Result<(), HnswError> {
        Ok(())
    }

    /// Commit a bulk-insert transaction. No-op for non-SQLite backends.
    fn commit_bulk_insert(&mut self) -> Result<(), HnswError> {
        Ok(())
    }

    /// Roll back a bulk-insert transaction. No-op for non-SQLite backends.
    fn rollback_bulk_insert(&mut self) {
        // Best-effort; ignore errors
    }
}

/// Vector storage statistics
///
/// Provides detailed information about storage usage and performance
/// for monitoring and optimization purposes.
#[derive(Debug, Clone)]
pub struct VectorStorageStats {
    /// Total number of stored vectors
    pub vector_count: usize,

    /// Total dimensions across all vectors
    pub total_dimensions: usize,

    /// Average vector dimension
    pub average_dimension: f32,

    /// Estimated memory usage in bytes
    pub estimated_memory_bytes: usize,

    /// Storage backend type
    pub backend_type: String,
}

impl VectorStorageStats {
    /// Create new storage statistics
    pub fn new(vector_count: usize, total_dimensions: usize, backend_type: String) -> Self {
        let average_dimension = if vector_count > 0 {
            total_dimensions as f32 / vector_count as f32
        } else {
            0.0
        };

        Self {
            vector_count,
            total_dimensions,
            average_dimension,
            estimated_memory_bytes: total_dimensions * std::mem::size_of::<f32>(),
            backend_type,
        }
    }
}

/// Serialize vector to byte array
///
/// Converts f32 slice to bytes using bytemuck for zero-copy operation.
/// This is safe because f32 is POD (Plain Old Data) with no padding.
fn serialize_vector(v: &[f32]) -> Vec<u8> {
    bytemuck::cast_slice::<f32, u8>(v).to_vec()
}

/// Deserialize byte array to vector
///
/// Converts bytes to f32 array using bytemuck for zero-copy operation.
/// This is safe because f32 is POD (Plain Old Data) with no padding.
fn deserialize_vector(bytes: &[u8]) -> Result<Vec<f32>, HnswError> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(HnswError::Storage(HnswStorageError::InvalidVectorData));
    }
    Ok(bytemuck::cast_slice::<u8, f32>(bytes).to_vec())
}

/// SQLite-backed vector storage implementation
///
/// Provides persistent vector storage using SQLite database. Vectors are stored
/// as BLOB data in the `hnsw_vectors` table with metadata support.
pub struct SQLiteVectorStorage {
    index_id: i64,
    conn: Connection,
}

impl SQLiteVectorStorage {
    /// Create new SQLite-backed storage
    ///
    /// # Arguments
    ///
    /// * `index_id` - Database ID of the HNSW index
    /// * `conn` - SQLite connection
    ///
    /// # Returns
    ///
    /// New SQLiteVectorStorage instance
    pub fn new(index_id: i64, conn: Connection) -> Self {
        Self { index_id, conn }
    }
}

impl VectorStorage for SQLiteVectorStorage {
    fn store_vector(&mut self, vector: &[f32], metadata: Option<Value>) -> Result<u64, HnswError> {
        let vector_bytes = serialize_vector(vector);
        let metadata_json = metadata.map(|m| m.to_string());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn
            .execute(
                "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![&self.index_id, &vector_bytes, &metadata_json, now, now],
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(self.conn.last_insert_rowid() as u64)
    }

    fn store_vector_with_id(
        &mut self,
        id: u64,
        vector: Vec<f32>,
        metadata: Option<Value>,
    ) -> Result<(), HnswError> {
        let vector_bytes = serialize_vector(&vector);
        let metadata_json = metadata.map(|m| m.to_string());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT OR IGNORE INTO hnsw_vectors (id, index_id, vector_data, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                &id,
                &self.index_id,
                &vector_bytes,
                &metadata_json,
                now,
                now,
            ],
        )
        .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(())
    }

    fn get_vector(&self, id: u64) -> Result<Option<Vec<f32>>, HnswError> {
        let vector_bytes: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT vector_data FROM hnsw_vectors WHERE id = ? AND index_id = ?",
                rusqlite::params![id, &self.index_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        match vector_bytes {
            Some(bytes) => {
                let vector = deserialize_vector(&bytes)?;
                Ok(Some(vector))
            }
            None => Ok(None),
        }
    }

    fn get_vector_with_metadata(&self, id: u64) -> Result<Option<(Vec<f32>, Value)>, HnswError> {
        let (vector_bytes, metadata_json): (Option<Vec<u8>>, Option<String>) = self
            .conn
            .query_row(
                "SELECT vector_data, metadata FROM hnsw_vectors WHERE id = ? AND index_id = ?",
                rusqlite::params![id, &self.index_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?
            .unwrap_or((None, None));

        match vector_bytes {
            Some(bytes) => {
                let vector = deserialize_vector(&bytes)?;
                let metadata = metadata_json
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(|e| {
                        HnswError::Storage(HnswStorageError::IoError(format!(
                            "Failed to parse metadata: {}",
                            e
                        )))
                    })?
                    .unwrap_or(Value::Null);

                Ok(Some((vector, metadata)))
            }
            None => Ok(None),
        }
    }

    fn store_batch(&mut self, batch: VectorBatch) -> Result<Vec<u64>, HnswError> {
        let mut ids = Vec::with_capacity(batch.len());

        self.conn
            .execute("BEGIN IMMEDIATE", [])
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        let result: Result<(), HnswError> = (|| {
            for record in batch.vectors {
                let vector_bytes = serialize_vector(&record.data);
                let metadata_json = record.metadata.map(|m| m.to_string());

                self.conn.execute(
                    "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        &self.index_id,
                        &vector_bytes,
                        &metadata_json,
                        record.created_at as i64,
                        record.updated_at as i64,
                    ],
                )
                .map_err(|e| {
                    HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
                })?;

                ids.push(self.conn.last_insert_rowid() as u64);
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", []).map_err(|e| {
                    HnswError::Storage(HnswStorageError::DatabaseError(e.to_string()))
                })?;
            }
            Err(err) => {
                let _ = self.conn.execute("ROLLBACK", []);
                return Err(err);
            }
        }

        Ok(ids)
    }

    fn delete_vector(&mut self, id: u64) -> Result<(), HnswError> {
        self.conn
            .execute(
                "DELETE FROM hnsw_vectors WHERE id = ? AND index_id = ?",
                rusqlite::params![id, &self.index_id],
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(())
    }

    fn vector_count(&self) -> Result<usize, HnswError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM hnsw_vectors WHERE index_id = ?",
                [&self.index_id],
                |row| row.get(0),
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(count as usize)
    }

    fn list_vectors(&self) -> Result<Vec<u64>, HnswError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM hnsw_vectors WHERE index_id = ? ORDER BY id")
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        let ids = stmt
            .query_map([&self.index_id], |row| row.get(0))
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(ids)
    }

    fn clear_vectors(&mut self) -> Result<(), HnswError> {
        self.conn
            .execute(
                "DELETE FROM hnsw_vectors WHERE index_id = ?",
                [&self.index_id],
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(())
    }

    fn get_statistics(&self) -> Result<VectorStorageStats, HnswError> {
        let (vector_count, total_dimensions): (i64, i64) = self
            .conn
            .query_row(
                "SELECT
                    COUNT(*) as count,
                    SUM(LENGTH(vector_data) / ?) as total_dims
                 FROM hnsw_vectors WHERE index_id = ?",
                rusqlite::params![std::mem::size_of::<f32>() as i64, &self.index_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;

        Ok(VectorStorageStats::new(
            vector_count as usize,
            total_dimensions as usize,
            "SQLite".to_string(),
        ))
    }

    fn as_sqlite_connection(&self) -> Option<(&rusqlite::Connection, i64)> {
        Some((&self.conn, self.index_id))
    }

    fn begin_bulk_insert(&mut self) -> Result<(), HnswError> {
        self.conn
            .execute("BEGIN IMMEDIATE", [])
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;
        Ok(())
    }

    fn commit_bulk_insert(&mut self) -> Result<(), HnswError> {
        self.conn
            .execute("COMMIT", [])
            .map_err(|e| HnswError::Storage(HnswStorageError::DatabaseError(e.to_string())))?;
        Ok(())
    }

    fn rollback_bulk_insert(&mut self) {
        let _ = self.conn.execute("ROLLBACK", []);
    }
}

/// In-memory vector storage implementation
///
/// Provides fast vector storage using in-memory HashMap. Suitable for
/// temporary storage, testing, and small-scale applications.
pub struct InMemoryVectorStorage {
    /// In-memory storage map
    vectors: HashMap<u64, VectorRecord>,

    /// Next available ID for auto-assignment
    next_id: u64,
}

impl InMemoryVectorStorage {
    /// Create new in-memory storage
    ///
    /// # Returns
    ///
    /// New InMemoryVectorStorage instance
    pub fn new() -> Self {
        Self {
            vectors: HashMap::new(),
            next_id: 1,
        }
    }

    /// Get next available ID
    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Default for InMemoryVectorStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl VectorStorage for InMemoryVectorStorage {
    fn store_vector(&mut self, vector: &[f32], metadata: Option<Value>) -> Result<u64, HnswError> {
        let id = self.next_id();
        let vector_data = vector.to_vec();
        let record = VectorRecord::new(id, vector_data, metadata);

        // Validate before storing
        record.validate()?;

        self.vectors.insert(id, record);
        Ok(id)
    }

    fn store_vector_with_id(
        &mut self,
        id: u64,
        vector: Vec<f32>,
        metadata: Option<Value>,
    ) -> Result<(), HnswError> {
        let record = VectorRecord::new(id, vector, metadata);

        // Validate before storing
        record.validate()?;

        self.vectors.insert(id, record);
        Ok(())
    }

    fn get_vector(&self, id: u64) -> Result<Option<Vec<f32>>, HnswError> {
        Ok(self.vectors.get(&id).map(|record| record.data.clone()))
    }

    fn get_vector_with_metadata(&self, id: u64) -> Result<Option<(Vec<f32>, Value)>, HnswError> {
        Ok(self.vectors.get(&id).map(|record| {
            let metadata = record.metadata.clone().unwrap_or(Value::Null);
            (record.data.clone(), metadata)
        }))
    }

    fn store_batch(&mut self, batch: VectorBatch) -> Result<Vec<u64>, HnswError> {
        let batch_len = batch.len();
        let mut ids = Vec::with_capacity(batch_len);
        let start_id = self.next_id;

        for (index, record) in batch.vectors.into_iter().enumerate() {
            let id = start_id + index as u64;
            self.vectors.insert(id, record);
            ids.push(id);
        }

        self.next_id = start_id + batch_len as u64;
        Ok(ids)
    }

    fn delete_vector(&mut self, id: u64) -> Result<(), HnswError> {
        self.vectors.remove(&id);
        Ok(())
    }

    fn vector_count(&self) -> Result<usize, HnswError> {
        Ok(self.vectors.len())
    }

    fn list_vectors(&self) -> Result<Vec<u64>, HnswError> {
        let mut ids: Vec<u64> = self.vectors.keys().copied().collect();
        ids.sort_unstable(); // Ensure deterministic ordering
        Ok(ids)
    }

    fn clear_vectors(&mut self) -> Result<(), HnswError> {
        self.vectors.clear();
        self.next_id = 1;
        Ok(())
    }

    fn get_statistics(&self) -> Result<VectorStorageStats, HnswError> {
        let vector_count = self.vectors.len();
        let total_dimensions = self.vectors.values().map(|record| record.dimension).sum();

        Ok(VectorStorageStats::new(
            vector_count,
            total_dimensions,
            "InMemory".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_vector(dimension: usize) -> Vec<f32> {
        (1..=dimension).map(|i| i as f32).collect()
    }

    fn create_test_metadata() -> Value {
        json!({
            "source": "test",
            "model": "test-model",
            "version": "1.0"
        })
    }

    #[test]
    fn test_vector_record_creation() {
        let vector = create_test_vector(3);
        let metadata = Some(create_test_metadata());

        let record = VectorRecord::new(42, vector.clone(), metadata.clone());

        assert_eq!(record.id(), 42);
        assert_eq!(record.dimension(), 3);
        assert_eq!(record.data(), vector.as_slice());
        assert_eq!(record.metadata(), metadata.as_ref());
        assert!(record.created_at() > 0);
        assert!(record.updated_at() > 0);
    }

    #[test]
    fn test_vector_record_validation() {
        // Valid record
        let record = VectorRecord::new(1, vec![1.0, 2.0], None);
        assert!(record.validate().is_ok());

        // Invalid dimension (zero)
        let invalid_record = VectorRecord::new(1, vec![], None);
        assert!(invalid_record.validate().is_err());

        // Dimension mismatch
        let mut invalid_record = VectorRecord::new(1, vec![1.0, 2.0], None);
        invalid_record.dimension = 3; // Mismatch with data length
        assert!(invalid_record.validate().is_err());

        // Invalid vector data (NaN)
        let mut invalid_record = VectorRecord::new(1, vec![1.0, 2.0], None);
        invalid_record.data[1] = f32::NAN;
        assert!(invalid_record.validate().is_err());
    }

    #[test]
    fn test_vector_record_touch() {
        let mut record = VectorRecord::new(1, vec![1.0, 2.0], None);
        let original_updated = record.updated_at();

        // Wait a bit to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_secs(1));

        record.touch();
        assert!(record.updated_at() > original_updated);
    }

    #[test]
    fn test_vector_batch_creation() {
        let vectors = vec![vec![1.0, 2.0], vec![3.0, 4.0, 5.0]];
        let metadatas = vec![Some(json!({"batch": 1})), Some(json!({"batch": 2}))];

        let batch = VectorBatch::new(vectors.clone(), metadatas).unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch.vectors[0].data(), vectors[0].as_slice());
        assert_eq!(batch.vectors[1].data(), vectors[1].as_slice());
    }

    #[test]
    fn test_vector_batch_size_mismatch() {
        let vectors = vec![vec![1.0, 2.0]];
        let metadatas = vec![]; // Empty but should match vectors length

        let result = VectorBatch::new(vectors, metadatas);
        assert!(result.is_err());
    }

    #[test]
    fn test_in_memory_storage() {
        let mut storage = InMemoryVectorStorage::new();
        let vector = create_test_vector(4);
        let metadata = Some(create_test_metadata());

        // Store vector
        let id = storage.store_vector(&vector, metadata.clone()).unwrap();
        assert_eq!(id, 1);

        // Retrieve vector
        let retrieved = storage.get_vector(id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), vector);

        // Retrieve with metadata
        let (retrieved_vector, retrieved_metadata) =
            storage.get_vector_with_metadata(id).unwrap().unwrap();
        assert_eq!(retrieved_vector, vector);
        assert_eq!(Some(retrieved_metadata), metadata);

        // Vector count
        assert_eq!(storage.vector_count().unwrap(), 1);
    }

    #[test]
    fn test_in_memory_storage_with_id() {
        let mut storage = InMemoryVectorStorage::new();
        let vector = create_test_vector(3);
        let metadata = Some(create_test_metadata());

        // Store with explicit ID
        storage
            .store_vector_with_id(100, vector.clone(), metadata)
            .unwrap();

        // Retrieve with correct ID
        let retrieved = storage.get_vector(100).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), vector);
    }

    #[test]
    fn test_in_memory_batch_storage() {
        let mut storage = InMemoryVectorStorage::new();

        let vectors = vec![vec![1.0, 2.0], vec![3.0, 4.0, 5.0]];
        let metadatas = vec![Some(json!({"batch": 1})), Some(json!({"batch": 2}))];

        let batch = VectorBatch::new(vectors, metadatas).unwrap();
        let ids = storage.store_batch(batch).unwrap();

        assert_eq!(ids.len(), 2);
        assert_eq!(ids, vec![1, 2]);

        // Verify batch storage
        assert_eq!(storage.vector_count().unwrap(), 2);
    }

    #[test]
    fn test_in_memory_vector_deletion() {
        let mut storage = InMemoryVectorStorage::new();
        let vector = create_test_vector(3);

        let id = storage.store_vector(&vector, None).unwrap();
        assert_eq!(storage.vector_count().unwrap(), 1);

        storage.delete_vector(id).unwrap();
        assert_eq!(storage.vector_count().unwrap(), 0);

        // Verify deletion
        let retrieved = storage.get_vector(id).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_in_memory_vector_listing() {
        let mut storage = InMemoryVectorStorage::new();

        // Store multiple vectors
        for i in 1..=3 {
            let vector = vec![i as f32; i];
            storage.store_vector(&vector, None).unwrap();
        }

        let ids = storage.list_vectors().unwrap();
        assert_eq!(ids, vec![1, 2, 3]); // Should be sorted
    }

    #[test]
    fn test_in_memory_storage_statistics() {
        let mut storage = InMemoryVectorStorage::new();

        // Store some vectors
        storage.store_vector(&[1.0, 2.0], None).unwrap();
        storage.store_vector(&[3.0, 4.0, 5.0], None).unwrap();

        let stats = storage.get_statistics().unwrap();
        assert_eq!(stats.vector_count, 2);
        assert_eq!(stats.total_dimensions, 5);
        assert!((stats.average_dimension - 2.5).abs() < f32::EPSILON);
        assert_eq!(stats.backend_type, "InMemory");
    }

    #[test]
    fn test_in_memory_storage_clear() {
        let mut storage = InMemoryVectorStorage::new();

        // Add some data
        storage.store_vector(&[1.0, 2.0], None).unwrap();
        storage.store_vector(&[3.0, 4.0], None).unwrap();

        assert_eq!(storage.vector_count().unwrap(), 2);

        // Clear all
        storage.clear_vectors().unwrap();
        assert_eq!(storage.vector_count().unwrap(), 0);
    }

    #[test]
    fn test_vector_memory_usage() {
        let vector = vec![1.0f32; 1000]; // 1000 dimensions
        let metadata = Some(json!({"key": "value"}));

        let record = VectorRecord::new(42, vector, metadata);
        let usage = record.memory_usage();

        let expected_min =
            std::mem::size_of::<VectorRecord>() + (1000 * std::mem::size_of::<f32>());
        assert!(usage >= expected_min);
    }

    // SQLiteVectorStorage tests
    #[test]
    fn test_sqlite_vector_storage() {
        use rusqlite::Connection;

        // Create in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE hnsw_indexes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                dimension INTEGER NOT NULL,
                m INTEGER NOT NULL,
                ef_construction INTEGER NOT NULL,
                distance_metric TEXT NOT NULL,
                vector_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE hnsw_vectors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                index_id INTEGER NOT NULL,
                vector_data BLOB NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE
            );
            "#,
        )
        .unwrap();

        // Insert test index
        conn.execute(
            "INSERT INTO hnsw_indexes (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params!["test_index", 3, 16, 200, "cosine", 0, 1000, 1000],
        )
        .unwrap();

        let index_id = conn.last_insert_rowid();

        // Test vector storage
        let mut storage = SQLiteVectorStorage::new(index_id, conn);
        let vector = create_test_vector(4);
        let metadata = Some(create_test_metadata());

        // Store vector
        let id = storage.store_vector(&vector, metadata.clone()).unwrap();
        assert_eq!(id, 1);

        // Retrieve vector
        let retrieved = storage.get_vector(id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), vector);

        // Retrieve with metadata
        let (retrieved_vector, retrieved_metadata) =
            storage.get_vector_with_metadata(id).unwrap().unwrap();
        assert_eq!(retrieved_vector, vector);
        assert_eq!(Some(retrieved_metadata), metadata);

        // Vector count
        assert_eq!(storage.vector_count().unwrap(), 1);
    }

    #[test]
    fn test_sqlite_vector_roundtrip() {
        use rusqlite::Connection;

        let conn = Connection::open_in_memory().unwrap();

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE hnsw_indexes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                dimension INTEGER NOT NULL,
                m INTEGER NOT NULL,
                ef_construction INTEGER NOT NULL,
                distance_metric TEXT NOT NULL,
                vector_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE hnsw_vectors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                index_id INTEGER NOT NULL,
                vector_data BLOB NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE
            );
            "#,
        )
        .unwrap();

        conn.execute(
            "INSERT INTO hnsw_indexes (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params!["test_index", 128, 16, 200, "euclidean", 0, 1000, 1000],
        )
        .unwrap();

        let index_id = conn.last_insert_rowid();

        let mut storage = SQLiteVectorStorage::new(index_id, conn);

        // Create test vector
        let original: Vec<f32> = (0..128).map(|i| i as f32 / 128.0).collect();

        // Store
        let id = storage.store_vector(&original, None).unwrap();

        // Retrieve
        let retrieved = storage.get_vector(id).unwrap().unwrap();

        // Verify equality
        assert_eq!(original, retrieved);
    }

    #[test]
    fn test_sqlite_vector_serialization() {
        // Test serialize/deserialize functions directly
        let original = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let bytes = serialize_vector(&original);
        let deserialized = deserialize_vector(&bytes).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_sqlite_vector_batch_storage() {
        use rusqlite::Connection;

        let conn = Connection::open_in_memory().unwrap();

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE hnsw_indexes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                dimension INTEGER NOT NULL,
                m INTEGER NOT NULL,
                ef_construction INTEGER NOT NULL,
                distance_metric TEXT NOT NULL,
                vector_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE hnsw_vectors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                index_id INTEGER NOT NULL,
                vector_data BLOB NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE
            );
            "#,
        )
        .unwrap();

        conn.execute(
            "INSERT INTO hnsw_indexes (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params!["test_index", 3, 16, 200, "cosine", 0, 1000, 1000],
        )
        .unwrap();

        let index_id = conn.last_insert_rowid();

        let mut storage = SQLiteVectorStorage::new(index_id, conn);

        let vectors = vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]];
        let metadatas = vec![Some(json!({"batch": 1})), Some(json!({"batch": 2}))];

        let batch = VectorBatch::new(vectors, metadatas).unwrap();
        let ids = storage.store_batch(batch).unwrap();

        assert_eq!(ids.len(), 2);
        assert_eq!(ids, vec![1, 2]);

        // Verify batch storage
        assert_eq!(storage.vector_count().unwrap(), 2);
    }
}
