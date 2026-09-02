//! Minimal reproduction test for edge-heavy corruption bug
//!
//! Scenarios B and D fail on reopen with error:
//! "Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 25448 > 4096"
//!
//! This test reproduces the issue with minimal data:
//! - 100 nodes
//! - 500 edges (5:1 edge-to-node ratio)
//!
//! Run with: cargo test --features native-v3 edge_corruption_repro -- --nocapture

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use tempfile::TempDir;

#[test]
fn edge_corruption_repro() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("edge_corruption.db");

    println!("\n=== Edge Corruption Reproduction Test ===\n");

    // Phase 1: Create database with nodes and edges
    println!("Phase 1: Creating database...");
    let backend = V3Backend::create(&db_path).unwrap();

    // Insert 100 nodes
    println!("  Inserting 100 nodes...");
    for i in 1..=100 {
        backend
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }

    // Insert 500 edges (5 outgoing edges per node)
    println!("  Inserting 500 edges...");
    for src in 1..=100 {
        for j in 1..=5 {
            let dst = (src * 5 + j) % 100 + 1; // Ensure dst is in 1..=100
            backend
                .insert_edge(sqlitegraph::backend::EdgeSpec {
                    from: src,
                    to: dst,
                    edge_type: String::new(), // Empty edge type
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }

    println!("  Flushing...");
    backend.flush().unwrap();
    println!("  ✓ Inserted 100 nodes + 500 edges");

    drop(backend);

    // Phase 2: Reopen and verify
    println!("\nPhase 2: Reopening database...");
    let backend2 = match V3Backend::open(&db_path) {
        Ok(b) => {
            println!("  ✓ Database opened successfully");
            b
        }
        Err(e) => {
            println!("  ❌ FAILED to open database: {:?}", e);
            panic!("Failed to reopen database");
        }
    };

    // Phase 3: Try to read all nodes
    println!("\nPhase 3: Reading nodes...");
    let snapshot_id = sqlitegraph::snapshot::SnapshotId::current();

    let mut nodes_read = 0;
    for node_id in 1..=100 {
        match backend2.get_node(snapshot_id, node_id) {
            Ok(_) => {
                nodes_read += 1;
                if node_id % 20 == 0 {
                    println!("  ✓ Read node {}", node_id);
                }
            }
            Err(e) => {
                println!("\n  ❌ FAILED at node {}: {:?}", node_id, e);
                panic!("Failed to read node {} after reopen: {:?}", node_id, e);
            }
        }
    }

    println!("\n  ✓ Successfully read all {} nodes", nodes_read);

    println!("\n✅ Test passed!");
}
