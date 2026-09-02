//! HNSW Neighborhood Search Algorithms
//!
//! This module implements the core k-nearest neighbor (k-NN) search functionality
//! for HNSW, including dynamic candidate lists, greedy search, and layer-by-layer
//! navigation. These algorithms provide the search performance that makes HNSW
//! efficient and scalable.
//!
//! # Architecture
//!
//! - **Search Candidate**: Dynamic candidate list with priority ordering
//! - **Greedy Search**: Local search within individual layers
//! - **Layer Navigation**: Multi-level search with entry point optimization
//! - **Distance Computation**: Efficient distance-based candidate selection
//!
//! # Performance Characteristics
//!
//! - **Time Complexity**: O(log N) average case for k-NN search
//! - **Memory Usage**: O(ef) candidate list size during search
//! - **Deterministic**: Predictable search results with stable sorting
//! - **Scalable**: Efficient for both small and large result sets

use crate::hnsw::distance_metric::{DistanceMetric, compute_distance};
use crate::hnsw::errors::{HnswError, HnswIndexError};
use crate::hnsw::layer::HnswLayer;
use std::collections::{HashMap, HashSet};

/// Search candidate for k-NN algorithms
///
/// Represents a potential nearest neighbor with distance information
/// and optional additional metadata for optimization.
#[derive(Debug, Clone, PartialEq)]
struct SearchCandidate {
    /// Node identifier
    node_id: u64,

    /// Distance from query vector
    distance: f32,

    /// Layer level where candidate was found
    level: u8,
}

impl SearchCandidate {
    /// Create a new search candidate
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node identifier
    /// * `distance` - Distance from query vector
    /// * `level` - Layer level where found
    fn new(node_id: u64, distance: f32, level: u8) -> Self {
        Self {
            node_id,
            distance,
            level,
        }
    }
}

// Implement partial ordering for search candidates
impl PartialOrd for SearchCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // For min-heap behavior: smaller distance should be "greater" in priority
        // But PartialOrd should reflect natural ordering (smaller = less)
        // We want: c2 < c1 when c2.distance < c1.distance
        match self.distance.partial_cmp(&other.distance) {
            Some(std::cmp::Ordering::Equal) => Some(self.node_id.cmp(&other.node_id)),
            Some(ord) => Some(ord), // Keep natural ordering for distances
            None => None,
        }
    }
}

/// Search result containing nearest neighbors
///
/// Represents the final result of a k-NN search operation with
/// sorted neighbors and distance information.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Nearest neighbor IDs sorted by distance
    neighbors: Vec<u64>,

    /// Corresponding distances from query vector
    distances: Vec<f32>,

    /// Number of candidates examined during search
    candidates_examined: usize,

    /// Search performance metrics
    search_metrics: SearchMetrics,
}

impl SearchResult {
    /// Create a new search result
    ///
    /// # Arguments
    ///
    /// * `neighbors` - Sorted neighbor IDs
    /// * `distances` - Corresponding distances
    /// * `candidates_examined` - Number of candidates examined
    /// * `search_metrics` - Performance metrics
    fn new(
        neighbors: Vec<u64>,
        distances: Vec<f32>,
        candidates_examined: usize,
        search_metrics: SearchMetrics,
    ) -> Self {
        Self {
            neighbors,
            distances,
            candidates_examined,
            search_metrics,
        }
    }

    /// Get nearest neighbor IDs
    pub fn neighbors(&self) -> &[u64] {
        &self.neighbors
    }

    /// Get corresponding distances
    pub fn distances(&self) -> &[f32] {
        &self.distances
    }

    /// Get number of results found
    pub fn len(&self) -> usize {
        self.neighbors.len()
    }

    /// Check if search result is empty
    pub fn is_empty(&self) -> bool {
        self.neighbors.is_empty()
    }

    /// Get number of candidates examined
    pub fn candidates_examined(&self) -> usize {
        self.candidates_examined
    }

