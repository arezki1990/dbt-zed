//! Test native backend edge insertion to reproduce the benchmark panic

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, open_graph};
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing native backend edge insertion...");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let graph = open_graph(&db_path, &GraphConfig::native())?;
    println!("✅ Graph created with native backend");

    let node1 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node1".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 1}),
    })?;
    println!("✅ Node 1 created: {}", node1);

    let node2 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node2".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 2}),
    })?;
    println!("✅ Node 2 created: {}", node2);

    match graph.insert_edge(EdgeSpec {
        from: node1,
        to: node2,
        edge_type: "test".to_string(),
        data: serde_json::json!({"order": 1}),
    }) {
        Ok(edge_id) => println!("✅ Edge created: {}", edge_id),
        Err(e) => {
            println!("❌ Edge insertion failed: {:?}", e);
            return Err(e.into());
        }
    }

    println!("SUCCESS: Native backend edge insertion works!");
    Ok(())
}
