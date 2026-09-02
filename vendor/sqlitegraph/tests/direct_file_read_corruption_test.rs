use sqlitegraph::backend::NodeSpec;
use sqlitegraph::{GraphConfig, SnapshotId, open_graph};
use std::fs::File;
use std::io::{Read, Seek};
use tempfile::TempDir;

#[test]
fn test_direct_file_read_corruption() -> Result<(), Box<dyn std::error::Error>> {
    // Test if corruption is in the actual file or just read cache
    unsafe {
        std::env::set_var("V2_SLOT_DEBUG", "1");
    }

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test_direct_read.db");

    println!("=== TESTING DIRECT FILE READ CORRUPTION ===");

    // Create graph with native V2 backend
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    // Insert 3 nodes like the failing test
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

    // CRITICAL: Close graph to flush all caches
    drop(graph);
    println!("Graph closed - all caches should be flushed");

    // REOPEN: Read direct from file to check if corruption is in the actual file
    println!("Reopening for edge insertion...");
    let graph_reopened =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen V2 native graph");

    // CRITICAL: Try to read node 1 through the API BEFORE edge insertion
    println!("Reading node 1 through API before edge insertion...");
    let api_read_node1 = graph_reopened
        .get_node(SnapshotId::current(), node_id1)
        .expect("Failed to read node 1 through API");

    println!(
        "API READ SUCCESS: node 1 kind={}, name={}",
        api_read_node1.kind, api_read_node1.name
    );

    // CRITICAL: Read directly from file to verify actual file contents
    println!("Reading directly from file at node 1 slot offset...");
    let mut file = File::open(&db_path)?;
    let mut node_slot_buffer = vec![0u8; 32]; // Read first 32 bytes of node slot

    // Node 1 slot offset = 0x400
    file.seek(std::io::SeekFrom::Start(0x400))?;
    file.read_exact(&mut node_slot_buffer)?;

    let version_byte = node_slot_buffer[0];
    println!(
        "DIRECT FILE READ: node 1 slot_offset=0x400, version={}, first_32_bytes={:02x?}",
        version_byte, &node_slot_buffer
    );

    // Now try edge insertion
    println!("Attempting edge insertion...");
    let edge_result = graph_reopened.insert_edge(sqlitegraph::backend::EdgeSpec {
        from: node_id1,
        to: node_id2,
        edge_type: "imports".to_string(),
        data: serde_json::json!({"reason": "config dependency"}),
    });

    match edge_result {
        Ok(edge_id) => println!("EDGE INSERTION SUCCESS: edge_id={}", edge_id),
        Err(e) => println!("EDGE INSERTION FAILED: {}", e),
    }

    unsafe {
        std::env::remove_var("V2_SLOT_DEBUG");
    }
    Ok(())
}
