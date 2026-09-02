use sqlitegraph::backend::NodeSpec;
use sqlitegraph::{GraphConfig, SnapshotId, open_graph};
use tempfile::TempDir;

#[test]
fn test_multi_node_write_corruption() -> Result<(), Box<dyn std::error::Error>> {
    // Test if multiple node writes cause corruption before edge insertion
    unsafe {
        std::env::set_var("V2_SLOT_DEBUG", "1");
    }

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test_multi_node.db");

    println!("=== TESTING MULTI-NODE WRITE CORRUPTION ===");

    // Create graph with native V2 backend
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    // Insert multiple nodes like the failing test
    println!("Inserting nodes...");
    let node_id1 = graph
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "main".to_string(),
            file_path: Some("/src/main.rs".to_string()),
            data: serde_json::json!({"lines": 100, "complexity": "medium"}),
        })
        .expect("Failed to insert node 1");

    let node_id2 = graph
        .insert_node(NodeSpec {
            kind: "Variable".to_string(),
            name: "config".to_string(),
            file_path: Some("/src/config.rs".to_string()),
            data: serde_json::json!({"type": "string", "mutable": false}),
        })
        .expect("Failed to insert node 2");

    let node_id3 = graph
        .insert_node(NodeSpec {
            kind: "Module".to_string(),
            name: "database".to_string(),
            file_path: Some("/src/database/mod.rs".to_string()),
            data: serde_json::json!({"dependencies": 5, "exports": ["connect", "query"]}),
        })
        .expect("Failed to insert node 3");

    println!("Inserted nodes: {}, {}, {}", node_id1, node_id2, node_id3);

    // CRITICAL: Read all nodes back IMMEDIATELY after all writes
    println!("Reading all nodes back immediately...");
    let read_node1 = graph
        .get_node(SnapshotId::current(), node_id1)
        .expect("Failed to read node 1");
    let read_node2 = graph
        .get_node(SnapshotId::current(), node_id2)
        .expect("Failed to read node 2");
    let read_node3 = graph
        .get_node(SnapshotId::current(), node_id3)
        .expect("Failed to read node 3");

    // Verify node data integrity
    assert_eq!(read_node1.kind, "Function");
    assert_eq!(read_node1.name, "main");
    assert_eq!(read_node2.kind, "Variable");
    assert_eq!(read_node2.name, "config");
    assert_eq!(read_node3.kind, "Module");
    assert_eq!(read_node3.name, "database");

    println!("SUCCESS: All nodes read correctly after multi-node write");

    // Close and reopen to test persistence
    drop(graph);
    println!("Graph closed");

    // Reopen and read again
    println!("Reopening graph...");
    let graph_reopened =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen V2 native graph");

    println!("Reading nodes after reopen...");
    let reopened_node1 = graph_reopened
        .get_node(SnapshotId::current(), node_id1)
        .expect("Failed to read node 1 after reopen");
    let reopened_node2 = graph_reopened
        .get_node(SnapshotId::current(), node_id2)
        .expect("Failed to read node 2 after reopen");
    let reopened_node3 = graph_reopened
        .get_node(SnapshotId::current(), node_id3)
        .expect("Failed to read node 3 after reopen");

    // Verify persistence
    assert_eq!(reopened_node1.kind, "Function");
    assert_eq!(reopened_node1.name, "main");
    assert_eq!(reopened_node2.kind, "Variable");
    assert_eq!(reopened_node2.name, "config");
    assert_eq!(reopened_node3.kind, "Module");
    assert_eq!(reopened_node3.name, "database");

    println!("SUCCESS: All nodes persisted correctly after reopen");

    unsafe {
        std::env::remove_var("V2_SLOT_DEBUG");
    }
    Ok(())
}
