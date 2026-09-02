//! Test to trigger index rebuilding and see check_buffer content

use sqlitegraph::{NodeSpec, config::GraphConfig, open_graph};
use tempfile::TempDir;

#[test]
fn test_index_rebuilding_debug() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test_rebuild.db");

    // Create graph and insert some nodes
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config).expect("Failed to create native graph");

    // Insert a few nodes
    for i in 1..=5 {
        let node_spec = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("test_node_{}", i),
            file_path: None,
            data: serde_json::json!({"node_id": i}),
        };
        graph.insert_node(node_spec).expect("Failed to insert node");
    }

    // Drop graph to force index rebuilding on next open
    drop(graph);

    // Reopen graph (this should trigger index rebuilding)
    let graph2 = open_graph(&db_path, &config).expect("Failed to reopen native graph");

    // Try to insert a node to trigger validation
    let node_spec = NodeSpec {
        kind: "test_node".to_string(),
        name: "test_node_after_rebuild".to_string(),
        file_path: None,
        data: serde_json::json!({"node_id": 999}),
    };

    let result = graph2.insert_node(node_spec);
    println!("Insert after rebuild result: {:?}", result);

    match result {
        Ok(_) => println!("SUCCESS: Node insertion worked after rebuild"),
        Err(e) => println!("ERROR: Node insertion failed after rebuild: {}", e),
    }
}
