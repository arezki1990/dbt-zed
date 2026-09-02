//! Graph Layer Node Existence Enforcement Test
//!
//! Tests that edges cannot be inserted to non-existent nodes.
//! This is a critical invariant to prevent graph corruption.

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, open_graph};

#[test]
fn test_graph_enforces_node_existence_before_edge_insert() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create graph
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    // Create exactly one node
    let existing_node_id = graph
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "existing_node".to_string(),
            file_path: None,
            data: serde_json::json!({"id": 1}),
        })
        .expect("Failed to insert existing node");

    // Create multiple nodes to have a valid ID range
    let node_ids: Vec<i64> = (2..=6)
        .map(|i| {
            graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap_or_else(|_| panic!("Failed to insert node {}", i))
        })
        .collect();

    // Now delete node 3 by setting its version to 0 (simulating corruption/gap)
    // We can't actually delete, so we'll use nodes that were never created
    // Use IDs 7, 8, 9 which are within reasonable range but don't exist

    // Test 1: Edge to non-existent source node should fail
    let result = graph.insert_edge(EdgeSpec {
        from: 7, // Non-existent source node (within range but not created)
        to: existing_node_id,
        edge_type: "test".to_string(),
        data: serde_json::json!({"test": "source_missing"}),
    });

    assert!(
        result.is_err(),
        "Edge insertion to non-existent source node should fail"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("not found")
            || error_msg.contains("7")
            || error_msg.contains("existing entities"),
        "Error should mention missing node, got: {}",
        error_msg
    );

    // Test 2: Edge to non-existent target node should fail
    let result = graph.insert_edge(EdgeSpec {
        from: existing_node_id,
        to: 8, // Non-existent target node
        edge_type: "test".to_string(),
        data: serde_json::json!({"test": "target_missing"}),
    });

    assert!(
        result.is_err(),
        "Edge insertion to non-existent target node should fail"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("not found")
            || error_msg.contains("8")
            || error_msg.contains("existing entities"),
        "Error should mention missing node, got: {}",
        error_msg
    );

    // Test 3: Edge between two non-existent nodes should fail
    let result = graph.insert_edge(EdgeSpec {
        from: 9, // Non-existent source node
        to: 10,  // Non-existent target node
        edge_type: "test".to_string(),
        data: serde_json::json!({"test": "both_missing"}),
    });

    assert!(
        result.is_err(),
        "Edge insertion between two non-existent nodes should fail"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("not found")
            || error_msg.contains("9")
            || error_msg.contains("existing entities"),
        "Error should mention missing source node, got: {}",
        error_msg
    );

    // Test 4: Edge between two existing nodes should succeed
    let result = graph.insert_edge(EdgeSpec {
        from: existing_node_id,
        to: node_ids[0], // First node from the created batch
        edge_type: "valid".to_string(),
        data: serde_json::json!({"test": "both_exist"}),
    });

    assert!(
        result.is_ok(),
        "Edge insertion between two existing nodes should succeed"
    );

    println!("✅ Graph layer correctly enforces node existence before edge insertion");
    println!("✅ All invalid edge insertions fail deterministically");
    println!("✅ Valid edge insertions succeed as expected");
}
