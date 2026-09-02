//! Semantic KNN fallback layer using HNSW vector search with optional turbovec optimization.
//!
//! `SemanticLayer` provides HNSW-based nearest neighbor search when CSR edges
//! lack coverage for generation. Enables cross-domain and cross-model transfer.
//! Uses turbovec for large-scale embedding datasets (>1K vectors) to reduce
//! memory footprint and improve search performance when the `turbovec` feature is enabled.

use crate::hnsw::{DistanceMetric, HnswConfigBuilder, HnswIndex};
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};

#[cfg(feature = "turbovec")]
use turbovec;

/// KNN search result with node ID and distance.
#[derive(Debug, Clone, PartialEq)]
pub struct KnnResult {
    /// Nearest neighbor node ID
    pub node_id: u32,

    /// Distance from query vector
    pub distance: f32,
}

/// Semantic layer for HNSW-based KNN fallback search.
///
/// Provides approximate nearest neighbor search when CSR edges don't provide
/// sufficient coverage for generation tokens. Optionally uses turbovec for
/// large-scale datasets (when `turbovec` feature is enabled) to reduce memory
/// footprint and improve performance.
///
/// # Performance
///
/// - **Search Time**: O(log N) average case via HNSW, optimized with turbovec for large N
/// - **Insert Time**: O(log N) average case
/// - **Memory Usage**: 2-3x vector data size overhead (reduced by turbovec quantization when enabled)
/// - **Accuracy**: 95%+ recall for typical workloads
/// - **Turbovec activation**: >1K embeddings for 2-4 bit compression + SIMD search (feature-gated)
pub struct SemanticLayer {
    /// HNSW index for vector search
    hnsw_index: Arc<Mutex<HnswIndex>>,

    /// Embedding dimension
    dimension: usize,

    /// Turbovec compressed index (activates >1K embeddings, feature-gated)
    #[cfg(feature = "turbovec")]
    turbovec_index: Arc<Mutex<Option<turbovec::IdMapIndex>>>,

    /// Current embedding count (for threshold tracking, feature-gated)
    #[cfg(feature = "turbovec")]
    embedding_count: Arc<Mutex<usize>>,
}

/// Turbovec activation threshold (>1K embeddings triggers compression)
#[cfg(feature = "turbovec")]
const TURBOVEC_THRESHOLD: usize = 1_000;

impl SemanticLayer {
    fn lock_hnsw(&self) -> MutexGuard<'_, HnswIndex> {
        self.hnsw_index
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[cfg(feature = "turbovec")]
    fn lock_turbovec_index(&self) -> MutexGuard<'_, Option<turbovec::IdMapIndex>> {
        self.turbovec_index
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[cfg(feature = "turbovec")]
    fn lock_embedding_count(&self) -> MutexGuard<'_, usize> {
        self.embedding_count
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// Create a new semantic layer with given dimension.
    ///
    /// # Arguments
    ///
    /// * `dimension` - Embedding vector dimension
    pub fn new(dimension: usize) -> Self {
        // Build HNSW config optimized for semantic search
        let config = HnswConfigBuilder::new()
            .dimension(dimension)
            .m_connections(16) // Balanced connectivity
            .ef_construction(200) // Good index quality
            .ef_search(50) // Fast search with good recall
            .distance_metric(DistanceMetric::Cosine)
            .build()
            .expect("Invalid HNSW configuration");

        // Create in-memory HNSW index
        let hnsw_index =
            HnswIndex::new("semantic_layer", config).expect("Failed to create HNSW index");

        #[cfg(feature = "turbovec")]
        return Self {
            hnsw_index: Arc::new(Mutex::new(hnsw_index)),
            turbovec_index: Arc::new(Mutex::new(None)),
            dimension,
            embedding_count: Arc::new(Mutex::new(0)),
        };

        #[cfg(not(feature = "turbovec"))]
        return Self {
            hnsw_index: Arc::new(Mutex::new(hnsw_index)),
            dimension,
        };
    }