    /// Get search performance metrics
    pub fn metrics(&self) -> &SearchMetrics {
        &self.search_metrics
    }
}

/// Search performance metrics
///
/// Tracks detailed performance information for search operations
/// to enable optimization and debugging.
#[derive(Debug, Clone)]
pub struct SearchMetrics {
    /// Number of layers visited
    layers_visited: u8,

    /// Number of entry points considered
    entry_points_considered: usize,

    /// Average degree of visited nodes
    average_degree: f32,

    /// Search depth (max distance from entry point)
    search_depth: usize,
}

impl SearchMetrics {
    /// Create new search metrics
    fn new() -> Self {
        Self {
            layers_visited: 0,
            entry_points_considered: 0,
            average_degree: 0.0,
            search_depth: 0,
        }
    }

    /// Get number of layers visited
    pub fn layers_visited(&self) -> u8 {
        self.layers_visited
    }

    /// Get number of entry points considered
    pub fn entry_points_considered(&self) -> usize {
        self.entry_points_considered
    }

    /// Get average degree of visited nodes
    pub fn average_degree(&self) -> f32 {
        self.average_degree
    }

    /// Get search depth
    pub fn search_depth(&self) -> usize {
        self.search_depth
    }
}

/// HNSW neighborhood search algorithms
///
/// Provides high-performance k-nearest neighbor search using the HNSW
/// algorithm with dynamic candidate management and layer-based navigation.
pub struct NeighborhoodSearch {
    /// Distance metric for similarity computation
    distance_metric: DistanceMetric,
}

impl NeighborhoodSearch {
    /// Create a new neighborhood search instance
    ///
    /// # Arguments
    ///
    /// * `distance_metric` - Distance metric to use for similarity computation
    ///
    /// # Returns
    ///
    /// New NeighborhoodSearch instance
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use sqlitegraph::hnsw::{NeighborhoodSearch, DistanceMetric};
    ///
    /// let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
    /// // Ready for k-NN search operations
    /// ```
    pub fn new(distance_metric: DistanceMetric) -> Self {
        Self { distance_metric }
    }

