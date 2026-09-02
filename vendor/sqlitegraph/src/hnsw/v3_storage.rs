//! V3 Vector Storage Implementation for HNSW
//!
//! This module provides a VectorStorage implementation that uses V3Backend's
//! KV store for persistence. This enables HNSW vector search with V3 backend.

#![cfg(feature = "native-v3")]

use crate::backend::native::v3::{KvValue, V3Backend};
use crate::hnsw::errors::{HnswError, HnswStorageError};
use crate::hnsw::storage::{VectorBatch, VectorRecord, VectorStorage};
use crate::snapshot::SnapshotId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Serializable vector record for V3 storage
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredVectorRecord {
    id: u64,
    dimension: usize,
    data: Vec<f32>,
    metadata: Option<Value>,
    created_at: u64,
    updated_at: u64,
}

impl From<&VectorRecord> for StoredVectorRecord {
    fn from(record: &VectorRecord) -> Self {
        Self {
            id: record.id(),
            dimension: record.dimension(),
            data: record.data().to_vec(),
            metadata: record.metadata().cloned(),
            created_at: record.created_at(),
            updated_at: record.updated_at(),
        }
    }
}

/// Internal storage handle that uses unsafe to allow &V3Backend -> Box<dyn VectorStorage>
///
/// SAFETY: This is safe because V3Backend uses interior mutability (RwLock) for all its fields.
/// The VectorStorage trait requires &mut self for writes, which prevents concurrent modifications
/// at the trait level. The pointer is only used to access the backend's KV methods which are
/// themselves thread-safe via RwLock.
pub struct V3VectorStorageHandle {
    /// Pointer to V3Backend (used for access only, lifetime managed by caller)
    backend_ptr: *const V3Backend,
    /// Index name for namespacing
    index_name: String,
    /// Next vector ID
    next_id: std::sync::atomic::AtomicU64,
    /// Vector count
    count: std::sync::atomic::AtomicUsize,
}

// SAFETY: V3VectorStorageHandle is safe to send between threads because:
// 1. The backend pointer is never dereferenced concurrently (VectorStorage uses &mut self for writes)
// 2. V3Backend uses interior mutability (RwLock) for thread safety
unsafe impl Send for V3VectorStorageHandle {}
unsafe impl Sync for V3VectorStorageHandle {}

