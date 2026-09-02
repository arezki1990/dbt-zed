//! Node Slot Transaction Persistence Tests
//!
//! Tests that verify nodes remain intact across transaction boundaries.
//! Transactions are triggered internally by edge insertion operations.
//!
//! Uses the public GraphBackend API (not raw file offsets) for V3 compatibility.

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

#[test]
fn test_node_slots_persist_across_edge_transactions() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("node_edge_test.graph");

    println!("=== Testing Node Persistence Across Edge Transactions ===");

    // Phase 1: Create nodes and verify they exist
    let mut node_ids = Vec::new();
    {
        let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

        // Create 300 nodes to cross the 256 boundary
        for i in 1..=300 {
            let node_id = graph
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"phase": 1, "id": i}),
                })
                .unwrap_or_else(|_| panic!("Failed to insert node {}", i));
            node_ids.push(node_id);

            // Every 50 nodes, verify via public API
            if i % 50 == 0 {
                let node = graph
                    .get_node(SnapshotId::current(), node_id)
                    .unwrap_or_else(|_| panic!("Node {} missing after insert", node_id));
                assert_eq!(node.id, node_id, "Node ID mismatch for node {}", node_id);
                assert_eq!(
                    node.name,
                    format!("node_{}", node_id),
                    "Node name mismatch for node {}",
                    node_id
                );
                println!(
                    "Node {} verified: kind={}, name={}",
                    node_id, node.kind, node.name
                );
            }
        }

        // Create many edges to trigger internal transaction boundaries
        println!("Creating edges to trigger internal transactions...");
        for i in 0..1000 {
            let from_idx = i % node_ids.len();
            let to_idx = (i + 1) % node_ids.len();

            graph
                .insert_edge(EdgeSpec {
                    from: node_ids[from_idx],
                    to: node_ids[to_idx],
                    edge_type: "test_edge".to_string(),
                    data: serde_json::json!({"edge_index": i}),
                })
                .unwrap_or_else(|_| panic!("Failed to insert edge {}", i));
        }

        println!("Created 300 nodes and 1000 edges, verifying node persistence");

        // Verify critical boundary nodes still exist and have correct data
        for &node_id in &[256i64, 257, 258] {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Critical node {} missing after edges", node_id));
            assert_eq!(node.id, node_id, "Critical node {} ID corrupted", node_id);
            assert_eq!(
                node.name,
                format!("node_{}", node_id),
                "Critical node {} name corrupted",
                node_id
            );
        }
        drop(graph);
    }

    // Phase 2: Reopen and verify all nodes still exist
    {
        let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen graph");

        // Verify critical boundary nodes
        for &node_id in &[256i64, 257, 258] {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Critical node {} missing after reopen", node_id));
            assert_eq!(
                node.id, node_id,
                "Critical node {} ID mismatch after reopen",
                node_id
            );
            assert_eq!(
                node.name,
                format!("node_{}", node_id),
                "Critical node {} name corrupted after reopen",
                node_id
            );
            println!(
                "Critical node {} verified after reopen: kind={}, name={}",
                node_id, node.kind, node.name
            );
        }

        // Verify sample nodes across the range
        for &node_id in [1, 50, 100, 150, 200, 250, 300].iter() {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Node {} missing after reopen", node_id));
            assert_eq!(node.id, node_id, "Node ID mismatch after reopen");
            assert_eq!(
                node.name,
                format!("node_{}", node_id),
                "Node name corrupted after reopen"
            );
        }

        println!("All critical and sample nodes verified after transaction/reopen");
    }
}

#[test]
fn test_file_size_never_shrinks_during_edge_operations() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("file_size_test.graph");

    println!("=== Testing File Size Never Shrinks During Edge Operations ===");

    let mut max_file_size = 0u64;
    let node_ids: Vec<i64>;

    // Phase 1: Create nodes and track file growth
    {
        let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

        // Create 200 nodes
        let mut temp_node_ids = Vec::new();
        for i in 1..=200 {
            let node_id = graph
                .insert_node(NodeSpec {
                    kind: "FileSizeTestNode".to_string(),
                    name: format!("file_size_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"node_index": i}),
                })
                .expect("Failed to insert file size test node");
            temp_node_ids.push(node_id);

            // Track maximum file size reached
            let current_size = std::fs::metadata(&db_path).unwrap().len();
            max_file_size = max_file_size.max(current_size);
        }
        node_ids = temp_node_ids;

        // Verify some critical nodes exist
        for &node_id in &[1, 50, 100, 150, 200] {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Node {} missing after insert", node_id));
            assert_eq!(node.id, node_id, "Node {} ID mismatch", node_id);
        }

        println!("Created 200 nodes, max file size: {} bytes", max_file_size);
        drop(graph);
    }

    // Phase 2: Create many edges (triggers internal transactions)
    {
        let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen graph");

        // Verify Phase 1 nodes are readable immediately after reopen
        match graph.get_node(SnapshotId::current(), 1) {
            Ok(node) => println!("Phase 2 reopen: node 1 OK, name={}", node.name),
            Err(e) => println!("Phase 2 reopen: node 1 MISSING: {}", e),
        }

        // Create thousands of edges to exercise transaction logic
        for i in 0..2000 {
            let from_idx = i % node_ids.len();
            let to_idx = (i + 1) % node_ids.len();

            graph
                .insert_edge(EdgeSpec {
                    from: node_ids[from_idx],
                    to: node_ids[to_idx],
                    edge_type: "file_size_test_edge".to_string(),
                    data: serde_json::json!({"edge_index": i}),
                })
                .unwrap_or_else(|_| panic!("Failed to insert edge {}", i));

            // Check file size periodically
            if i % 500 == 0 {
                let current_size = std::fs::metadata(&db_path).unwrap().len();
                assert!(
                    current_size >= max_file_size,
                    "File size shrunk during edge operations: max was {}, now {}",
                    max_file_size,
                    current_size
                );
                max_file_size = max_file_size.max(current_size);
                println!("After {} edges, file size: {} bytes", i, current_size);
            }
        }

        // Verify nodes still exist before closing Phase 2
        for &node_id in &[1, 50, 100] {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Node {} missing before Phase 2 close", node_id));
            assert_eq!(node.id, node_id);
        }

        // Note: intentionally NOT flushing here - edge store changes are in-memory
        // and the test verifies nodes (which were already persisted in Phase 1)
        // survive the reopen in Phase 3 regardless of edge state

        println!(
            "Created 2000 edges, file size never shrunk below max: {}",
            max_file_size
        );
        drop(graph);
    }

    // Phase 3: Reopen and verify all nodes still exist
    {
        let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen graph");

        // Verify critical nodes still exist
        for &node_id in &[1, 50, 100, 150, 200] {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Node {} missing after edge operations", node_id));
            assert_eq!(
                node.id, node_id,
                "Node {} ID corrupted after edge operations",
                node_id
            );
        }

        // Verify all nodes still exist
        for &node_id in &node_ids {
            let node = graph
                .get_node(SnapshotId::current(), node_id)
                .unwrap_or_else(|_| panic!("Node {} missing after edge operations", node_id));
            assert_eq!(node.id, node_id, "Node ID corrupted after edge operations");
        }

        let final_size = std::fs::metadata(&db_path).unwrap().len();
        assert!(
            final_size >= max_file_size,
            "Final file size {} is smaller than max size {}",
            final_size,
            max_file_size
        );

        println!(
            "All {} nodes verified after edge operations, final file size: {}",
            node_ids.len(),
            final_size
        );
    }
}