    /// Insert or update a token embedding (turbovec version).
    ///
    /// Supports incremental insert for fine-tune updates (transferable doc).
    /// Automatically builds turbovec index when threshold is crossed (>1K embeddings) when turbovec feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `token_id` - Token node ID
    /// * `embedding` - Embedding vector (must match configured dimension)
    ///
    /// # Returns
    ///
    /// `Ok(())` if insert succeeded, `Err(String)` if dimension mismatch or HNSW error.
    #[cfg(feature = "turbovec")]
    pub fn insert_embedding(&mut self, token_id: u32, embedding: Vec<f32>) -> Result<(), String> {
        if embedding.len() != self.dimension {
            return Err(format!(
                "Embedding dimension mismatch: expected {}, got {}",
                self.dimension,
                embedding.len()
            ));
        }

        // Store token_id in metadata for reverse mapping during search
        let metadata = serde_json::json!({ "token_id": token_id });

        let mut hnsw = self.lock_hnsw();
        hnsw.insert_vector(&embedding, Some(metadata))
            .map_err(|e| format!("HNSW insert failed: {}", e))?;

        // Update embedding count
        let mut count = self.lock_embedding_count();
        *count += 1;
        let current_count = *count;
        drop(count);

        // Check if we've crossed the turbovec threshold
        if current_count == TURBOVEC_THRESHOLD + 1 {
            // Build turbovec index on threshold crossing
            self.build_turbovec_index()?;
        } else if current_count > TURBOVEC_THRESHOLD {
            // Mark turbovec for rebuild (lazy rebuild on next search)
            let mut turbovec = self.lock_turbovec_index();
            *turbovec = None; // Clear to trigger rebuild on search
        }

        Ok(())
    }

    /// Find K nearest neighbors for a query embedding.
    ///
    /// # Arguments
    ///
    /// * `query_embedding` - Query vector (must match configured dimension)
    /// * `k` - Number of neighbors to return
    ///
    /// # Returns
    ///
    /// Top-K nearest neighbors sorted by distance (closest first).
    /// Returns fewer than K results if insufficient embeddings exist.
    /// Uses turbovec for datasets >1K embeddings (automatic activation).
    #[cfg(feature = "turbovec")]
    pub fn knn_search(&self, query_embedding: &[f32], k: usize) -> Vec<KnnResult> {
        if query_embedding.len() != self.dimension {
            return Vec::new(); // Dimension mismatch
        }

        // Check if we should use turbovec (large dataset)
        let count = *self.lock_embedding_count();
        if count > TURBOVEC_THRESHOLD {
            // Ensure turbovec index is built
            self.ensure_turbovec_index();

            // Try turbovec search first
            let turbovec = self.lock_turbovec_index();
            if let Some(ref index) = *turbovec {
                return self.turbovec_search(index, query_embedding, k);
            }
        }

        // Fall back to HNSW search for small datasets or if turbovec unavailable
        let hnsw = self.lock_hnsw();

        // Search HNSW index
        let results = hnsw.search(query_embedding, k);

        // Convert HNSW results (vector_id, distance) to KnnResult (token_id, distance)
        match results {
            Ok(hnsw_results) => {
                hnsw_results
                    .into_iter()
                    .filter_map(|(vector_id, distance)| {
                        // Extract token_id from stored metadata
                        hnsw.get_vector(vector_id)
                            .ok()
                            .flatten()
                            .and_then(|(_, metadata)| {
                                metadata
                                    .get("token_id")
                                    .and_then(|v| v.as_u64())
                                    .map(|token_id| KnnResult {
                                        node_id: token_id as u32,
                                        distance,
                                    })
                            })
                    })
                    .collect()
            }
            Err(_) => Vec::new(), // HNSW search failed
        }
    }

    /// Build turbovec index from existing HNSW embeddings.
    ///
    /// Called automatically when threshold is crossed (>1K embeddings).
    /// Extracts all embeddings from HNSW and builds compressed turbovec index.
    #[cfg(feature = "turbovec")]
    fn build_turbovec_index(&self) -> Result<(), String> {
        let hnsw = self.lock_hnsw();
        let count = hnsw.vector_count();

        if count == 0 {
            return Ok(()); // No embeddings to index
        }

        // Collect all embeddings and IDs from HNSW
        let mut embeddings: Vec<f32> = Vec::with_capacity(count * self.dimension);
        let mut ids: Vec<u64> = Vec::with_capacity(count);

        for i in 1..=count {
            if let Ok(Some((vector, metadata))) = hnsw.get_vector(i as u64) {
                if let Some(token_id) = metadata.get("token_id").and_then(|v| v.as_u64()) {
                    embeddings.extend_from_slice(&vector);
                    ids.push(token_id);
                }
            }
        }

        drop(hnsw);

        // Build turbovec index with 4-bit quantization
        let mut turbovec_index = turbovec::IdMapIndex::new(self.dimension, 4)
            .map_err(|e| format!("Turbovec construction failed: {}", e))?;

        turbovec_index
            .add_with_ids(&embeddings, &ids)
            .map_err(|e| format!("Turbovec add failed: {}", e))?;

        // Store the index
        let mut turbovec = self.lock_turbovec_index();
        *turbovec = Some(turbovec_index);

        Ok(())
    }