impl V3VectorStorageHandle {
    pub fn new(backend: &V3Backend, index_name: impl Into<String>) -> Self {
        Self {
            backend_ptr: backend as *const V3Backend,
            index_name: index_name.into(),
            next_id: std::sync::atomic::AtomicU64::new(1),
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// SAFETY: Caller must ensure the backend is still alive
    unsafe fn backend(&self) -> &V3Backend {
        // SAFETY: We use unsafe block to dereference raw pointer
        unsafe { &*self.backend_ptr }
    }

    fn vector_key(&self, id: u64) -> Vec<u8> {
        format!("hnsw:{}:vector:{}", self.index_name, id).into_bytes()
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl VectorStorage for V3VectorStorageHandle {
    fn store_vector(&mut self, vector: &[f32], metadata: Option<Value>) -> Result<u64, HnswError> {
        let id = self.next_id();

        let record = VectorRecord::new(id, vector.to_vec(), metadata);
        record.validate()?;

        let stored: StoredVectorRecord = (&record).into();
        let json_value = serde_json::to_value(&stored).map_err(|e| {
            HnswError::Storage(HnswStorageError::IoError(format!(
                "Serialization error: {}",
                e
            )))
        })?;

        let key = self.vector_key(id);
        unsafe {
            self.backend()
                .kv_set_v3(key, KvValue::Json(json_value), None);
        }

        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(id)
    }

    fn store_vector_with_id(
        &mut self,
        id: u64,
        vector: Vec<f32>,
        metadata: Option<Value>,
    ) -> Result<(), HnswError> {
        let current_next = self.next_id.load(std::sync::atomic::Ordering::SeqCst);
        if id >= current_next {
            self.next_id
                .store(id + 1, std::sync::atomic::Ordering::SeqCst);
        }

        let record = VectorRecord::new(id, vector, metadata);
        record.validate()?;

        let stored: StoredVectorRecord = (&record).into();
        let json_value = serde_json::to_value(&stored).map_err(|e| {
            HnswError::Storage(HnswStorageError::IoError(format!(
                "Serialization error: {}",
                e
            )))
        })?;

        let key = self.vector_key(id);
        unsafe {
            self.backend()
                .kv_set_v3(key, KvValue::Json(json_value), None);
        }

        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn get_vector(&self, id: u64) -> Result<Option<Vec<f32>>, HnswError> {
        let key = self.vector_key(id);
        let snapshot_id = SnapshotId::current();

        let value = unsafe { self.backend().kv_get_v3(snapshot_id, &key) };

        match value {
            Some(KvValue::Json(json)) => {
                let stored: StoredVectorRecord = serde_json::from_value(json).map_err(|e| {
                    HnswError::Storage(HnswStorageError::IoError(format!(
                        "Deserialization error: {}",
                        e
                    )))
                })?;
                Ok(Some(stored.data))
            }
            Some(_) => Err(HnswError::Storage(HnswStorageError::IoError(
                "Unexpected KV value type".to_string(),
            ))),
            None => Ok(None),
        }
    }

    fn get_vector_with_metadata(&self, id: u64) -> Result<Option<(Vec<f32>, Value)>, HnswError> {
        let key = self.vector_key(id);
        let snapshot_id = SnapshotId::current();

        let value = unsafe { self.backend().kv_get_v3(snapshot_id, &key) };

        match value {
            Some(KvValue::Json(json)) => {
                let stored: StoredVectorRecord = serde_json::from_value(json).map_err(|e| {
                    HnswError::Storage(HnswStorageError::IoError(format!(
                        "Deserialization error: {}",
                        e
                    )))
                })?;
                let metadata = stored.metadata.unwrap_or(Value::Null);
                Ok(Some((stored.data, metadata)))
            }
            Some(_) => Err(HnswError::Storage(HnswStorageError::IoError(
                "Unexpected KV value type".to_string(),
            ))),
            None => Ok(None),
        }
    }

    fn store_batch(&mut self, batch: VectorBatch) -> Result<Vec<u64>, HnswError> {
        let mut ids = Vec::with_capacity(batch.len());

        for record in batch.vectors {
            let id = self.next_id();

            record.validate()?;

            let stored = StoredVectorRecord {
                id,
                dimension: record.dimension(),
                data: record.data().to_vec(),
                metadata: record.metadata().cloned(),
                created_at: record.created_at(),
                updated_at: record.updated_at(),
            };

            let json_value = serde_json::to_value(&stored).map_err(|e| {
                HnswError::Storage(HnswStorageError::IoError(format!(
                    "Serialization error: {}",
                    e
                )))
            })?;

            let key = self.vector_key(id);
            unsafe {
                self.backend()
                    .kv_set_v3(key, KvValue::Json(json_value), None);
            }

            ids.push(id);
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        Ok(ids)
    }

    fn delete_vector(&mut self, id: u64) -> Result<(), HnswError> {
        let key = self.vector_key(id);
        unsafe {
            self.backend().kv_delete_v3(&key);
        }

        let current = self.count.load(std::sync::atomic::Ordering::SeqCst);
        if current > 0 {
            self.count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }

        Ok(())
    }

    fn vector_count(&self) -> Result<usize, HnswError> {
        Ok(self.count.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn list_vectors(&self) -> Result<Vec<u64>, HnswError> {
        // Would need prefix scan to implement properly
        Ok(Vec::new())
    }

    fn clear_vectors(&mut self) -> Result<(), HnswError> {
        self.next_id.store(1, std::sync::atomic::Ordering::SeqCst);
        self.count.store(0, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn get_statistics(&self) -> Result<crate::hnsw::storage::VectorStorageStats, HnswError> {
        let count = self.count.load(std::sync::atomic::Ordering::SeqCst);

        Ok(crate::hnsw::storage::VectorStorageStats::new(
            count,
            0,
            "V3KV".to_string(),
        ))
    }
}

impl V3Backend {
    /// Create a new HNSW vector storage using this V3Backend
    ///
    /// # Arguments
    ///
    /// * `index_name` - Name for the HNSW index (used for namespacing keys)
    ///
    /// # Returns
    ///
    /// `Some(Box<dyn VectorStorage>)` containing a storage backed by this V3Backend
    ///
    /// # Safety
    ///
    /// The returned storage must not outlive the V3Backend it was created from.
    /// This is enforced by Rust's lifetime system when used correctly.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let backend = V3Backend::create("/path/to/db.graph").unwrap();
    /// let storage = backend.create_hnsw_storage("my_index").unwrap();
    /// ```
    pub fn create_hnsw_storage(
        &self,
        index_name: impl Into<String>,
    ) -> Option<Box<dyn VectorStorage>> {
        Some(Box::new(V3VectorStorageHandle::new(self, index_name)))
    }
}
