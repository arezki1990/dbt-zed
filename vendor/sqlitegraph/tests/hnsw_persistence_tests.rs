//! Comprehensive HNSW Persistence Tests
//!
//! Test suite for validating HNSW index persistence across sessions.

use rusqlite::Connection;
use sqlitegraph::{
    SqliteGraph,
    hnsw::{DistanceMetric, HnswConfig, HnswIndex},
    schema::ensure_schema,
};
use tempfile::TempDir;

/// Test metadata persistence across database reconnection
#[test]
fn test_hnsw_metadata_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Session 1: Create index and save metadata
    {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(128, 16, 200, DistanceMetric::Cosine);
        let hnsw = HnswIndex::new("test_index", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();
    }

    // Session 2: Reopen and verify metadata loaded
    {
        let conn = Connection::open(&db_path).unwrap();
        let loaded = HnswIndex::load_metadata(&conn, "test_index").unwrap();

        assert_eq!(loaded.config().dimension, 128);
        assert_eq!(loaded.config().m, 16);
        assert_eq!(loaded.config().ef_construction, 200);
        assert_eq!(loaded.config().distance_metric, DistanceMetric::Cosine);
        assert_eq!(loaded.name(), "test_index");
    }
}

/// Test vector persistence across database reconnection
///
/// NOTE: This test manually persists vectors to the database to work around
/// the current limitation where HnswIndex uses InMemoryVectorStorage by default.
/// Full automatic vector persistence will be added in a future update.
#[test]
fn test_hnsw_vector_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let vectors = vec![
        vec![1.0_f32, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.0, 0.0, 1.0],
    ];

    // Session 1: Create index and manually persist vectors
    let index_id = {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        let hnsw = HnswIndex::new("test_index", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();

        // Get index ID
        conn.query_row(
            "SELECT id FROM hnsw_indexes WHERE name = ?",
            ["test_index"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };

    // Manually insert vectors into database (simulating SQLiteVectorStorage)
    {
        let conn = Connection::open(&db_path).unwrap();
        for vector in &vectors {
            let vector_bytes = bytemuck::cast_slice::<f32, u8>(vector).to_vec();
            conn.execute(
                "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![index_id, vector_bytes, None::<String>, 1000, 1000],
            )
            .unwrap();
        }
    }

    // Session 2: Reopen and verify vectors loaded
    {
        let conn = Connection::open(&db_path).unwrap();
        let hnsw = HnswIndex::load_with_vectors(&conn, "test_index").unwrap();

        assert_eq!(hnsw.vector_count(), 3);
    }
}

/// Test full lifecycle: create -> insert -> close -> reopen -> search
///
/// NOTE: This test manually persists vectors to work around current limitations.
#[test]
fn test_hnsw_create_insert_close_reopen_search() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create and insert vectors
    let index_id = {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        let hnsw = HnswIndex::new("lifecycle_test", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();

        conn.query_row(
            "SELECT id FROM hnsw_indexes WHERE name = ?",
            ["lifecycle_test"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };

    // Manually insert vectors
    {
        let conn = Connection::open(&db_path).unwrap();
        for i in 0..10 {
            let vector = vec![i as f32, (i * 2) as f32, (i * 3) as f32];
            let vector_bytes = bytemuck::cast_slice::<f32, u8>(&vector).to_vec();
            conn.execute(
                "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![index_id, vector_bytes, None::<String>, 1000, 1000],
            )
            .unwrap();
        }
    }

    // Reopen and search
    {
        let conn = Connection::open(&db_path).unwrap();
        let hnsw = HnswIndex::load_with_vectors(&conn, "lifecycle_test").unwrap();

        assert_eq!(hnsw.vector_count(), 10);

        // Search for a similar vector
        let query = vec![5.0, 10.0, 15.0];
        let results = hnsw.search(&query, 3).unwrap();

        assert!(!results.is_empty(), "Search should return results");
        let (_best_id, distance) = &results[0];
        assert!(
            *distance < 5.0,
            "Distance should be small for similar vector"
        );
    }
}

/// Test empty index persistence
#[test]
fn test_hnsw_empty_index_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create index without inserting vectors
    {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(5, 16, 200, DistanceMetric::Cosine);
        let hnsw = HnswIndex::new("empty_index", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();
        assert_eq!(hnsw.vector_count(), 0);
    }

    // Reopen and verify empty index loads
    {
        let conn = Connection::open(&db_path).unwrap();
        let hnsw = HnswIndex::load_with_vectors(&conn, "empty_index").unwrap();

        assert_eq!(hnsw.config().dimension, 5);
        assert_eq!(hnsw.vector_count(), 0);
    }
}

/// Test index deletion cascades to vectors
#[test]
fn test_hnsw_delete_index() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create index with vectors
    {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        let mut hnsw = HnswIndex::new("delete_test", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();

        hnsw.insert_vector(&[1.0, 0.0, 0.0], None).unwrap();
        hnsw.insert_vector(&[0.0, 1.0, 0.0], None).unwrap();
        hnsw.insert_vector(&[0.0, 0.0, 1.0], None).unwrap();
    }

    // Delete index
    {
        let conn = Connection::open(&db_path).unwrap();
        HnswIndex::delete_index(&conn, "delete_test").unwrap();

        // Verify index gone
        let result = HnswIndex::load_metadata(&conn, "delete_test");
        assert!(result.is_err(), "Index should not exist after deletion");
    }
}

/// Test configuration preservation
#[test]
fn test_hnsw_config_preservation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create index with specific config
    {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(256, 32, 400, DistanceMetric::Euclidean);
        let hnsw = HnswIndex::new("config_test", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();
    }

    // Reopen and verify config matches
    {
        let conn = Connection::open(&db_path).unwrap();
        let loaded = HnswIndex::load_metadata(&conn, "config_test").unwrap();

        assert_eq!(loaded.config().dimension, 256);
        assert_eq!(loaded.config().m, 32);
        assert_eq!(loaded.config().ef_construction, 400);
        assert_eq!(loaded.config().distance_metric, DistanceMetric::Euclidean);
    }
}

/// Test all distance metrics persist correctly
#[test]
fn test_hnsw_distance_metric_preservation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let metrics = [
        DistanceMetric::Euclidean,
        DistanceMetric::Cosine,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];

    for (i, metric) in metrics.iter().enumerate() {
        let index_name = format!("metric_test_{}", i);

        // Create index
        {
            let conn = Connection::open(&db_path).unwrap();
            ensure_schema(&conn).unwrap();

            let config = HnswConfig::new(10, 16, 200, *metric);
            let hnsw = HnswIndex::new(&index_name, config).unwrap();
            hnsw.save_metadata(&conn).unwrap();
        }

        // Verify metric preserved
        {
            let conn = Connection::open(&db_path).unwrap();
            let loaded = HnswIndex::load_metadata(&conn, &index_name).unwrap();
            assert_eq!(loaded.config().distance_metric, *metric);
        }
    }
}

/// Regression: delete_index must remove vectors from hnsw_vectors (Bug 4)
///
/// Before fix: delete_index only deleted from hnsw_indexes. FK CASCADE
/// didn't fire because pool connections lack PRAGMA foreign_keys=ON.
#[test]
fn test_delete_index_removes_vectors() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let index_id = {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        let hnsw = HnswIndex::new("cascade_test", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();

        conn.query_row(
            "SELECT id FROM hnsw_indexes WHERE name = ?",
            ["cascade_test"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };

    {
        let conn = Connection::open(&db_path).unwrap();
        for v in &[vec![1.0_f32, 0.0, 0.0], vec![0.0, 1.0, 0.0]] {
            let bytes = bytemuck::cast_slice::<f32, u8>(v).to_vec();
            conn.execute(
                "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![index_id, bytes, None::<String>, 1000, 1000],
            ).unwrap();
        }
    }

    {
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hnsw_vectors WHERE index_id = ?1",
                [index_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "vectors should exist before delete");
    }

    {
        let conn = Connection::open(&db_path).unwrap();
        HnswIndex::delete_index(&conn, "cascade_test").unwrap();
    }

    {
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hnsw_vectors WHERE index_id = ?1",
                [index_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "delete_index must remove all vectors (Bug 4)");
    }
}

/// Regression: SqliteGraph.delete_hnsw_index must remove vectors via pool (Bug 4 real path)
///
/// Before fix: pool connections lack PRAGMA foreign_keys=ON, so CASCADE never
/// fires and hnsw_vectors rows are orphaned. This test proves it by checking
/// raw row count after delete — the FK constraint is in DDL but won't cascade
/// without the pragma.
#[test]
fn test_graph_delete_hnsw_index_removes_vectors() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let index_id: i64;

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        {
            let _guard = graph
                .hnsw_index_persistent("graph_cascade", config)
                .unwrap();
        }

        graph
            .get_hnsw_index_mut("graph_cascade", |idx| {
                idx.insert_vector(&[1.0, 0.0, 0.0], None)
            })
            .unwrap()
            .unwrap();

        graph
            .get_hnsw_index_mut("graph_cascade", |idx| {
                idx.insert_vector(&[0.0, 1.0, 0.0], None)
            })
            .unwrap()
            .unwrap();

        let conn = Connection::open(&db_path).unwrap();
        index_id = conn
            .query_row(
                "SELECT id FROM hnsw_indexes WHERE name = ?",
                ["graph_cascade"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();

        let count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hnsw_vectors WHERE index_id = ?1",
                [index_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            count_before >= 2,
            "vectors must exist before delete, got {}",
            count_before
        );
    }

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        graph.delete_hnsw_index("graph_cascade").unwrap();
    }

    {
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hnsw_vectors WHERE index_id = ?1",
                [index_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "delete_hnsw_index must remove all vectors via pool (Bug 4), got {} orphaned rows",
            count
        );
    }
}

/// Regression: delete+recreate persistent index via SqliteGraph (Bug 1 + Bug 4)
///
/// Before fix: hnsw_index_persistent uses auto-increment rowids that become
/// stale after delete, causing InvalidNodeId when HNSW layers expect
/// sequential 0-based IDs.
#[test]
fn test_persistent_index_delete_and_recreate() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        {
            let _guard = graph
                .hnsw_index_persistent("recreate_test", config)
                .unwrap();
        }

        graph
            .get_hnsw_index_mut("recreate_test", |idx| {
                idx.insert_vector(&[1.0, 0.0, 0.0], None)
            })
            .unwrap()
            .unwrap();

        graph
            .get_hnsw_index_mut("recreate_test", |idx| {
                idx.insert_vector(&[0.0, 1.0, 0.0], None)
            })
            .unwrap()
            .unwrap();

        let results = graph
            .get_hnsw_index_ref("recreate_test", |idx| idx.search(&[1.0, 0.0, 0.0], 1))
            .unwrap()
            .unwrap();
        assert!(
            !results.is_empty(),
            "search should find results before delete"
        );
    }

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        graph.delete_hnsw_index("recreate_test").unwrap();
    }

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        {
            let _guard = graph
                .hnsw_index_persistent("recreate_test", config)
                .unwrap();
        }

        for i in 0..5 {
            let v = vec![i as f32, 0.0, 0.0];
            graph
                .get_hnsw_index_mut("recreate_test", |idx| idx.insert_vector(&v, None))
                .unwrap()
                .unwrap();
        }

        let results = graph
            .get_hnsw_index_ref("recreate_test", |idx| idx.search(&[4.0, 0.0, 0.0], 1))
            .unwrap()
            .unwrap();
        assert!(
            !results.is_empty(),
            "search must work after delete+recreate (Bug 1)"
        );
    }
}

