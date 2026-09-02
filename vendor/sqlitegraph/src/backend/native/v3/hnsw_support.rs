use super::V3Backend;
use crate::SqliteGraphError;
use crate::hnsw::HnswIndex;
use std::sync::{Arc, MutexGuard};

#[path = "hnsw_support/index_ops_support.rs"]
mod index_ops_support;
#[path = "hnsw_support/insert_support.rs"]
mod insert_support;
#[path = "hnsw_support/search_support.rs"]
mod search_support;
#[cfg(feature = "turbovec")]
#[path = "hnsw_support/turbovec_support.rs"]
mod turbovec_support;

/// Metadata wrapper for HNSW index with turbovec integration.
///
/// Tracks embedding count and turbovec compression state.
/// Automatically activates turbovec (4-bit quantization) at 1K vectors.
#[derive(Clone)]
pub(super) struct HnswIndexMetadata {
    /// HNSW index (always available, works for small datasets)
    pub(super) hnsw_index: Arc<std::sync::Mutex<HnswIndex>>,
    /// Turbovec compressed index (feature-gated, activates at 1K vectors)
    #[cfg(feature = "turbovec")]
    pub(super) turbovec_index: Arc<std::sync::Mutex<Option<turbovec::IdMapIndex>>>,
    /// Embedding count tracker (triggers turbovec at threshold)
    #[cfg(feature = "turbovec")]
    pub(super) embedding_count: Arc<std::sync::Mutex<usize>>,
    /// Vector dimension (must match all insertions)
    pub(super) dimension: usize,
}

impl HnswIndexMetadata {
    pub(super) fn lock_hnsw(
        &self,
        index_name: &str,
    ) -> Result<MutexGuard<'_, HnswIndex>, SqliteGraphError> {
        self.hnsw_index.lock().map_err(|_| {
            SqliteGraphError::validation(format!("HNSW index lock poisoned: {}", index_name))
        })
    }

    #[cfg(feature = "turbovec")]
    pub(super) fn lock_turbovec(
        &self,
        index_name: &str,
    ) -> Result<MutexGuard<'_, Option<turbovec::IdMapIndex>>, SqliteGraphError> {
        self.turbovec_index.lock().map_err(|_| {
            SqliteGraphError::validation(format!("Turbovec index lock poisoned: {}", index_name))
        })
    }

    #[cfg(feature = "turbovec")]
    pub(super) fn lock_embedding_count(
        &self,
        index_name: &str,
    ) -> Result<MutexGuard<'_, usize>, SqliteGraphError> {
        self.embedding_count.lock().map_err(|_| {
            SqliteGraphError::validation(format!(
                "HNSW embedding count lock poisoned: {}",
                index_name
            ))
        })
    }
}

/// Turbovec activation threshold: 1K vectors triggers compression.
#[cfg(feature = "turbovec")]
pub(super) const TURBOVEC_THRESHOLD: usize = 1_000;

#[cfg(feature = "turbovec")]
pub(super) const TURBOVEC_BIT_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HnswSearchConfig {
    pub force_exact: bool,
    pub ef_search_override: Option<usize>,
}

impl V3Backend {
    fn hnsw_metadata(&self, index_name: &str) -> Result<HnswIndexMetadata, SqliteGraphError> {
        index_ops_support::hnsw_metadata(self, index_name)
    }

    fn validate_hnsw_vector_dimension(
        metadata_arc: &HnswIndexMetadata,
        vector: &[f32],
        expected_label: &str,
    ) -> Result<(), SqliteGraphError> {
        index_ops_support::validate_hnsw_vector_dimension(metadata_arc, vector, expected_label)
    }

    fn hnsw_exact_search(
        &self,
        metadata_arc: &HnswIndexMetadata,
        query_vector: &[f32],
        k: usize,
        ef_search_override: Option<usize>,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        search_support::hnsw_exact_search(metadata_arc, query_vector, k, ef_search_override)
    }

    pub fn create_hnsw_index(
        &self,
        index_name: &str,
        dimension: usize,
    ) -> Result<(), SqliteGraphError> {
        index_ops_support::create_hnsw_index(self, index_name, dimension)
    }

    pub fn has_hnsw_index(&self, index_name: &str) -> bool {
        index_ops_support::has_hnsw_index(self, index_name)
    }

    pub fn hnsw_embedding_count(&self, index_name: &str) -> Result<usize, SqliteGraphError> {
        index_ops_support::hnsw_embedding_count(self, index_name)
    }

    pub fn hnsw_turbovec_ready(&self, index_name: &str) -> Result<bool, SqliteGraphError> {
        index_ops_support::hnsw_turbovec_ready(self, index_name)
    }

    pub fn get_hnsw_index(
        &self,
        index_name: &str,
    ) -> Result<Option<Arc<std::sync::Mutex<HnswIndex>>>, SqliteGraphError> {
        index_ops_support::get_hnsw_index(self, index_name)
    }

    #[cfg(feature = "turbovec")]
    pub fn insert_hnsw_vector(
        &self,
        index_name: &str,
        vector: &[f32],
        metadata: Option<serde_json::Value>,
    ) -> Result<(), SqliteGraphError> {
        insert_support::insert_hnsw_vector_turbovec(self, index_name, vector, metadata)
    }

    #[cfg(not(feature = "turbovec"))]
    pub fn insert_hnsw_vector(
        &self,
        index_name: &str,
        vector: &[f32],
        metadata: Option<serde_json::Value>,
    ) -> Result<(), SqliteGraphError> {
        insert_support::insert_hnsw_vector(self, index_name, vector, metadata)
    }

    pub fn delete_hnsw_index(&self, index_name: &str) -> Result<(), SqliteGraphError> {
        index_ops_support::delete_hnsw_index(self, index_name)
    }

    #[cfg(feature = "turbovec")]
    fn build_turbovec_index(&self, index_name: &str) -> Result<(), SqliteGraphError> {
        turbovec_support::build_turbovec_index(self, index_name)
    }

    #[cfg(feature = "turbovec")]
    fn ensure_turbovec_index(&self, index_name: &str) -> Result<(), SqliteGraphError> {
        turbovec_support::ensure_turbovec_index(self, index_name)
    }

    #[cfg(feature = "turbovec")]
    fn turbovec_search(
        &self,
        index: &turbovec::IdMapIndex,
        query_vector: &[f32],
        k: usize,
    ) -> Vec<(i64, f32)> {
        turbovec_support::turbovec_search(index, query_vector, k)
    }

    pub fn hnsw_vector_search(
        &self,
        index_name: &str,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        self.hnsw_vector_search_with_config(
            index_name,
            query_vector,
            k,
            HnswSearchConfig::default(),
        )
    }

    pub fn hnsw_vector_search_with_config(
        &self,
        index_name: &str,
        query_vector: &[f32],
        k: usize,
        config: HnswSearchConfig,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        let metadata_arc = self.hnsw_metadata(index_name)?;
        Self::validate_hnsw_vector_dimension(&metadata_arc, query_vector, "Query vector")?;
        search_support::hnsw_vector_search_with_config(
            self,
            index_name,
            &metadata_arc,
            query_vector,
            k,
            config,
        )
    }
}
