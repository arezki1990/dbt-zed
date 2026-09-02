use sqlitegraph::backend::NodeSpec;
use sqlitegraph::{GraphConfig, SnapshotId, open_graph};
use tempfile::TempDir;

#[test]
fn test_write_buffer_coherence_regression() -> Result<(), Box<dyn std::error::Error>> {
    // Instrument write buffer behavior to prove/disprove coherence hypothesis
    unsafe {
        std::env::set_var("WRITEBUF_DEBUG", "1");
    }

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test_writebuf_coherence.db");

    println!("=== PHASE 2C.3 WRITE BUFFER COHERENCE TEST ===");

    // Create graph with native V2 backend (default build)
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    println!("INSERTING NODE...");
    let node_id1 = graph
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "test_fn".to_string(),
            file_path: Some("/src/test.rs".to_string()),
            data: serde_json::json!({"lines": 50, "complexity": "low"}),
        })
        .expect("Failed to insert node");

    println!("Inserted node: {}", node_id1);

    // Read node back immediately - this should trigger read_bytes path
    println!("READING NODE BACK IMMEDIATELY...");
    let read_node = graph
        .get_node(SnapshotId::current(), node_id1)
        .expect("Failed to read node back");

    // Verify data integrity
    assert_eq!(read_node.kind, "Function");
    assert_eq!(read_node.name, "test_fn");
    println!("SUCCESS: Node data intact immediately after insert");

    // Close and reopen
    drop(graph);
    println!("FILE CLOSED");

    // Reopen and read again
    println!("REOPENING FILE...");
    let graph_reopened =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen graph");

    println!("READING NODE AFTER REOPEN...");
    let reopened_node = graph_reopened
        .get_node(SnapshotId::current(), node_id1)
        .expect("Failed to read node after reopen");

    // Verify persistence
    assert_eq!(reopened_node.kind, "Function");
    assert_eq!(reopened_node.name, "test_fn");
    println!("SUCCESS: Node data intact after reopen");

    unsafe {
        std::env::remove_var("WRITEBUF_DEBUG");
    }
    Ok(())
}