/// Regression: persistent index survives process restart via SqliteGraph (Bug 3 partial)
#[test]
fn test_persistent_index_survives_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        {
            let _guard = graph.hnsw_index_persistent("survive_test", config).unwrap();
        }

        for i in 0..3 {
            let v = vec![i as f32, 0.0, 0.0];
            graph
                .get_hnsw_index_mut("survive_test", |idx| idx.insert_vector(&v, None))
                .unwrap()
                .unwrap();
        }
    }

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let names = graph.list_hnsw_indexes().unwrap();
        assert!(
            names.contains(&"survive_test".to_string()),
            "index must survive reopen (Bug 3)"
        );

        graph
            .get_hnsw_index_ref("survive_test", |idx| {
                assert_eq!(idx.vector_count(), 3, "vectors must survive reopen");
            })
            .unwrap();
    }
}
///
/// NOTE: This test manually persists vectors to work around current limitations.
#[test]
fn test_hnsw_graph_autoload() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create index
    let index_id = {
        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(10, 16, 200, DistanceMetric::Cosine);
        let hnsw = HnswIndex::new("autoload_test", config).unwrap();
        hnsw.save_metadata(&conn).unwrap();

        conn.query_row(
            "SELECT id FROM hnsw_indexes WHERE name = ?",
            ["autoload_test"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };

    // Manually insert vectors
    {
        let conn = Connection::open(&db_path).unwrap();
        for vector in &[vec![1.0; 10], vec![2.0; 10]] {
            let vector_bytes = bytemuck::cast_slice::<f32, u8>(vector).to_vec();
            conn.execute(
                "INSERT INTO hnsw_vectors (index_id, vector_data, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![index_id, vector_bytes, None::<String>, 1000, 1000],
            )
            .unwrap();
        }
    }

    // Reopen via SqliteGraph and verify auto-load
    {
        let graph = SqliteGraph::open(&db_path).unwrap();

        // Verify index was auto-loaded
        let index_names = graph.list_hnsw_indexes().unwrap();
        assert_eq!(index_names, vec!["autoload_test".to_string()]);

        // Verify vectors loaded
        graph
            .get_hnsw_index_ref("autoload_test", |hnsw| {
                assert_eq!(hnsw.vector_count(), 2);
            })
            .unwrap();
    }
}

/// Regression: HnswIndex::delete_vector removes a single vector by ID
#[test]
fn test_hnsw_delete_single_vector() {
    let temp_dir = TempDir::new().unwrap();
    let _db_path = temp_dir.path().join("test.db");

    let mut hnsw = {
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        HnswIndex::new("delete_vec_test", config).unwrap()
    };

    let _id_a = hnsw.insert_vector(&[1.0, 0.0, 0.0], None).unwrap();
    let id_b = hnsw.insert_vector(&[0.0, 1.0, 0.0], None).unwrap();
    let _id_c = hnsw.insert_vector(&[0.0, 0.0, 1.0], None).unwrap();

    assert_eq!(hnsw.vector_count(), 3);

    hnsw.delete_vector(id_b).unwrap();
    assert_eq!(
        hnsw.vector_count(),
        2,
        "vector_count must decrease after delete"
    );

    let results = hnsw.search(&[1.0, 0.0, 0.0], 2).unwrap();
    assert!(!results.is_empty(), "search must still return results");
    let returned_ids: Vec<u64> = results.iter().map(|(id, _)| *id).collect();
    assert!(
        !returned_ids.contains(&id_b),
        "deleted vector must not appear in search results"
    );
}

/// Regression: SqliteGraph::delete_hnsw_vector removes vector from persistent index
#[test]
fn test_graph_delete_hnsw_vector() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let mut vector_ids: Vec<u64> = Vec::new();

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        {
            let _guard = graph.hnsw_index_persistent("del_test", config).unwrap();
        }

        for i in 0..5 {
            let v = vec![i as f32, 0.0, 0.0];
            let id = graph
                .get_hnsw_index_mut("del_test", |idx| idx.insert_vector(&v, None))
                .unwrap()
                .unwrap();
            vector_ids.push(id);
        }

        graph
            .get_hnsw_index_ref("del_test", |idx| {
                assert_eq!(idx.vector_count(), 5);
            })
            .unwrap();
    }

    {
        let graph = SqliteGraph::open(&db_path).unwrap();
        graph.delete_hnsw_vector("del_test", vector_ids[2]).unwrap();

        graph
            .get_hnsw_index_ref("del_test", |idx| {
                assert_eq!(
                    idx.vector_count(),
                    4,
                    "count must be 4 after deleting 1 vector"
                );
            })
            .unwrap();

        let results = graph
            .get_hnsw_index_ref("del_test", |idx| idx.search(&[0.0, 0.0, 0.0], 5))
            .unwrap()
            .unwrap();
        let returned_ids: Vec<u64> = results.iter().map(|(id, _)| *id).collect();
        assert!(
            !returned_ids.contains(&vector_ids[2]),
            "deleted vector must not appear in results"
        );
    }
}