    /// Find k-nearest neighbors in a single layer
    ///
    /// Performs greedy search within a specific layer to find the nearest
    /// neighbors to the query vector.
    ///
    /// # Arguments
    ///
    /// * `layer` - Target layer for search
    /// * `query_vector` - Query vector for similarity comparison
    /// * `vectors` - Vector storage (node_id -> vector mapping)
    /// * `entry_points` - Initial entry points for search
    /// * `k` - Number of neighbors to find
    ///
    /// # Returns
    ///
    /// SearchResult with k nearest neighbors or error
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use sqlitegraph::hnsw::{NeighborhoodSearch, DistanceMetric};
    ///
    /// let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
    /// let vectors: Vec<Vec<f32>> = create_test_vectors();
    /// let layer = create_test_layer();
    ///
    /// let result = search.search_layer(
    ///     &layer,
    ///     &query_vector,
    ///     &vectors,
    ///     &[0, 1],  // entry points
    ///     5         // k = 5
    /// )?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn search_layer(
        &self,
        layer: &HnswLayer,
        query_vector: &[f32],
        vectors: &HashMap<u64, Vec<f32>>,
        entry_points: &[u64],
        k: usize,
    ) -> Result<SearchResult, HnswError> {
        self.search_layer_fn(
            layer,
            query_vector,
            |node_id| vectors.get(&node_id).map(|v| v.as_slice()),
            entry_points,
            k,
        )
    }

    pub(crate) fn search_layer_fn<'a>(
        &self,
        layer: &HnswLayer,
        query_vector: &[f32],
        lookup: impl Fn(u64) -> Option<&'a [f32]>,
        entry_points: &[u64],
        k: usize,
    ) -> Result<SearchResult, HnswError> {
        if query_vector.is_empty() {
            return Err(HnswError::Index(HnswIndexError::InvalidSearchParameters));
        }

        if k == 0 {
            return Ok(SearchResult::new(vec![], vec![], 0, SearchMetrics::new()));
        }

        if entry_points.is_empty() {
            return Err(HnswError::Index(HnswIndexError::InvalidSearchParameters));
        }

        if layer.node_count() == 0 {
            return Err(HnswError::Index(HnswIndexError::IndexNotInitialized));
        }

        let mut metrics = SearchMetrics::new();
        metrics.entry_points_considered = entry_points.len();

        // Initialize candidate list with entry points
        let mut candidates = Vec::new();
        let mut visited = HashSet::new();

        // Add entry points to candidate list
        for &entry_point in entry_points {
            if layer.contains_node(entry_point) {
                let distance = self.compute_distance(
                    query_vector,
                    lookup(entry_point)
                        .ok_or(HnswError::Index(HnswIndexError::NodeNotFound(entry_point)))?,
                )?;
                candidates.push(SearchCandidate::new(entry_point, distance, layer.level()));
                visited.insert(entry_point);
            }
        }

        let mut candidates_examined = 0;
        // W: dynamic result set, capped at ef_search size (k is ef_search here)
        let mut result_set: Vec<SearchCandidate> = Vec::new();

        // Standard HNSW greedy search: expand candidates, prune result set to ef_search
        while !candidates.is_empty() {
            // Pop closest candidate
            candidates.sort_by(|a, b| {
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let candidate = candidates.remove(0);
            candidates_examined += 1;

            // Early termination: if candidate is farther than worst in result set, stop
            if result_set.len() >= k {
                let worst = result_set
                    .iter()
                    .map(|c| c.distance)
                    .fold(f32::NEG_INFINITY, f32::max);
                if candidate.distance > worst {
                    break;
                }
            }

            result_set.push(candidate.clone());

            // Expand neighbors
            if let Ok(connections) = layer.get_connections(candidate.node_id) {
                for &neighbor_id in connections {
                    if !visited.contains(&neighbor_id) {
                        visited.insert(neighbor_id);
                        if let Some(vec) = lookup(neighbor_id) {
                            let distance = self.compute_distance(query_vector, vec)?;
                            candidates.push(SearchCandidate::new(
                                neighbor_id,
                                distance,
                                layer.level(),
                            ));
                        }
                    }
                }
            }

            // Prune result set to ef_search
            if result_set.len() > k {
                result_set.sort_by(|a, b| {
                    a.distance
                        .partial_cmp(&b.distance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                result_set.truncate(k);
            }
        }

        // Sort final result set by distance
        result_set.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });

        let result_candidates = result_set;

        // Extract neighbors and distances
        let neighbors: Vec<u64> = result_candidates.iter().map(|c| c.node_id).collect();
        let distances: Vec<f32> = result_candidates.iter().map(|c| c.distance).collect();

        metrics.search_depth = visited.len();
        Ok(SearchResult::new(
            neighbors,
            distances,
            candidates_examined,
            metrics,
        ))
    }

    /// Compute distance between query and target vectors
    ///
    /// # Arguments
    ///
    /// * `query_vector` - Query vector
    /// * `target_vector` - Target vector for comparison
    ///
    /// # Returns
    ///
    /// Distance value or error if computation fails
    fn compute_distance(
        &self,
        query_vector: &[f32],
        target_vector: &[f32],
    ) -> Result<f32, HnswError> {
        if query_vector.len() != target_vector.len() {
            return Err(HnswError::Index(HnswIndexError::VectorDimensionMismatch {
                expected: query_vector.len(),
                actual: target_vector.len(),
            }));
        }

        Ok(compute_distance(
            self.distance_metric,
            query_vector,
            target_vector,
        ))
    }
}

impl Default for NeighborhoodSearch {
    fn default() -> Self {
        Self::new(DistanceMetric::Cosine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hnsw::distance_metric::DistanceMetric;

    fn create_test_vectors() -> HashMap<u64, Vec<f32>> {
        let raw: Vec<Vec<f32>> = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0],
            vec![0.5, 0.5, 0.5],
        ];
        raw.into_iter()
            .enumerate()
            .map(|(i, v)| (i as u64, v))
            .collect()
    }

    fn create_test_layer() -> HnswLayer {
        let mut layer = HnswLayer::new(0, 4);

        // Add nodes
        for i in 0..5 {
            layer.add_node(i).unwrap();
        }

        // Add some connections
        layer.add_connection(0, 1).unwrap();
        layer.add_connection(1, 2).unwrap();
        layer.add_connection(2, 3).unwrap();
        layer.add_connection(3, 4).unwrap();
        layer.add_connection(0, 3).unwrap();

        layer
    }

    #[test]
    fn test_search_candidate_creation() {
        let candidate = SearchCandidate::new(42, 0.75, 2);

        assert_eq!(candidate.node_id, 42);
        assert_eq!(candidate.distance, 0.75);
        assert_eq!(candidate.level, 2);
    }

    #[test]
    fn test_search_candidate_ordering() {
        let c1 = SearchCandidate::new(1, 0.9, 0);
        let c2 = SearchCandidate::new(2, 0.5, 0);
        let c3 = SearchCandidate::new(3, 0.7, 0);

        // Debug print to understand ordering
        println!("c1 distance: {}, c2 distance: {}", c1.distance, c2.distance);
        println!("c1 < c2: {}", c1 < c2);
        println!("c2 < c1: {}", c2 < c1);

        // Min-heap ordering (smaller distance = higher priority)
        assert!(c2 < c1); // 0.5 < 0.9
        assert!(c2 < c3); // 0.5 < 0.7
        assert!(c3 < c1); // 0.7 < 0.9
    }

    #[test]
    fn test_neighborhood_search_creation() {
        let search = NeighborhoodSearch::new(DistanceMetric::Euclidean);

        // Search should be ready for use
        assert_eq!(search.distance_metric, DistanceMetric::Euclidean);
    }

    #[test]
    fn test_neighborhood_search_default() {
        let search = NeighborhoodSearch::default();

        // Default should use cosine similarity
        assert_eq!(search.distance_metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_search_layer_basic() {
        let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
        let vectors = create_test_vectors();
        let layer = create_test_layer();
        let query_vector = vec![1.0, 0.0, 0.0]; // Similar to node 0

        let result = search
            .search_layer(&layer, &query_vector, &vectors, &[0], 3)
            .unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result.neighbors().len(), 3);
        assert_eq!(result.distances().len(), 3);

        // Node 0 should be first (identical vector)
        assert_eq!(result.neighbors()[0], 0);
        assert_eq!(result.distances()[0], 0.0);
    }

    #[test]
    fn test_search_layer_k_zero() {
        let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
        let vectors = create_test_vectors();
        let layer = create_test_layer();
        let query_vector = vec![1.0, 0.0, 0.0];

        let result = search
            .search_layer(&layer, &query_vector, &vectors, &[0], 0)
            .unwrap();

        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
        assert_eq!(result.candidates_examined(), 0);
    }

    #[test]
    fn test_search_layer_no_entry_points() {
        let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
        let vectors = create_test_vectors();
        let layer = create_test_layer();
        let query_vector = vec![1.0, 0.0, 0.0];

        let result = search.search_layer(&layer, &query_vector, &vectors, &[], 3);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            HnswError::Index(HnswIndexError::InvalidSearchParameters)
        ));
    }

    #[test]
    fn test_search_layer_empty_layer() {
        let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
        let vectors: HashMap<u64, Vec<f32>> = HashMap::new();
        let layer = HnswLayer::new(0, 4); // Empty layer
        let query_vector = vec![1.0, 0.0, 0.0];

        let result = search.search_layer(&layer, &query_vector, &vectors, &[0], 3);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            HnswError::Index(HnswIndexError::IndexNotInitialized)
        ));
    }

    #[test]
    fn test_search_layer_empty_query_vector() {
        let search = NeighborhoodSearch::new(DistanceMetric::Cosine);
        let vectors = create_test_vectors();
        let layer = create_test_layer();

        let result = search.search_layer(&layer, &[], &vectors, &[0], 3);

        assert!(
            result.is_err(),
            "search_layer should reject empty query_vector, got {:?}",
            result
        );
        assert!(matches!(
            result.unwrap_err(),
            HnswError::Index(HnswIndexError::InvalidSearchParameters)
        ));
    }

    #[test]
    fn test_search_result_accessors() {
        let result =
            SearchResult::new(vec![1, 2, 3], vec![0.1, 0.2, 0.3], 10, SearchMetrics::new());

        assert_eq!(result.len(), 3);
        assert!(!result.is_empty());
        assert_eq!(result.neighbors(), &[1, 2, 3]);
        assert_eq!(result.distances(), &[0.1, 0.2, 0.3]);
        assert_eq!(result.candidates_examined(), 10);
        assert_eq!(result.metrics().layers_visited(), 0);
    }

    #[test]
    fn test_search_result_empty() {
        let result = SearchResult::new(vec![], vec![], 0, SearchMetrics::new());

        assert_eq!(result.len(), 0);
        assert!(result.is_empty());
        assert!(result.neighbors().is_empty());
        assert!(result.distances().is_empty());
    }

    #[test]
    fn test_search_metrics() {
        let metrics = SearchMetrics::new();

        assert_eq!(metrics.layers_visited(), 0);
        assert_eq!(metrics.entry_points_considered(), 0);
        assert_eq!(metrics.average_degree(), 0.0);
        assert_eq!(metrics.search_depth(), 0);
    }

    #[test]
    fn test_compute_distance_success() {
        let search = NeighborhoodSearch::new(DistanceMetric::Euclidean);
        let query = vec![1.0, 0.0];
        let target = vec![0.0, 1.0];

        let distance = search.compute_distance(&query, &target).unwrap();

        assert!((distance - std::f32::consts::SQRT_2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_distance_dimension_mismatch() {
        let search = NeighborhoodSearch::new(DistanceMetric::Euclidean);
        let query = vec![1.0, 0.0, 0.0]; // 3D
        let target = vec![1.0, 0.0]; // 2D

        let result = search.compute_distance(&query, &target);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            HnswError::Index(HnswIndexError::VectorDimensionMismatch {
                expected: 3,
                actual: 2
            })
        ));
    }

    #[test]
    fn test_different_distance_metrics() {
        let vectors = create_test_vectors();
        let layer = create_test_layer();
        let query_vector = vec![0.3, 0.7, 0.2]; // Non-matching query

        // Test with Euclidean distance
        let euclidean_search = NeighborhoodSearch::new(DistanceMetric::Euclidean);
        let euclidean_result = euclidean_search
            .search_layer(&layer, &query_vector, &vectors, &[0], 1)
            .unwrap();

        // Test with Manhattan distance
        let manhattan_search = NeighborhoodSearch::new(DistanceMetric::Manhattan);
        let manhattan_result = manhattan_search
            .search_layer(&layer, &query_vector, &vectors, &[0], 1)
            .unwrap();

        // Test that both metrics work and produce valid results
        assert_eq!(euclidean_result.len(), 1);
        assert_eq!(manhattan_result.len(), 1);

        // Both should find the same nearest neighbor (deterministic ordering)
        assert_eq!(
            euclidean_result.neighbors()[0],
            manhattan_result.neighbors()[0]
        );

        // Distances should be different for different metrics
        assert_ne!(
            euclidean_result.distances()[0],
            manhattan_result.distances()[0]
        );
    }
}