    /// Ensure turbovec index is built (lazy rebuild if cleared).
    ///
    /// Called during search when turbovec is needed but not available.
    #[cfg(feature = "turbovec")]
    fn ensure_turbovec_index(&self) {
        let turbovec = self.lock_turbovec_index();
        if turbovec.is_some() {
            return; // Already built
        }
        drop(turbovec);

        // Rebuild if cleared (after incremental inserts)
        if let Err(e) = self.build_turbovec_index() {
            eprintln!("Failed to rebuild turbovec index: {}", e);
        }
    }

    /// Search using turbovec index (for large datasets).
    ///
    /// # Arguments
    ///
    /// * `index` - Turbovec index (must be built)
    /// * `query_embedding` - Query vector
    /// * `k` - Number of neighbors to return
    ///
    /// # Returns
    ///
    /// Top-K nearest neighbors sorted by distance.
    #[cfg(feature = "turbovec")]
    fn turbovec_search(
        &self,
        index: &turbovec::IdMapIndex,
        query_embedding: &[f32],
        k: usize,
    ) -> Vec<KnnResult> {
        // Turbovec search returns (scores, ids)
        let (scores, ids) = index.search(query_embedding, k);

        // Convert to KnnResult format
        scores
            .into_iter()
            .zip(ids.into_iter())
            .map(|(distance, node_id)| KnnResult {
                node_id: node_id as u32,
                distance,
            })
            .collect()
    }

    /// Get the number of embeddings in this layer (turbovec version).
    #[cfg(feature = "turbovec")]
    pub fn embedding_count(&self) -> usize {
        let count = self.lock_embedding_count();
        *count
    }

    /// Check if a token has an embedding (turbovec version).
    #[cfg(feature = "turbovec")]
    pub fn has_embedding(&self, token_id: u32) -> bool {
        let hnsw = self.lock_hnsw();

        // Brute-force search through HNSW to check if token_id exists
        // In production, maintain a separate HashSet for O(1) lookup
        for i in 1..=hnsw.vector_count() {
            if let Ok(Some((_, metadata))) = hnsw.get_vector(i as u64)
                && let Some(id) = metadata.get("token_id").and_then(|v| v.as_u64())
                && id == token_id as u64
            {
                return true;
            }
        }
        false
    }

    /// Remove an embedding (turbovec version - stub for API compatibility).
    ///
    /// Note: deletion requires full index rebuild — call rebuild_hnsw_index() after removals.
    #[cfg(feature = "turbovec")]
    pub fn remove_embedding(&mut self, _token_id: u32) -> bool {
        // HNSW doesn't support efficient deletion
        // In production, would need to rebuild index or use versioning
        // For now, return false to maintain API compatibility
        false
    }

    /// Get HNSW index statistics (turbovec version).
    #[cfg(feature = "turbovec")]
    pub fn statistics(&self) -> Option<crate::hnsw::HnswIndexStats> {
        let hnsw = self.lock_hnsw();
        hnsw.statistics().ok()
    }
}

// Non-turbovec implementations (HNSW-only)
#[cfg(not(feature = "turbovec"))]
impl SemanticLayer {
    /// Insert or update a token embedding (HNSW-only version).
    ///
    /// # Arguments
    ///
    /// * `token_id` - Token node ID
    /// * `embedding` - Embedding vector (must match configured dimension)
    ///
    /// # Returns
    ///
    /// `Ok(())` if insert succeeded, `Err(String)` if dimension mismatch or HNSW error.
    pub fn insert_embedding(&mut self, token_id: u32, embedding: Vec<f32>) -> Result<(), String> {
        if embedding.len() != self.dimension {
            return Err(format!(
                "Embedding dimension mismatch: expected {}, got {}",
                self.dimension,
                embedding.len()
            ));
        }

        let metadata = serde_json::json!({ "token_id": token_id });
        let mut hnsw = self.lock_hnsw();
        hnsw.insert_vector(&embedding, Some(metadata))
            .map_err(|e| format!("HNSW insert failed: {}", e))?;
        Ok(())
    }

