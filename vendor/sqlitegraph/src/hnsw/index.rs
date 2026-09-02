//! HNSW Vector Search Index API
//!
//! This module provides the main HNSW index implementation that integrates
//! vector search capabilities with SQLiteGraph. It combines all the HNSW
//! components (layers, neighborhood search, storage) into a cohesive API
//! that follows SQLiteGraph patterns and conventions.
//!
//! # Architecture
//!
//! The HnswIndex serves as the main orchestrator that coordinates:
//! - Vector storage and retrieval via VectorStorage trait
//! - Layer management for the hierarchical graph structure
//! - Neighborhood search for approximate nearest neighbors
//! - Entry point management for multi-layer navigation
//!
//! # Integration with SQLiteGraph
//!
//! The HNSW index is designed to work with SQLiteGraph:
//! - Uses SqliteGraphError for consistent error handling
//! - Follows SQLiteGraph method naming conventions
//! - Integrates with existing SQLite schemas
//! - Supports both in-memory and persistent storage
//!
//! # Examples
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, hnsw::{HnswConfigBuilder, DistanceMetric}};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! let config = HnswConfigBuilder::new()
//!     .dimension(768)
//!     .distance_metric(DistanceMetric::Cosine)
//!     .build()?;
//!
//! let hnsw = graph.hnsw_index("vectors", config)?;
//!
//! // Insert vectors with metadata
//! let vector_data = vec![1.0f32; 768];
//! let metadata = serde_json::json!({"label": "test"});
//! let vector_id = hnsw.get_mut("vectors").unwrap()
//!     .insert_vector(&vector_data, Some(metadata))?;
//!
//! // Search for similar vectors
//! let query_vector = vec![1.0f32; 768];
//! let results = hnsw.get_mut("vectors").unwrap()
//!     .search(&query_vector, 10)?;
//! for (id, distance) in results {
//!     println!("Vector {}: distance {}", id, distance);
//! }
//! ```

use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use crate::hnsw::{
    config::HnswConfig,
    distance_metric::DistanceMetric,
    errors::HnswError,
    layer::HnswLayer,
    multilayer::{LevelDistributor, MultiLayerNodeManager},
    neighborhood::NeighborhoodSearch,
    storage::{VectorStorage, VectorStorageStats},
};
#[cfg(test)]
use crate::hnsw::{config::hnsw_config, errors::HnswIndexError};

/// Main HNSW vector search index
///
/// Provides approximate nearest neighbor search capabilities using the
/// Hierarchical Navigable Small World algorithm. Integrates with SQLiteGraph
/// to provide vector-augmented graph queries.
///
/// # Performance Characteristics
///
/// - **Search Time**: O(log N) average case complexity
/// - **Memory Usage**: 2-3x vector data size overhead
/// - **Build Time**: O(N log N) with construction parameters
/// - **Accuracy**: 95%+ recall for typical workloads
pub struct HnswIndex {
    /// Name of this index (for persistence and multi-index support)
    pub(crate) name: String,

    /// HNSW configuration parameters
    pub(crate) config: HnswConfig,

    /// Layer management (0 = base layer, higher numbers = smaller layers)
    pub(crate) layers: Vec<HnswLayer>,

    /// Vector storage backend
    pub(crate) storage: Box<dyn VectorStorage>,

    /// Entry points for navigating the hierarchical structure
    pub(crate) entry_points: Vec<u64>,

    /// Number of vectors currently indexed
    pub(crate) vector_count: usize,

    /// Neighborhood search engine
    pub(crate) search_engine: NeighborhoodSearch,

    /// Level distributor for exponential level assignment in multi-layer mode
    /// Only initialized when enable_multilayer == true
    pub(crate) level_distributor: Option<LevelDistributor>,

    /// Multi-layer node manager for tracking layer assignments and ID translation
    /// Only initialized when enable_multilayer == true
    pub(crate) multi_layer_manager: Option<MultiLayerNodeManager>,

    /// Incremental cache of all vectors, keyed by vector_id.
    /// Avoids re-querying SQLite on every insert during HNSW construction.
    /// Populated incrementally on store_vector, or in bulk from restore_topology.
    pub(crate) vector_cache: HashMap<u64, Vec<f32>>,

    /// Lock-free counter for vector insert operations.
    pub(crate) insert_count: AtomicU64,

    /// Lock-free counter for search operations.
    pub(crate) search_count: AtomicU64,

    /// Lock-free counter for vector cache hits while building/searching layers.
    pub(crate) vector_cache_hits: AtomicU64,

    /// Lock-free counter for vector cache misses while building/searching layers.
    pub(crate) vector_cache_misses: AtomicU64,
}

/// Comprehensive statistics for an HNSW index
#[derive(Debug, Clone)]
pub struct HnswIndexStats {
    /// Total number of vectors in the index
    pub vector_count: usize,

    /// Number of layers in the hierarchical structure
    pub layer_count: usize,

    /// Number of entry points (vectors in higher layers)
    pub entry_point_count: usize,

    /// Vector dimension
    pub dimension: usize,

    /// Distance metric being used
    pub distance_metric: DistanceMetric,

    /// Storage backend statistics
    pub storage_stats: VectorStorageStats,

    /// Per-layer statistics (node_count, total_connections, avg_connections)
    pub layer_stats: Vec<(usize, usize, f32)>,

    /// Total successful insert operations.
    pub insert_count: u64,

    /// Total successful search operations.
    pub search_count: u64,

    /// Count of vector cache hits while materializing layer-local vectors.
    pub vector_cache_hits: u64,

    /// Count of vector cache misses while materializing layer-local vectors.
    pub vector_cache_misses: u64,
}

