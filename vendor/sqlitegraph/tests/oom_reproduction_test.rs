//! OOM Reproduction Test
//!
//! This test reproduces the OOM issue when running tests with multiple threads.

use std::thread;

use sqlitegraph::{GraphEntity, SqliteGraph};

/// Test that creates multiple temp databases to simulate memory pressure
#[test]
fn test_oom_memory_pressure() {
    let mut handles = vec![];

    // Spawn threads that each create a graph instance
    for i in 0..10 {
        let handle = thread::spawn(move || {
            // Each thread creates its own isolated database
            let temp_dir = tempfile::tempdir().unwrap();
            let db_path = temp_dir.path().join(format!("test_{}.db", i));

            // Create a graph - this is where memory gets allocated
            let graph = SqliteGraph::open(&db_path).expect("Failed to create graph");

            // Insert some data
            for j in 0..100 {
                let _ = graph.insert_entity(&GraphEntity {
                    id: 0,
                    kind: "Test".to_string(),
                    name: format!("node_{}_{}", i, j),
                    file_path: None,
                    data: serde_json::json!({"index": j}),
                });
            }

            // Graph is dropped here, temp_dir is cleaned up
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("Thread panicked");
    }
}

/// Test sequential database creation (should not OOM)
#[test]
fn test_sequential_no_oom() {
    for i in 0..10 {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join(format!("test_{}.db", i));

        let graph = SqliteGraph::open(&db_path).expect("Failed to create graph");

        for j in 0..100 {
            let _ = graph.insert_entity(&GraphEntity {
                id: 0,
                kind: "Test".to_string(),
                name: format!("node_{}_{}", i, j),
                file_path: None,
                data: serde_json::json!({"index": j}),
            });
        }

        // Graph and temp_dir dropped at end of iteration
    }
}