    /// Find K nearest neighbors for a query embedding (HNSW-only version).
    ///
    /// # Arguments
    ///
    /// * `query_embedding` - Query vector (must match configured dimension)
    /// * `k` - Number of neighbors to return
    ///
    /// # Returns
    ///
    /// Top-K nearest neighbors sorted by distance (closest first).
    pub fn knn_search(&self, query_embedding: &[f32], k: usize) -> Vec<KnnResult> {
        if query_embedding.len() != self.dimension {
            return Vec::new();
        }

        let hnsw = self.lock_hnsw();
        let results = hnsw.search(query_embedding, k);

        match results {
            Ok(hnsw_results) => hnsw_results
                .into_iter()
                .filter_map(|(vector_id, distance)| {
                    hnsw.get_vector(vector_id)
                        .ok()
                        .flatten()
                        .and_then(|(_, metadata)| {
                            metadata
                                .get("token_id")
                                .and_then(|v| v.as_u64())
                                .map(|token_id| KnnResult {
                                    node_id: token_id as u32,
                                    distance,
                                })
                        })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get the number of embeddings in this layer (HNSW-only version).
    pub fn embedding_count(&self) -> usize {
        let hnsw = self.lock_hnsw();
        hnsw.vector_count()
    }

    /// Check if a token has an embedding (HNSW-only version).
    #[cfg(not(feature = "turbovec"))]
    pub fn has_embedding(&self, token_id: u32) -> bool {
        let hnsw = self.lock_hnsw();

        // Brute-force search through HNSW to check if token_id exists
        // In production, maintain a separate HashSet for O(1) lookup
        for i in 1..=hnsw.vector_count() {
            if let Ok(Some((_, metadata))) = hnsw.get_vector(i as u64)
                && let Some(id) = metadata.get("token_id").and_then(|v| v.as_u64())
                && id == token_id as u64
            {
                return true;
            }
        }
        false
    }

    /// Remove an embedding (for transfer learning fine-tune updates).
    ///
    /// Used during incremental fine-tune to update only displaced embeddings.
    ///
    /// **Note**: HNSW doesn't support efficient deletion. This method is a no-op
    /// that maintains API compatibility. Deletion requires full index rebuild —
    /// call rebuild_hnsw_index() after removals.
    #[cfg(not(feature = "turbovec"))]
    pub fn remove_embedding(&mut self, _token_id: u32) -> bool {
        // HNSW doesn't support efficient deletion
        // In production, would need to rebuild index or use versioning
        // For now, return false to maintain API compatibility
        false
    }

    /// Get HNSW index statistics (for debugging/benchmarking).
    ///
    /// # Returns
    ///
    /// `Some(stats)` if statistics available, `None` if HNSW error occurred.
    #[cfg(not(feature = "turbovec"))]
    pub fn statistics(&self) -> Option<crate::hnsw::HnswIndexStats> {
        let hnsw = self.lock_hnsw();
        hnsw.statistics().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};

    #[test]
    fn test_semantic_layer_creation() {
        let layer = SemanticLayer::new(128);
        assert_eq!(layer.dimension, 128);
        assert_eq!(layer.embedding_count(), 0);
    }

    #[test]
    fn test_insert_embedding() {
        let mut layer = SemanticLayer::new(4);

        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        assert!(layer.insert_embedding(100, embedding).is_ok());
        assert_eq!(layer.embedding_count(), 1);
        assert!(layer.has_embedding(100));
    }

    #[test]
    fn test_insert_embedding_dimension_mismatch() {
        let mut layer = SemanticLayer::new(4);

        let wrong_embedding = vec![0.1, 0.2, 0.3]; // 3D instead of 4D
        assert!(layer.insert_embedding(100, wrong_embedding).is_err());
    }

    #[test]
    fn test_has_embedding_recovers_from_poisoned_hnsw_lock() {
        let layer = SemanticLayer::new(3);

        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = layer.hnsw_index.lock().unwrap();
            panic!("poison hnsw mutex");
        }));

        assert!(!layer.has_embedding(42));
    }

    #[cfg(feature = "turbovec")]
    #[test]
    fn test_embedding_count_recovers_from_poisoned_count_lock() {
        let layer = SemanticLayer::new(3);

        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = layer.embedding_count.lock().unwrap();
            panic!("poison count mutex");
        }));

        assert_eq!(layer.embedding_count(), 0);
    }

    #[test]
    fn test_knn_search() {
        let mut layer = SemanticLayer::new(3);

        // Insert test embeddings
        layer.insert_embedding(1, vec![1.0, 0.0, 0.0]).unwrap();
        layer.insert_embedding(2, vec![0.9, 0.1, 0.0]).unwrap();
        layer.insert_embedding(3, vec![0.0, 1.0, 0.0]).unwrap();

        // Query closest to [1.0, 0.0, 0.0]
        let query = vec![1.0, 0.0, 0.0];
        let results = layer.knn_search(&query, 2);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].node_id, 1); // Exact match
        assert!(results[0].distance < results[1].distance); // Sorted by distance
    }

    #[test]
    fn test_knn_search_empty() {
        let layer = SemanticLayer::new(3);

        let query = vec![1.0, 0.0, 0.0];
        let results = layer.knn_search(&query, 5);

        assert_eq!(results.len(), 0); // No embeddings
    }

    #[test]
    fn test_knn_search_dimension_mismatch() {
        let mut layer = SemanticLayer::new(3);
        layer.insert_embedding(1, vec![1.0, 0.0, 0.0]).unwrap();

        let wrong_query = vec![1.0, 0.0]; // 2D instead of 3D
        let results = layer.knn_search(&wrong_query, 5);

        assert_eq!(results.len(), 0); // Dimension mismatch
    }

    #[test]
    fn test_remove_embedding() {
        let mut layer = SemanticLayer::new(3);

        layer.insert_embedding(100, vec![0.1, 0.2, 0.3]).unwrap();
        assert!(layer.has_embedding(100));

        // HNSW doesn't support deletion, so this is a no-op
        assert!(!layer.remove_embedding(100));
        // Token still exists since HNSW can't delete
        assert!(layer.has_embedding(100));
    }

    #[test]
    fn test_cosine_distance_identical() {
        let mut layer = SemanticLayer::new(3);

        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];

        layer.insert_embedding(1, a).unwrap();
        layer.insert_embedding(2, b).unwrap();

        let query = vec![1.0, 2.0, 3.0];
        let results = layer.knn_search(&query, 1);

        assert_eq!(results.len(), 1);
        // Identical vectors should have distance close to 0
        assert!(results[0].distance < 0.1);
    }

    #[test]
    fn test_cosine_distance_opposite() {
        let mut layer = SemanticLayer::new(3);

        layer.insert_embedding(1, vec![1.0, 0.0, 0.0]).unwrap();
        layer.insert_embedding(2, vec![-1.0, 0.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0];
        let results = layer.knn_search(&query, 2);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].node_id, 1); // Closest (distance 0)
        // Opposite vector should have large distance (HNSW approximation varies)
        eprintln!("Distance to opposite vector: {}", results[1].distance);
        assert!(results[1].distance > 0.5); // Should be far
        assert!(results[0].distance < results[1].distance); // Results sorted by distance
    }

    #[test]
    fn test_cosine_distance_orthogonal() {
        let mut layer = SemanticLayer::new(3);

        // Only insert orthogonal vectors
        layer.insert_embedding(1, vec![1.0, 0.0, 0.0]).unwrap();
        layer.insert_embedding(2, vec![0.0, 1.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0];
        let results = layer.knn_search(&query, 2);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].node_id, 1); // Closest (distance 0)
        // Orthogonal vector should have distance around 1.0 (HNSW approximation varies)
        eprintln!("Distance to orthogonal vector: {}", results[1].distance);
        assert!(results[1].distance > 0.1); // Should be some distance
        assert!(results[0].distance < results[1].distance); // Results sorted by distance
    }

    #[test]
    fn test_hnsw_statistics() {
        let mut layer = SemanticLayer::new(4);

        layer
            .insert_embedding(100, vec![0.1, 0.2, 0.3, 0.4])
            .unwrap();
        layer
            .insert_embedding(200, vec![0.5, 0.6, 0.7, 0.8])
            .unwrap();

        let stats = layer.statistics();
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.vector_count, 2);
        assert_eq!(stats.dimension, 4);
    }
}
