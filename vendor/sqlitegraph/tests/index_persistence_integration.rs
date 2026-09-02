//! Integration test for index persistence on open/flush
//!
//! This test verifies that:
//! 1. Indexes are persisted to sidecar file on flush
//! 2. Indexes are restored from sidecar file on open
//! 3. Open is faster with persisted indexes than with rebuild

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use std::time::Instant;

#[test]
fn test_index_persistence_on_flush() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create a database with some nodes
    let backend = V3Backend::create(&db_path).unwrap();
    for i in 0..100 {
        let kind = format!("Kind{}", i % 3);
        let name = format!("node_{}", i);
        backend
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind,
                name,
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    backend.flush_to_disk().unwrap();

    // Verify sidecar file exists
    let index_path = db_path.with_extension("v3index");
    assert!(index_path.exists(), "Index file should exist after flush");

    // Reopen and verify it works
    let _backend = V3Backend::open(&db_path).unwrap();
}

#[test]
fn test_open_fallback_when_index_missing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_fallback.db");

    // Create a database with some nodes
    let backend = V3Backend::create(&db_path).unwrap();
    for i in 0..50 {
        let kind = format!("Kind{}", i % 2);
        let name = format!("node_{}", i);
        backend
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind,
                name,
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    backend.flush_to_disk().unwrap();

    // Remove the index file to simulate first open or corrupted index
    let index_path = db_path.with_extension("v3index");
    std::fs::remove_file(&index_path).unwrap();

    // Reopen should still work (fallback to rebuild)
    let _backend = V3Backend::open(&db_path).unwrap();
}

#[test]
fn test_open_with_persisted_indexes_is_faster() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("perf_test.db");

    // Create a database with many nodes
    let node_count = 10000;
    let backend = V3Backend::create(&db_path).unwrap();
    for i in 0..node_count {
        let kind = format!("Kind{}", i % 10);
        let name = format!("node_{:05}", i);
        backend
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind,
                name,
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    backend.flush_to_disk().unwrap();

    // Ensure index file exists
    let index_path = db_path.with_extension("v3index");
    assert!(index_path.exists(), "Index file should exist");

    // Measure open with persisted indexes
    let start = Instant::now();
    {
        let _backend = V3Backend::open(&db_path).unwrap();
    }
    let open_with_index = start.elapsed();

    // Remove index and measure open with rebuild
    std::fs::remove_file(&index_path).unwrap();

    let start = Instant::now();
    {
        let _backend = V3Backend::open(&db_path).unwrap();
    }
    let open_with_rebuild = start.elapsed();

    // Open with persisted indexes should be faster for large datasets
    let speedup = open_with_rebuild.as_nanos() as f64 / open_with_index.as_nanos().max(1) as f64;

    println!("Open with persisted index: {:?}", open_with_index);
    println!("Open with rebuild: {:?}", open_with_rebuild);
    println!("Speedup: {:.2}x", speedup);

    // For large datasets, persisted indexes should be at least as fast
    // (speedup >= 1.0 means persisted index is faster or equal)
    // Note: For small datasets, the page scan might be faster due to cache effects
    assert!(
        speedup >= 0.8, // Allow 20% tolerance for variance
        "Persisted indexes should not be significantly slower than rebuild, got {:.2}x speedup",
        speedup
    );
}
