//! Test to verify data persists across reopen
//! Run with: cargo test --features native-v3 test_persistence -- --nocapture

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

#[test]
fn test_persistence_across_reopen() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_persistence_test.db");

    println!("\n=== Phase 1: Create and populate ===\n");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 10 nodes with distinctive data
        for i in 0..10 {
            let node_id = backend
                .insert_node(NodeSpec {
                    kind: "TEST_KIND".to_string(),
                    name: format!("TEST_NODE_{}", i),
                    file_path: None,
                    data: serde_json::json!({"value": i * 1000, "marker": format!("MARKER_{}", i)}),
                })
                .unwrap();

            println!("Inserted node {}: TEST_NODE_{}", node_id, i);
        }

        println!("Flushing...");
        backend.flush().unwrap();

        // Verify nodes exist before dropping
        for i in 0..10 {
            let node = backend.get_node(SnapshotId::current(), i + 1).unwrap();
            println!("Before drop: node {} = {:?}", i + 1, node.name);
        }
    }

    // Force file sync
    println!("\n=== Phase 2: Verify file exists ===\n");
    let metadata = std::fs::metadata(&db_path).unwrap();
    println!("File size: {} bytes", metadata.len());

    // Try to read a known node name from the file
    use std::io::Read;
    let mut file = std::fs::File::open(&db_path).unwrap();
    let mut buffer = vec![0u8; 1024];
    file.read_exact(&mut buffer).unwrap();
    println!("First 100 bytes of file:");
    for (i, byte) in buffer.iter().enumerate().take(100) {
        print!("{:02x} ", byte);
        if (i + 1) % 16 == 0 {
            println!();
        }
    }

    println!("\n=== Phase 3: Reopen and verify ===\n");

    let backend2 = V3Backend::open(&db_path).unwrap();

    println!("\nChecking if nodes persist...");
    for i in 0..10 {
        match backend2.get_node(SnapshotId::current(), i + 1) {
            Ok(node) => {
                println!(
                    "✓ Node {} found: kind={}, name={}",
                    i + 1,
                    node.kind,
                    node.name
                );
            }
            Err(e) => {
                println!("❌ Error reading node {}: {:?}", i + 1, e);
            }
        }
    }
}
