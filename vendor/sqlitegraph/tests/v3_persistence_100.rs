#![allow(clippy::needless_range_loop, reason = "test/bench code")]
//! Test with 100 nodes to verify persistence and correctness
//! Run with: cargo test --features native-v3 test_100_nodes -- --nocapture

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

#[test]
fn test_100_nodes_persistence() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_100_test.db");

    println!("\n=== Phase 1: Create and insert 100 nodes ===\n");

    let mut inserted_ids = Vec::new();

    // Create and insert 100 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();

        for i in 0..100 {
            let node_id = backend
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i, "data": "test data here".repeat(5)}),
                })
                .unwrap();
            inserted_ids.push(node_id);

            if (i + 1) % 20 == 0 {
                println!("Inserted {} nodes...", i + 1);
            }
        }

        println!("\nFlushing...");
        backend.flush().unwrap();

        // Verify a few nodes before dropping
        println!("\nVerifying sample nodes before drop:");
        for i in [0, 25, 50, 75, 99] {
            let node_id = inserted_ids[i];
            match backend.get_node(SnapshotId::current(), node_id) {
                Ok(node) => println!(
                    "  ✓ Node {} ({}): kind={}, name={}",
                    i + 1,
                    node.id,
                    node.kind,
                    node.name
                ),
                Err(e) => println!("  ❌ Node {} error: {:?}", i + 1, e),
            }
        }
    }

    println!("\n=== Phase 2: File stats ===\n");
    let metadata = std::fs::metadata(&db_path).unwrap();
    println!("File size: {} bytes", metadata.len());

    println!("\n=== Phase 3: Reopen and verify all 100 nodes ===\n");

    let backend2 = V3Backend::open(&db_path).unwrap();
    println!("Database reopened successfully");

    let mut found_count = 0;
    let mut error_count = 0;

    for i in 0..100 {
        let node_id = inserted_ids[i];
        match backend2.get_node(SnapshotId::current(), node_id) {
            Ok(node) => {
                // Verify the data is correct
                let expected_name = format!("node_{}", i);
                if node.name == expected_name && node.kind == "TestNode" {
                    found_count += 1;
                    if (i + 1) % 20 == 0 {
                        println!("  ✓ Node {}: verified", i + 1);
                    }
                } else {
                    println!(
                        "  ❌ Node {}: data mismatch! got name='{}', kind='{}'",
                        i + 1,
                        node.name,
                        node.kind
                    );
                    error_count += 1;
                }
            }
            Err(e) => {
                println!("  ❌ Node {} not found: {:?}", i + 1, e);
                error_count += 1;
            }
        }
    }

    println!("\n=== Results ===");
    println!("Found: {}/100", found_count);
    println!("Errors: {}", error_count);

    assert_eq!(found_count, 100, "All 100 nodes should be found");
    assert_eq!(error_count, 0, "No errors should occur");

    println!("\n✓ All 100 nodes persisted correctly!");
}
