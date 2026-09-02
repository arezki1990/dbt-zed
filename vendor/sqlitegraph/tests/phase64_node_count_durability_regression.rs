//! Phase 64 — Node Count Durability Regression Test
//!
//! This test validates that node_count is properly persisted across file close/reopen
//! without requiring edge operations to trigger header writes.

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

/// Regression test for node count durability
#[test]
fn test_phase64_node_count_durability_regression() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 64 Node Count Durability Regression Test ===");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("phase64_node_count_test.db");

    // Step 1: Create database and insert nodes ONLY (no edges)
    println!("STEP 1: Creating database and inserting nodes...");
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config)?;

    // Insert 3 nodes to test node_count advancement
    let mut node_ids = Vec::new();
    for i in 1..=3 {
        let node_id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(node_id);
        println!("✅ Created node {} with ID {}", i, node_id);
    }

    // Step 2: Close database WITHOUT inserting edges
    println!("STEP 2: Closing database (no edges inserted)...");
    drop(graph);
    println!("✅ Database closed");

    // Step 3: Reopen database and verify node_count persisted
    println!("STEP 3: Reopening database...");
    let mut reopen_config = GraphConfig::native();
    reopen_config.native.create_if_missing = false; // Open existing file only

    let graph_reopened = open_graph(&db_path, &reopen_config)?;
    println!("✅ Database reopened successfully");

    // Step 4: Verify all nodes are accessible (proves node_count persistence)
    println!("STEP 4: Verifying node accessibility after reopen...");
    for (i, &expected_id) in node_ids.iter().enumerate() {
        // Try to access each node - this would fail with InvalidNodeId if node_count = 0
        let neighbors =
            graph_reopened.neighbors(SnapshotId::current(), expected_id, Default::default())?;
        println!(
            "✅ Node {} (ID {}) accessible - has {} neighbors",
            i + 1,
            expected_id,
            neighbors.len()
        );
    }

    // Step 5: Verify node count header field
    // Use internal API to check header directly (if accessible) or infer from node access
    println!(
        "✅ All {} nodes accessible after reopen - node_count persistence confirmed",
        node_ids.len()
    );

    println!("=== PHASE 64 NODE COUNT DURABILITY TEST PASSED ===");
    Ok(())
}

/// Extended test with mixed node+edge operations
#[test]
fn test_phase64_node_count_durability_with_edges() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 64 Node Count Durability Test (with edges) ===");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("phase64_node_count_edges_test.db");

    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config)?;

    // Create nodes
    let node1 = graph.insert_node(NodeSpec {
        kind: "SourceNode".to_string(),
        name: "source".to_string(),
        file_path: None,
        data: serde_json::json!({"type": "source"}),
    })?;

    let node2 = graph.insert_node(NodeSpec {
        kind: "TargetNode".to_string(),
        name: "target".to_string(),
        file_path: None,
        data: serde_json::json!({"type": "target"}),
    })?;

    println!("✅ Created nodes: {} and {}", node1, node2);

    // Close before edge insertion
    drop(graph);
    println!("✅ Closed database before edge insertion");

    // Reopen and insert edges
    let mut reopen_config = GraphConfig::native();
    reopen_config.native.create_if_missing = false;
    let graph_reopened = open_graph(&db_path, &reopen_config)?;
    println!("✅ Reopened database");

    // Verify nodes are accessible
    let neighbors1 = graph_reopened.neighbors(SnapshotId::current(), node1, Default::default())?;
    let neighbors2 = graph_reopened.neighbors(SnapshotId::current(), node2, Default::default())?;
    println!(
        "✅ Nodes accessible after reopen - node1: {} neighbors, node2: {} neighbors",
        neighbors1.len(),
        neighbors2.len()
    );

    // Insert edge
    let edge_id = graph_reopened.insert_edge(EdgeSpec {
        from: node1,
        to: node2,
        edge_type: "test_edge".to_string(),
        data: serde_json::json!({"test": true}),
    })?;
    println!("✅ Inserted edge with ID {}", edge_id);

    println!("=== PHASE 64 NODE COUNT DURABILITY WITH EDGES TEST PASSED ===");
    Ok(())
}
