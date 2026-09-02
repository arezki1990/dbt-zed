//! Reproduction test for 10K-node database creation bug
//!
//! Bug: When creating a 10K-node database without WAL and calling flush_to_disk(),
//! the database cannot be reopened because pages aren't properly synced.
//!
//! Run with: cargo test --features native-v3 test_10k_bug_reproduction -- --nocapture

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_10k_nodes_without_wal() {
    println!("\n=== Testing 10K node creation without WAL ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_10k_no_wal.graph");

    // Create database with 10K nodes
    {
        println!("Creating database with 10K nodes...");
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..10000 {
            if i % 1000 == 0 {
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
        println!("  All 10K nodes inserted.");
        println!("  Calling flush_to_disk()...");
        backend.flush_to_disk().unwrap();
        println!("  Flush completed.");
        println!("  Backend dropping...");
    }

    // Try to reopen
    println!("\nAttempting to reopen database...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS: Database reopened successfully!"),
        Err(e) => {
            println!("FAILED: Could not reopen database: {:?}", e);
            panic!("10K-node bug reproduced: {:?}", e);
        }
    }
}

#[test]
fn test_1k_nodes_without_wal() {
    println!("\n=== Testing 1K node creation without WAL (baseline) ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_1k_no_wal.graph");

    // Create database with 1K nodes
    {
        println!("Creating database with 1K nodes...");
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..1000 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        println!("  All 1K nodes inserted.");
        println!("  Calling flush_to_disk()...");
        backend.flush_to_disk().unwrap();
        println!("  Flush completed.");
        println!("  Backend dropping...");
    }

    // Try to reopen
    println!("\nAttempting to reopen database...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS: Database reopened successfully!"),
        Err(e) => {
            println!("FAILED: Could not reopen database: {:?}", e);
            panic!("1K-node bug: {:?}", e);
        }
    }
}

#[test]
fn test_10k_nodes_drop_only() {
    println!("\n=== Testing 10K nodes with Drop only (no explicit flush) ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_10k_drop.graph");

    // Create database with 10K nodes, rely on Drop
    {
        println!("Creating database with 10K nodes...");
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..10000 {
            if i % 1000 == 0 {
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
        println!("  All 10K nodes inserted.");
        println!("  Relying on Drop trait to flush...");
    }

    // Try to reopen
    println!("\nAttempting to reopen database...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS: Database reopened successfully!"),
        Err(e) => {
            println!("FAILED: Could not reopen database: {:?}", e);
            panic!("10K-node Drop bug: {:?}", e);
        }
    }
}
