//! Detailed integrity check for V3 10K-node databases
//!
//! This test verifies:
//! 1. Database can be created with 10K nodes
//! 2. Database can be reopened
//! 3. File size is reasonable (not corrupted)
//! 4. Basic backend operations work after reopen

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_10k_integrity_reopen_basic() {
    println!("\n=== 10K-node integrity: basic reopen check ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("integrity_test.graph");

    // Create database
    {
        println!("Creating 10K-node database...");
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..10000 {
            if i % 2000 == 0 {
                println!("  Inserted {} nodes...", i);
            }
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        println!("10K nodes inserted, flushing...");
        backend.flush_to_disk().unwrap();
        println!("Flush complete.");

        // Check file size before dropping
        let metadata = fs::metadata(&db_path).unwrap();
        let file_size = metadata.len();
        println!(
            "File size after creation: {} bytes ({} MB)",
            file_size,
            file_size / 1_000_000
        );
        assert!(file_size > 100_000, "File should be at least 100KB");
    }

    // Reopen and verify
    println!("\nReopening database...");
    let backend = V3Backend::open(&db_path).unwrap();
    println!("Database reopened successfully!");

    // Try to insert another node to verify write capability works
    println!("Inserting a test node after reopen...");
    let new_id = backend
        .insert_node(NodeSpec {
            kind: "AfterReopen".to_string(),
            name: "test_after_reopen".to_string(),
            file_path: None,
            data: serde_json::json!({"test": true}),
        })
        .unwrap();
    println!("New node inserted with ID: {}", new_id);

    println!("\n✓ INTEGRITY CHECK PASSED");
    println!("  - Database created with 10K nodes");
    println!("  - Database reopened successfully");
    println!("  - Write operations work after reopen");
}