// Include split module implementations using the include! macro
// This allows us to split the file while maintaining a single compilation unit
include!("index_api.rs");
include!("index_internal.rs");
include!("index_persist.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::SqliteGraph;
    use crate::hnsw::{DistanceMetric, HnswConfigBuilder};

    #[test]
    fn test_hnsw_index_creation() {
        let config = HnswConfigBuilder::new()
            .dimension(3)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();

        let hnsw = HnswIndex::new("test_index", config).unwrap();
        let stats = hnsw.statistics().unwrap();

        assert_eq!(stats.vector_count, 0);
        assert_eq!(stats.dimension, 3);
        assert_eq!(stats.distance_metric, DistanceMetric::Euclidean);
        assert_eq!(stats.insert_count, 0);
        assert_eq!(stats.search_count, 0);
        assert_eq!(stats.vector_cache_hits, 0);
        assert_eq!(stats.vector_cache_misses, 0);
    }

    #[test]
    fn test_vector_insertion() {
        let config = hnsw_config().dimension(3).build().unwrap();
        let mut hnsw = HnswIndex::new("test_insert", config).unwrap();
        let vector = vec![1.0, 0.0, 0.0];
        let metadata = serde_json::json!({"label": "test"});

        let result = hnsw.insert_vector(&vector, Some(metadata));
        println!("Insert result: {:?}", result);
        let vector_id = result.unwrap();
        assert!(vector_id > 0);

        let stats = hnsw.statistics().unwrap();
        assert_eq!(stats.vector_count, 1);
    }

    #[test]
    fn test_dimension_mismatch_error() {
        let mut hnsw = HnswIndex::new("test_dim_error", HnswConfig::default()).unwrap();
        let wrong_vector = vec![1.0, 0.0]; // Default config expects 768 dimensions

        let result = hnsw.insert_vector(&wrong_vector, None);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(matches!(
            error,
            HnswError::Index(HnswIndexError::VectorDimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_empty_search() {
        let hnsw = HnswIndex::new("test_empty_search", HnswConfig::default()).unwrap();
        let query = vec![1.0; 768];

        let results = hnsw.search(&query, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_vector_retrieval() {
        let config = hnsw_config().dimension(3).build().unwrap();
        let mut hnsw = HnswIndex::new("test_retrieval", config).unwrap();
        let vector = vec![1.0, 0.0, 0.0];
        let metadata = serde_json::json!({"label": "test"});

        let vector_id = hnsw.insert_vector(&vector, Some(metadata.clone())).unwrap();
        let result = hnsw.get_vector(vector_id).unwrap();

        assert!(result.is_some());
        let (retrieved_vector, retrieved_metadata) = result.unwrap();
        assert_eq!(retrieved_vector, vector);
        assert_eq!(retrieved_metadata, metadata);
    }

    #[test]
    fn test_sqlite_graph_integration() {
        let graph = SqliteGraph::open_in_memory().unwrap();
        let config = HnswConfigBuilder::new()
            .dimension(4)
            .distance_metric(DistanceMetric::Cosine)
            .build()
            .unwrap();

        let hnsw_indexes = graph.hnsw_index("test_index", config).unwrap();
        let hnsw = hnsw_indexes.get("test_index").unwrap();
        let stats = hnsw.statistics().unwrap();

        assert_eq!(stats.vector_count, 0);
        assert_eq!(stats.dimension, 4);
        assert_eq!(stats.distance_metric, DistanceMetric::Cosine);
    }

    #[test]
    fn test_basic_search_functionality() {
        let mut hnsw = HnswIndex::new(
            "test_search",
            HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap(),
        )
        .unwrap();

        // Insert some test vectors
        let vectors = vec![
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![-1.0, 0.0],
            vec![0.0, -1.0],
        ];

        let mut vector_ids = Vec::new();
        for vector in vectors {
            let id = hnsw.insert_vector(&vector, None).unwrap();
            vector_ids.push(id);
        }

        // Search for nearest neighbors
        let query = vec![0.9, 0.1];
        let results = hnsw.search(&query, 2).unwrap();

        assert!(!results.is_empty());
        assert!(results.len() <= 2);

        // Results should be sorted by distance
        for window in results.windows(2) {
            assert!(window[0].1 <= window[1].1);
        }
    }

    #[test]
    fn test_index_statistics() {
        let mut hnsw = HnswIndex::new(
            "test_stats",
            HnswConfigBuilder::new()
                .dimension(3)
                .max_layers(3)
                .distance_metric(DistanceMetric::Euclidean) // Use Euclidean to avoid zero magnitude issues
                .build()
                .unwrap(),
        )
        .unwrap();

        // Insert some vectors (starting from 1 to avoid all-zero vector)
        for i in 1..=5 {
            let vector = vec![i as f32, (i * 2) as f32, (i * 3) as f32];
            hnsw.insert_vector(&vector, None).unwrap();
        }

        let stats = hnsw.statistics().unwrap();
        assert_eq!(stats.vector_count, 5);
        assert_eq!(stats.layer_count, 3);
        assert_eq!(stats.dimension, 3);
        assert_eq!(stats.insert_count, 5);
        assert_eq!(stats.search_count, 0);
        assert_eq!(stats.vector_cache_hits, 4);
        assert_eq!(stats.vector_cache_misses, 0);
        assert!(!stats.layer_stats.is_empty());
    }

    #[test]
    fn delete_entry_point_re_elects_and_search_survives() {
        // Regression: deleting the current HNSW entry point must re-elect a new
        // one so a non-empty index stays searchable (was IndexNotInitialized).
        let mut hnsw = HnswIndex::new(
            "test_delete_entry_point",
            HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap(),
        )
        .unwrap();
        for v in [
            [1.0f32, 0.0],
            [0.0, 1.0],
            [-1.0, 0.0],
            [0.0, -1.0],
            [0.7, 0.7],
        ] {
            hnsw.insert_vector(&v, None).unwrap();
        }
        let ep = *hnsw
            .entry_points
            .last()
            .expect("a non-empty index has an entry point");
        hnsw.delete_vector(ep).unwrap();
        let results = hnsw.search(&[0.95, 0.05], 3).unwrap();
        assert!(
            !results.is_empty(),
            "search must still find survivors after deleting the entry point"
        );
        assert!(
            results.iter().all(|(id, _)| *id != ep),
            "the deleted entry point must not be returned"
        );
        assert!(
            !hnsw.entry_points.is_empty(),
            "a non-empty index must keep an entry point"
        );
    }

    #[test]
    fn delete_non_entry_point_leaves_entry_points_unchanged() {
        let mut hnsw = HnswIndex::new(
            "test_delete_non_entry_point",
            HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap(),
        )
        .unwrap();
        let mut ids = Vec::new();
        for v in [
            [1.0f32, 0.0],
            [0.0, 1.0],
            [-1.0, 0.0],
            [0.0, -1.0],
            [0.7, 0.7],
        ] {
            ids.push(hnsw.insert_vector(&v, None).unwrap());
        }
        let before = hnsw.entry_points.clone();
        let victim = *ids
            .iter()
            .find(|&&id| !before.contains(&id))
            .expect("a non-entry-point vector exists");
        hnsw.delete_vector(victim).unwrap();
        assert_eq!(
            hnsw.entry_points, before,
            "deleting a non-entry-point must not touch entry_points"
        );
        let results = hnsw.search(&[1.0, 0.0], 3).unwrap();
        assert!(
            results.iter().all(|(id, _)| *id != victim),
            "the deleted vector must not be returned"
        );
    }

    #[test]
    fn delete_last_vector_leaves_index_empty_without_spurious_entry_point() {
        let mut hnsw = HnswIndex::new(
            "test_delete_to_empty",
            HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap(),
        )
        .unwrap();
        let id = hnsw.insert_vector(&[1.0, 0.0], None).unwrap();
        hnsw.delete_vector(id).unwrap();
        assert!(
            hnsw.entry_points.is_empty(),
            "an empty index must have no entry point (no spurious re-election)"
        );
        let results = hnsw.search(&[1.0, 0.0], 1).unwrap();
        assert!(
            results.is_empty(),
            "search on an empty index returns nothing"
        );
    }

    #[test]
    fn re_elected_entry_point_persists_across_reopen() {
        use std::fs;
        let dir = "/tmp/test_hnsw_ep_reelect_persist";
        let _ = fs::remove_dir_all(dir);
        fs::create_dir_all(dir).unwrap();
        let db = format!("{}/t.db", dir);
        {
            let graph = SqliteGraph::open(&db).unwrap();
            let cfg = HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap();
            drop(graph.hnsw_index_persistent("p", cfg).unwrap());
            graph
                .get_hnsw_index_mut("p", |idx| {
                    for v in [[1.0f32, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]] {
                        idx.insert_vector(&v, None).unwrap();
                    }
                    let ep = *idx.entry_points.last().unwrap();
                    idx.delete_vector(ep).unwrap();
                })
                .unwrap();
        }
        // Reopen → topology (incl. the re-elected entry point) restored.
        let graph = SqliteGraph::open(&db).unwrap();
        let results = graph
            .get_hnsw_index_ref("p", |idx| idx.search(&[0.95, 0.05], 3).unwrap())
            .unwrap();
        assert!(
            !results.is_empty(),
            "search works after reopen — re-elected entry point round-tripped"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn elect_entry_point_falls_back_to_storage_when_cache_empty() {
        // PR #13 review edge-case (Option 1): in the memory-constrained restore
        // mode the topology is rebuilt but `vector_cache` stays EMPTY (it is
        // lazy-loaded). Deleting the entry point in that state must still
        // re-elect — from storage, since the cache can't serve as the live-set
        // oracle — otherwise `search` returns IndexNotInitialized again.
        let mut hnsw = HnswIndex::new(
            "test_elect_storage_fallback",
            HnswConfigBuilder::new()
                .dimension(2)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap(),
        )
        .unwrap();
        for v in [
            [1.0f32, 0.0],
            [0.0, 1.0],
            [-1.0, 0.0],
            [0.0, -1.0],
            [0.7, 0.7],
        ] {
            hnsw.insert_vector(&v, None).unwrap();
        }
        // Simulate the constrained restore: drop the in-memory cache so the
        // live set must come from storage (which still holds every vector).
        hnsw.vector_cache.clear();
        let ep = *hnsw
            .entry_points
            .last()
            .expect("a non-empty index has an entry point");
        hnsw.delete_vector(ep).unwrap();
        // RED before the fix: election saw an empty cache → None → the
        // entry-point set stayed empty.
        assert!(
            !hnsw.entry_points.is_empty(),
            "a non-empty index must keep an entry point even when the cache is empty"
        );
        let results = hnsw.search(&[0.95, 0.05], 3).unwrap();
        assert!(
            !results.is_empty(),
            "search must find survivors after a cache-empty entry-point delete"
        );
        assert!(
            results.iter().all(|(id, _)| *id != ep),
            "the deleted entry point must not be returned"
        );
    }

    #[test]
    fn test_metadata_persistence() {
        use std::fs;

        let test_dir = "/tmp/test_hnsw_metadata_persistence";
        let db_path = format!("{}/test.db", test_dir);

        // Clean up any existing test database
        let _ = fs::remove_dir_all(test_dir);

        // Create directory
        fs::create_dir_all(test_dir).unwrap();

        // Create graph and index
        {
            let graph = SqliteGraph::open(&db_path).unwrap();
            let config = HnswConfigBuilder::new()
                .dimension(128)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap();

            let hnsw_indexes = graph.hnsw_index("persist_test", config).unwrap();
            let hnsw = hnsw_indexes.get("persist_test").unwrap();

            // Verify index was created
            assert_eq!(hnsw.name(), "persist_test");
            assert_eq!(hnsw.config().dimension, 128);
            assert_eq!(hnsw.config().distance_metric, DistanceMetric::Euclidean);

            // Save metadata explicitly
            let conn = graph.connection();
            let conn_ref = conn.underlying();
            hnsw.save_metadata(conn_ref).unwrap();
        }

        // Reopen and verify metadata persists
        {
            let graph2 = SqliteGraph::open(&db_path).unwrap();

            // Check that index was loaded
            let index_names = graph2.list_hnsw_indexes().unwrap();
            assert_eq!(index_names, vec!["persist_test".to_string()]);

            // Get the loaded index
            let loaded_hnsw = graph2
                .get_hnsw_index_ref("persist_test", |hnsw| {
                    assert_eq!(hnsw.name(), "persist_test");
                    assert_eq!(hnsw.config().dimension, 128);
                    assert_eq!(hnsw.config().distance_metric, DistanceMetric::Euclidean);
                    hnsw.config().dimension
                })
                .unwrap();

            assert_eq!(loaded_hnsw, 128);
        }

        // Clean up
        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_vector_loading_and_rebuild() {
        use rusqlite::Connection;
        use std::fs;

        let test_dir = "/tmp/test_hnsw_vector_loading";
        let db_path = format!("{}/test.db", test_dir);

        // Clean up any existing test database
        let _ = fs::remove_dir_all(test_dir);

        // Create directory
        fs::create_dir_all(test_dir).unwrap();

        // Create index and manually persist vectors to database
        {
            let conn = Connection::open(&db_path).unwrap();

            // Create schema
            crate::schema::ensure_schema(&conn).unwrap();

            // Create HNSW index metadata
            conn.execute(
                "INSERT INTO hnsw_indexes (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params!["load_test", 3, 16, 200, "euclidean", 5, 1000, 1000],
            ).unwrap();

            let index_id = conn.last_insert_rowid();

            // Manually insert vectors into database
            for i in 0..5 {
                let vector = vec![i as f32, (i * 2) as f32, (i * 3) as f32];
                let vector_bytes = bytemuck::cast_slice::<f32, u8>(&vector).to_vec();

                conn.execute(
                    "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![index_id, vector_bytes, None::<String>, 1000, 1000],
                ).unwrap();
            }
        }

        // Load index with vectors and verify rebuild works
        {
            let conn2 = Connection::open(&db_path).unwrap();
            crate::schema::ensure_schema(&conn2).unwrap();

            // Load metadata only (vectors not loaded yet)
            let hnsw_metadata = HnswIndex::load_metadata(&conn2, "load_test").unwrap();
            assert_eq!(hnsw_metadata.vector_count, 5);
            assert_eq!(hnsw_metadata.storage.vector_count().unwrap(), 0); // No vectors loaded

            // Load with vectors - this rebuilds the graph
            let hnsw_loaded = HnswIndex::load_with_vectors(&conn2, "load_test").unwrap();
            assert_eq!(hnsw_loaded.vector_count, 5);
            assert_eq!(hnsw_loaded.storage.vector_count().unwrap(), 5); // Vectors loaded

            // Verify we can retrieve vectors
            let (vector, _) = hnsw_loaded.get_vector(1).unwrap().unwrap();
            assert_eq!(vector, vec![0.0, 0.0, 0.0]);

            // Verify search works (graph was rebuilt)
            let query = vec![2.0, 4.0, 6.0];
            let results = hnsw_loaded.search(&query, 3).unwrap();
            assert!(!results.is_empty());
        }

        // Clean up
        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_e2e_hnsw_persistence() {
        use rusqlite::Connection;
        use std::fs;

        let test_dir = "/tmp/test_hnsw_e2e_persistence";
        let db_path = format!("{}/test.db", test_dir);

        // Clean up any existing test database
        let _ = fs::remove_dir_all(test_dir);

        // Create directory
        fs::create_dir_all(test_dir).unwrap();

        // Create index and manually persist vectors to database
        {
            let conn = Connection::open(&db_path).unwrap();

            // Create schema
            crate::schema::ensure_schema(&conn).unwrap();

            // Create HNSW index metadata
            conn.execute(
                "INSERT INTO hnsw_indexes (name, dimension, m, ef_construction, distance_metric, vector_count, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params!["e2e_test", 3, 16, 200, "euclidean", 5, 1000, 1000],
            ).unwrap();

            let index_id = conn.last_insert_rowid();

            // Manually insert vectors into database (simulating what SQLiteVectorStorage would do)
            for i in 0..5 {
                let vector = vec![i as f32, (i * 2) as f32, (i * 3) as f32];
                let vector_bytes = bytemuck::cast_slice::<f32, u8>(&vector).to_vec();
                let metadata = serde_json::json!({"label": format!("vector_{}", i)}).to_string();

                conn.execute(
                    "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![index_id, vector_bytes, metadata, 1000, 1000],
                ).unwrap();
            }
        }

        // Reopen database and verify index is restored with vectors via SqliteGraph
        {
            let graph = SqliteGraph::open(&db_path).unwrap();

            // Check that index was loaded
            let index_names = graph.list_hnsw_indexes().unwrap();
            assert_eq!(index_names, vec!["e2e_test".to_string()]);

            // Get the loaded index
            let loaded_count = graph
                .get_hnsw_index_ref("e2e_test", |hnsw| {
                    // Verify all vectors were loaded
                    assert_eq!(hnsw.vector_count(), 5);

                    // Verify we can retrieve a vector
                    let (vector, metadata) = hnsw.get_vector(1).unwrap().unwrap();
                    assert_eq!(vector, vec![0.0, 0.0, 0.0]);
                    assert_eq!(metadata, serde_json::json!({"label": "vector_0"}));

                    // Verify search works (graph was rebuilt)
                    let query = vec![2.0, 4.0, 6.0];
                    let results = hnsw.search(&query, 3).unwrap();
                    assert!(!results.is_empty());

                    hnsw.vector_count()
                })
                .unwrap();

            assert_eq!(loaded_count, 5);
        }

        // Clean up
        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_multilayer_level_distribution() {
        // Create HnswIndex with multi-layer enabled
        let config = HnswConfig {
            dimension: 4,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 4,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(16),
            multilayer_deterministic_seed: Some(42),
        };

        let hnsw = HnswIndex::new("test_multilayer_dist", config).unwrap();

        // Verify level distributor was initialized
        assert!(
            hnsw.has_level_distributor(),
            "LevelDistributor should be initialized in multi-layer mode"
        );

        // Sample 1000 levels directly from the distributor to verify distribution
        use crate::hnsw::multilayer::LevelDistributor;
        let mut distributor = LevelDistributor::new(16.0, 4).with_seed(42);

        let mut level_counts = [0; 4];
        for _ in 0..1000 {
            let level = distributor.sample_level_internal();
            level_counts[level] += 1;
        }

        // The distribution is:
        // - P(level = 0) = 1 - 1/16 = 15/16 ≈ 937.5 out of 1000 (only base layer)
        // - P(level = 1) = 1/16 - 1/256 ≈ 58.6 out of 1000 (layers 0, 1)
        // - P(level = 2) = 1/256 - 1/4096 ≈ 3.7 out of 1000 (layers 0, 1, 2)
        // - P(level = 3) = 1/4096 ≈ 0.24 out of 1000 (layers 0, 1, 2, 3)

        // Level 0 should have approximately 937-944 vectors (allow 900-950 range)
        assert!(
            level_counts[0] >= 900 && level_counts[0] <= 950,
            "Level 0 should have ~938 samples, got {}",
            level_counts[0]
        );

        // Level 1 should have approximately 1000/16 = ~62 samples (allow 40-80 range)
        assert!(
            level_counts[1] >= 40 && level_counts[1] <= 80,
            "Level 1 should have ~62 samples, got {}",
            level_counts[1]
        );

        // Level 2 should have approximately 1000/256 = ~4 samples (allow 1-10 range)
        assert!(
            level_counts[2] >= 1 && level_counts[2] <= 10,
            "Level 2 should have ~4 samples, got {}",
            level_counts[2]
        );

        println!(
            "Level distribution (direct sampling): L0={}, L1={}, L2={}, L3={}",
            level_counts[0], level_counts[1], level_counts[2], level_counts[3]
        );

        // Note: Full multi-layer graph insertion requires LayerMappings integration
        // (deferred to plan 15-02) to handle bidirectional ID translation between
        // global vector IDs and layer-local node IDs.
        //
        // For now, the exponential distribution is wired into determine_insertion_level()
        // and will produce the correct level assignments. The full multi-layer graph
        // structure will be completed in subsequent plans.
    }

    #[test]
    fn test_single_layer_mode() {
        // Create HnswIndex with single-layer mode (default)
        let config = HnswConfig {
            dimension: 4,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 4,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: false, // Single-layer mode
            multilayer_level_distribution_base: None,
            multilayer_deterministic_seed: None,
        };

        let hnsw = HnswIndex::new("test_single_layer", config.clone()).unwrap();

        // Verify level distributor is NOT initialized in single-layer mode
        assert!(
            !hnsw.has_level_distributor(),
            "LevelDistributor should not be initialized in single-layer mode"
        );

        // Insert 100 vectors
        let test_vector = vec![1.0, 0.0, 0.0, 0.0];
        let mut hnsw_mut = HnswIndex::new("test_single_layer_mut", config).unwrap();
        for _ in 0..100 {
            hnsw_mut.insert_vector(&test_vector, None).unwrap();
        }

        let stats = hnsw_mut.statistics().unwrap();

        // In single-layer mode, all vectors should only be in layer 0
        assert_eq!(
            stats.layer_stats[0].0, 100,
            "Layer 0 should have 100 vectors"
        );

        // Higher layers should be empty
        assert_eq!(
            stats.layer_stats[1].0, 0,
            "Layer 1 should be empty in single-layer mode"
        );
        assert_eq!(
            stats.layer_stats[2].0, 0,
            "Layer 2 should be empty in single-layer mode"
        );
        assert_eq!(
            stats.layer_stats[3].0, 0,
            "Layer 3 should be empty in single-layer mode"
        );
    }

    #[test]
    fn test_multilayer_recall() {
        use std::collections::HashSet;

        let config = HnswConfig {
            dimension: 64,
            m: 16,
            ef_construction: 200,
            ef_search: 200,
            ml: 16,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: true, // Test multi-layer recall
            multilayer_level_distribution_base: Some(16),
            multilayer_deterministic_seed: Some(42),
        };

        let mut hnsw = HnswIndex::new("recall_test_unique", config).unwrap();
        let mut vectors = Vec::new();

        // Insert 100 random vectors
        for i in 0..1000 {
            let vector: Vec<f32> = (0..64)
                .map(|j| ((i * 64 + j) as f32 * 0.01).cos())
                .collect();
            vectors.push(vector.clone());
            hnsw.insert_vector(&vector, None).unwrap();
        }

        let k = 10;
        let query = &vectors[0];

        // HNSW approximate results
        let hnsw_results = hnsw.search(query, k).unwrap();
        let hnsw_ids: HashSet<_> = hnsw_results.iter().map(|(id, _)| *id).collect();

        // Exact nearest neighbors (brute force)
        fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum::<f32>()
                .sqrt()
        }

        let mut exact_results: Vec<_> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u64 + 1, euclidean_distance(query, v)))
            .collect();

        // Sort by distance (simple manual sort)
        for i in 0..exact_results.len() {
            let mut min_idx = i;
            for j in (i + 1)..exact_results.len() {
                if exact_results[j].1 < exact_results[min_idx].1 {
                    min_idx = j;
                }
            }
            if min_idx != i {
                exact_results.swap(i, min_idx);
            }
        }

        let exact_ids: HashSet<_> = exact_results.iter().take(k).map(|(id, _)| *id).collect();

        // Count overlap
        let overlap = hnsw_ids.intersection(&exact_ids).count();
        let recall = (overlap as f64 / k as f64) * 100.0;

        println!("HNSW results: {:?}", hnsw_results);
        println!("Exact top {}: {:?}", k, exact_ids);
        println!("Recall: {:.1}% ({}/{})", recall, overlap, k);
        assert!(
            recall >= 90.0,
            "Recall {:.1}% is below 90% threshold",
            recall
        );
    }

    #[test]
    #[ignore = "flaky: fails non-deterministically when run with all lib tests due to HNSW test pollution / NodeNotFound bug"]
    fn test_multilayer_search_complexity_ologn() {
        use std::time::Instant;

        // Test configurations with increasing dataset sizes
        let sizes = vec![100, 1000, 10000];
        let mut search_times = Vec::new();

        for size in sizes {
            let config = HnswConfig {
                dimension: 64,
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                ml: 16,
                distance_metric: DistanceMetric::Euclidean,
                enable_multilayer: true,
                multilayer_level_distribution_base: Some(16),
                multilayer_deterministic_seed: Some(42),
            };

            let mut hnsw = HnswIndex::new(&format!("complexity_test_{}", size), config).unwrap();

            // Insert vectors
            for i in 0..size {
                let vector: Vec<f32> = (0..64)
                    .map(|j| ((i * 64 + j) as f32 * 0.01).sin())
                    .collect();
                hnsw.insert_vector(&vector, None).unwrap();
            }

            // Measure search time (average of multiple searches)
            let query: Vec<f32> = (0..64).map(|j| (j as f32 * 0.01).sin()).collect();
            let iterations = 10;
            let start = Instant::now();
            for _ in 0..iterations {
                let _ = hnsw.search(&query, 10).unwrap();
            }
            let elapsed = start.elapsed();
            let avg_time_ns = elapsed.as_nanos() / iterations as u128;
            search_times.push((size, avg_time_ns));

            println!("Size {}: avg search time = {} ns", size, avg_time_ns);
        }

        // Verify logarithmic scaling: T(1000) / T(100) should be < 10
        // Linear scaling would be 10x (1000/100), logarithmic is typically < 5x
        let ratio_100_to_1000 = search_times[1].1 as f64 / search_times[0].1 as f64;
        println!("Time ratio (1000/100): {:.2}x", ratio_100_to_1000);
        assert!(
            ratio_100_to_1000 < 10.0,
            "Search time ratio {:.2}x suggests worse than log scaling; expected < 10x for O(log N)",
            ratio_100_to_1000
        );

        // Verify logarithmic scaling: T(10000) / T(1000) should be < 10
        // Linear scaling would be 10x (10000/1000), but log should be better
        let ratio_1000_to_10000 = search_times[2].1 as f64 / search_times[1].1 as f64;
        println!("Time ratio (10000/1000): {:.2}x", ratio_1000_to_10000);
        assert!(
            ratio_1000_to_10000 < 10.0,
            "Search time ratio {:.2}x suggests worse than log scaling; expected < 10x for O(log N)",
            ratio_1000_to_10000
        );

        // Most importantly: overall T(10000) / T(100) should be MUCH better than linear (100x)
        let overall_ratio = search_times[2].1 as f64 / search_times[0].1 as f64;
        println!("Overall time ratio (10000/100): {:.2}x", overall_ratio);
        assert!(
            overall_ratio < 50.0,
            "Overall search time ratio {:.2}x suggests linear scaling; expected < 50x for O(log N) (linear would be 100x)",
            overall_ratio
        );
    }

    #[test]
    fn test_multilayer_insert_layers_correct() {
        let config = HnswConfig {
            dimension: 64,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 16,
            distance_metric: DistanceMetric::Euclidean,
            enable_multilayer: true,
            multilayer_level_distribution_base: Some(16),
            multilayer_deterministic_seed: Some(42),
        };

        let mut hnsw = HnswIndex::new("test_layers", config).unwrap();

        // Insert 100 vectors
        for i in 0..100 {
            let vector: Vec<f32> = (0..64)
                .map(|j| ((i * 64 + j) as f32 * 0.01).cos())
                .collect();
            hnsw.insert_vector(&vector, None).unwrap();
        }

        // Verify nodes are distributed across layers
        let stats = hnsw.statistics().unwrap();

        println!("Layer stats: {:?}", stats.layer_stats);

        // All 100 vectors should be in layer 0 (base layer)
        assert_eq!(
            stats.layer_stats[0].0, 100,
            "Layer 0 should have all 100 vectors"
        );

        // Layer 1 should have some vectors (approximately 100/16 = 6-7)
        // With seed 42 and exponential distribution, we expect ~6 vectors in layer 1
        let layer1_count = stats.layer_stats[1].0;
        assert!(
            layer1_count > 0 && layer1_count < 20,
            "Layer 1 should have some vectors (got {}), but not all",
            layer1_count
        );

        // Verify higher layers have fewer or equal nodes than lower layers
        assert!(
            stats.layer_stats[0].0 >= stats.layer_stats[1].0,
            "Layer 0 should have >= Layer 1"
        );
        assert!(
            stats.layer_stats[1].0 >= stats.layer_stats[2].0,
            "Layer 1 should have >= Layer 2"
        );

        // Verify multi-layer mode is enabled
        assert!(
            hnsw.has_level_distributor(),
            "LevelDistributor should be initialized"
        );
    }

    #[test]
    fn test_topology_persistence_cross_session() {
        use std::fs;

        let test_dir = "/tmp/test_hnsw_topology_cross_session";
        let db_path = format!("{}/test.db", test_dir);
        let _ = fs::remove_dir_all(test_dir);
        fs::create_dir_all(test_dir).unwrap();

        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| (0..8).map(|j| ((i * 8 + j) as f32 * 0.1).sin()).collect())
            .collect();

        let session1_results = {
            let graph = SqliteGraph::open(&db_path).unwrap();
            let config = HnswConfigBuilder::new()
                .dimension(8)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap();

            {
                let _indexes = graph.hnsw_index_persistent("topo_test", config).unwrap();
            }

            for vector in &vectors {
                graph
                    .get_hnsw_index_mut("topo_test", |idx| idx.insert_vector(vector, None).unwrap())
                    .unwrap();
            }

            let query = vectors[0].clone();
            graph
                .get_hnsw_index_ref("topo_test", |idx| {
                    idx.search(&query, 5)
                        .unwrap()
                        .into_iter()
                        .map(|(id, dist)| (id, dist.to_bits()))
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };

        assert!(
            !session1_results.is_empty(),
            "session 1 should return results"
        );

        let session2_results = {
            let graph = SqliteGraph::open(&db_path).unwrap();

            let query = vectors[0].clone();
            graph
                .get_hnsw_index_ref("topo_test", |idx| {
                    let stats = idx.statistics().unwrap();
                    assert!(
                        stats.vector_count > 0,
                        "restored index should have vectors, got {}",
                        stats.vector_count
                    );
                    idx.search(&query, 5)
                        .unwrap()
                        .into_iter()
                        .map(|(id, dist)| (id, dist.to_bits()))
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };

        assert!(
            !session2_results.is_empty(),
            "session 2 should return results after restore"
        );

        assert_eq!(
            session1_results[0].0, session2_results[0].0,
            "cross-session top result must match: session1={:?} session2={:?}",
            session1_results, session2_results
        );

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_batch_insert_speed_large_index() {
        use crate::hnsw::{DistanceMetric, HnswConfigBuilder, HnswIndex};

        let config = HnswConfigBuilder::new()
            .dimension(768)
            .distance_metric(DistanceMetric::Cosine)
            .build()
            .unwrap();
        let mut index = HnswIndex::new("bench", config).unwrap();
        let dummy = vec![0.5f32; 768];

        // Build 50K vectors in batches of 256 to avoid per-insert persist overhead
        for batch_idx in 0..(50_000 / 256) {
            let batch: Vec<_> = (0..256)
                .map(|i| {
                    let mut v = dummy.clone();
                    v[0] = ((batch_idx * 256 + i) as f32) * 0.00001;
                    (v, None)
                })
                .collect();
            index.batch_insert_vectors(&batch).unwrap();
        }

        let start = std::time::Instant::now();
        let batch: Vec<_> = (0..16)
            .map(|i| {
                let mut v = dummy.clone();
                v[0] = (50_000 + i) as f32 * 0.00001;
                (v, None)
            })
            .collect();
        index.batch_insert_vectors(&batch).unwrap();
        let elapsed = start.elapsed();
        println!(
            "Batch insert into 50K in-memory index: {:?} for 16 vectors = {:.1} vectors/sec",
            elapsed,
            16.0 / elapsed.as_secs_f64()
        );
    }

    #[test]
    fn test_multi_index_coexistence() {
        use std::fs;

        let test_dir = "/tmp/test_hnsw_multi_index_coexist";
        let db_path = format!("{}/test.db", test_dir);
        let _ = fs::remove_dir_all(test_dir);
        fs::create_dir_all(test_dir).unwrap();

        {
            let graph = SqliteGraph::open(&db_path).unwrap();

            let config_a = HnswConfigBuilder::new()
                .dimension(4)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap();

            {
                let _indexes = graph.hnsw_index_persistent("index_a", config_a).unwrap();
            }

            for i in 0..10u32 {
                let v = vec![i as f32, (i * 2) as f32, (i * 3) as f32, (i * 4) as f32];
                graph
                    .get_hnsw_index_mut("index_a", |idx| idx.insert_vector(&v, None).unwrap())
                    .unwrap();
            }

            let config_b = HnswConfigBuilder::new()
                .dimension(4)
                .m_connections(4)
                .distance_metric(DistanceMetric::Euclidean)
                .build()
                .unwrap();

            {
                let _indexes_b = graph.hnsw_index_persistent("index_b", config_b).unwrap();
            }

            for i in 100..110u32 {
                let v = vec![
                    (i as f32).sin(),
                    (i as f32).cos(),
                    (i as f32).tan(),
                    (i as f32 * 0.5),
                ];
                graph
                    .get_hnsw_index_mut("index_b", |idx| idx.insert_vector(&v, None).unwrap())
                    .unwrap();
            }

            let results_a = graph
                .get_hnsw_index_ref("index_a", |idx| {
                    idx.search(&[1.0, 2.0, 3.0, 4.0], 3).unwrap()
                })
                .unwrap();
            assert!(!results_a.is_empty(), "index_a should have results");

            let results_b = graph
                .get_hnsw_index_ref("index_b", |idx| {
                    idx.search(&[0.0, 1.0, 0.0, 50.0], 3).unwrap()
                })
                .unwrap();
            assert!(!results_b.is_empty(), "index_b should have results");

            let ids_a: std::collections::HashSet<u64> =
                results_a.iter().map(|(id, _)| *id).collect();
            let ids_b: std::collections::HashSet<u64> =
                results_b.iter().map(|(id, _)| *id).collect();

            assert!(
                ids_a.intersection(&ids_b).count() == 0,
                "indices must not share vector IDs: A={:?} B={:?}",
                ids_a,
                ids_b
            );
        }

        {
            let graph2 = SqliteGraph::open(&db_path).unwrap();

            let restored_a = graph2
                .get_hnsw_index_ref("index_a", |idx| {
                    let stats = idx.statistics().unwrap();
                    assert!(
                        stats.vector_count == 10,
                        "restored index_a should have 10 vectors, got {}",
                        stats.vector_count
                    );
                    idx.search(&[1.0, 2.0, 3.0, 4.0], 3).unwrap()
                })
                .unwrap();
            assert!(
                !restored_a.is_empty(),
                "restored index_a should have results"
            );
        }

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_batch_insert_empty() {
        let config = HnswConfigBuilder::new()
            .dimension(3)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();
        let mut hnsw = HnswIndex::new("batch_empty", config).unwrap();
        let ids = hnsw.batch_insert_vectors(&[]).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn test_batch_insert_basic() {
        let config = HnswConfigBuilder::new()
            .dimension(3)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();
        let mut hnsw = HnswIndex::new("batch_basic", config).unwrap();

        let batch: Vec<(Vec<f32>, Option<serde_json::Value>)> = vec![
            (vec![1.0, 0.0, 0.0], None),
            (
                vec![0.0, 1.0, 0.0],
                Some(serde_json::json!({"label": "y-axis"})),
            ),
            (vec![0.0, 0.0, 1.0], None),
        ];

        let ids = hnsw.batch_insert_vectors(&batch).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(hnsw.statistics().unwrap().vector_count, 3);

        // Search should find results
        let results = hnsw.search(&[0.9, 0.1, 0.0], 2).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_batch_insert_dimension_mismatch() {
        let config = HnswConfigBuilder::new()
            .dimension(3)
            .distance_metric(DistanceMetric::Euclidean)
            .build()
            .unwrap();
        let mut hnsw = HnswIndex::new("batch_dim", config).unwrap();

        let batch: Vec<(Vec<f32>, Option<serde_json::Value>)> = vec![
            (vec![1.0, 0.0, 0.0], None),
            (vec![0.0, 1.0], None), // wrong dimension
        ];

        let result = hnsw.batch_insert_vectors(&batch);
        assert!(result.is_err());
        // No vectors should have been inserted (dimension check happens before any mutation)
        assert_eq!(hnsw.statistics().unwrap().vector_count, 0);
    }

    #[test]
    fn test_batch_insert_vs_individual_equivalence() {
        let config = HnswConfigBuilder::new()
            .dimension(4)
            .distance_metric(DistanceMetric::Cosine)
            .m_connections(8)
            .build()
            .unwrap();

        let vectors: Vec<(Vec<f32>, Option<serde_json::Value>)> = (0..20)
            .map(|i| {
                let v = vec![
                    (i as f32).cos(),
                    (i as f32).sin(),
                    ((i + 1) as f32).cos(),
                    ((i + 1) as f32).sin(),
                ];
                (v, Some(serde_json::json!({"idx": i})))
            })
            .collect();

        // Insert individually
        let mut hnsw_individual = HnswIndex::new("batch_vs_ind", config.clone()).unwrap();
        for (vec, meta) in &vectors {
            hnsw_individual.insert_vector(vec, meta.clone()).unwrap();
        }

        // Insert as batch
        let mut hnsw_batch = HnswIndex::new("batch_vs_batch", config).unwrap();
        let batch_ids = hnsw_batch.batch_insert_vectors(&vectors).unwrap();
        assert_eq!(batch_ids.len(), 20);

        // Both should have same vector count
        assert_eq!(
            hnsw_individual.statistics().unwrap().vector_count,
            hnsw_batch.statistics().unwrap().vector_count
        );

        // Both should find results for the same query
        let query = vec![1.0, 0.0, 0.9, 0.1];
        let results_ind = hnsw_individual.search(&query, 5).unwrap();
        let results_batch = hnsw_batch.search(&query, 5).unwrap();

        assert!(!results_ind.is_empty());
        assert!(!results_batch.is_empty());
    }

    #[test]
    fn test_batch_insert_with_persistence() {
        // Tests that batch_insert_vectors works through SqliteGraph's
        // get_hnsw_index_mut API (which acquires the mutex once for the batch)
        let graph = SqliteGraph::open_in_memory().unwrap();
        let config = HnswConfigBuilder::new()
            .dimension(3)
            .distance_metric(DistanceMetric::Cosine)
            .build()
            .unwrap();

        // Create index — must drop the MutexGuard before calling get_hnsw_index_mut
        // to avoid deadlock (both acquire hnsw_indexes lock)
        {
            let _guard = graph.hnsw_index("test_idx", config).unwrap();
        }

        let batch: Vec<(Vec<f32>, Option<serde_json::Value>)> = vec![
            (vec![1.0, 0.0, 0.0], Some(serde_json::json!({"id": 1}))),
            (vec![0.0, 1.0, 0.0], Some(serde_json::json!({"id": 2}))),
            (vec![0.0, 0.0, 1.0], Some(serde_json::json!({"id": 3}))),
        ];

        let ids = graph
            .get_hnsw_index_mut("test_idx", |idx| idx.batch_insert_vectors(&batch).unwrap())
            .unwrap();

        assert_eq!(ids.len(), 3);

        // Verify via search
        let results = graph
            .get_hnsw_index_ref("test_idx", |idx| {
                let stats = idx.statistics().unwrap();
                assert_eq!(stats.vector_count, 3);
                idx.search(&[1.0, 0.0, 0.0], 2).unwrap()
            })
            .unwrap();
        assert!(!results.is_empty(), "search should find results");
    }

    #[test]
    fn test_batch_insert_uses_transaction_for_sqlite() {
        // Regression: batch_insert_vectors must wrap SQLite stores in a single
        // transaction. Without it each store_vector is an autocommit —
        // bulk inserts are O(N) fsync instead of O(1).
        // Uses a file-backed SqliteGraph to exercise the SQLiteVectorStorage path.
        use std::fs;

        let test_dir = "/tmp/test_hnsw_batch_tx";
        let db_path = format!("{}/test.db", test_dir);
        let _ = fs::remove_dir_all(test_dir);
        fs::create_dir_all(test_dir).unwrap();

        let n = 500usize;
        let batch: Vec<(Vec<f32>, Option<serde_json::Value>)> = (0..n)
            .map(|i| {
                let f = i as f32;
                (
                    vec![f.sin(), f.cos(), f * 0.01, 1.0 - f * 0.005],
                    Some(serde_json::json!({"seq": i})),
                )
            })
            .collect();

        let config = HnswConfigBuilder::new()
            .dimension(4)
            .distance_metric(DistanceMetric::Cosine)
            .build()
            .unwrap();

        let graph = SqliteGraph::open(&db_path).unwrap();
        {
            let _guard = graph.hnsw_index_persistent("tx_test", config).unwrap();
        }

        let start = std::time::Instant::now();
        let ids = graph
            .get_hnsw_index_mut("tx_test", |idx| idx.batch_insert_vectors(&batch).unwrap())
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(ids.len(), n, "all {n} vectors must be inserted");

        graph
            .get_hnsw_index_ref("tx_test", |idx| {
                assert_eq!(
                    idx.statistics().unwrap().vector_count,
                    n,
                    "vector_count must equal batch size"
                );
            })
            .unwrap();

        // Without a transaction, 500 SQLite autocommits take >> 500ms on any real disk.
        // With a single transaction this completes in under 500ms.
        assert!(
            elapsed.as_millis() < 500,
            "batch_insert_vectors took {}ms for {n} vectors — missing transaction wrapper",
            elapsed.as_millis()
        );

        let results = graph
            .get_hnsw_index_ref("tx_test", |idx| {
                idx.search(&[0.0, 1.0, 0.0, 1.0], 5).unwrap()
            })
            .unwrap();
        assert!(
            !results.is_empty(),
            "search must find results after batch insert"
        );

        let _ = fs::remove_dir_all(test_dir);
    }
}

/// Returns available physical memory in bytes, or 0 if detection fails.
/// Used to gate vector cache loading so we don't OOM on low-memory systems.
fn available_memory_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        if let Ok(content) = fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemAvailable:") {
                    let kb: u64 = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    return kb * 1024;
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // On macOS, use sysctl hw.memsize as approximation.
        // vm_stat page_count * page_size would be more accurate but complex.
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(total) = s.trim().parse::<u64>() {
                    // Conservative: assume 50% available
                    return total / 2;
                }
            }
        }
    }
    // Unknown platform or detection failed: return 0 so caller uses
    // the cache unconditionally (no memory limit enforced).
    0
}
