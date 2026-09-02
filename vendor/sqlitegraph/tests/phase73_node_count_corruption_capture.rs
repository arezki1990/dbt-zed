//! Phase 73 — Header Snapshot Integrity Test
//!
//! Focused test to capture node_count corruption at exact checkpoints

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

#[test]
fn test_phase73_node_count_corruption_capture() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 73: Node Count Corruption Capture ===");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("phase73_node_count_debug.db");

    // Create graph
    let cfg = GraphConfig::native();
    let graph = open_graph(&db_path, &cfg)?;

    // === CHECKPOINT A: After node insertion, before edge tx ===
    println!("STEP 1: Insert 5 nodes");
    let mut node_ids = Vec::new();
    for i in 1..=5 {
        let node_id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(node_id);
    }
    println!("✅ Inserted {} nodes", node_ids.len());

    // Capture node_count immediately after node insertion
    println!("=== CHECKPOINT A: After node insertion ===");
    let checkpoint_a_count = get_header_node_count_from_disk(&db_path)?;
    println!("Header.node_count on disk: {}", checkpoint_a_count);

    // === CHECKPOINT B: After begin_transaction() ===
    println!("STEP 2: Begin edge transaction (should snapshot header state)");
    // Force an edge insertion to trigger begin_transaction
    let result = graph.insert_edge(EdgeSpec {
        from: node_ids[0],
        to: node_ids[1],
        edge_type: "test_edge".to_string(),
        data: serde_json::json!({"test": "phase73"}),
    });

    println!("Edge insertion result: {:?}", result);

    // Capture node_count immediately after begin_transaction
    println!("=== CHECKPOINT B: After begin_transaction() ===");
    let checkpoint_b_count = get_header_node_count_from_disk(&db_path)?;
    println!("Header.node_count on disk: {}", checkpoint_b_count);

    // === CHECKPOINT C: After rollback_transaction() ===
    // The edge insertion above should have failed and triggered rollback
    // Capture node_count after rollback
    println!("=== CHECKPOINT C: After rollback_transaction() ===");
    let checkpoint_c_count = get_header_node_count_from_disk(&db_path)?;
    println!("Header.node_count on disk: {}", checkpoint_c_count);

    // === REOPEN TEST ===
    println!("STEP 3: Close and reopen graph");
    drop(graph);

    // Capture header state before reopen
    println!("=== BEFORE REOPEN: Raw disk header analysis ===");
    let mut file = std::fs::File::open(&db_path)?;
    use std::io::{Read, Seek, SeekFrom};

    // Read flags to see if transaction state is corrupted
    file.seek(SeekFrom::Start(12))?;
    let mut flags_bytes = [0u8; 4];
    file.read_exact(&mut flags_bytes)?;
    let flags = u32::from_be_bytes(flags_bytes);
    println!("Header flags: 0x{:08x}", flags);
    println!(
        "FLAG_V2_ATOMIC_COMMIT: 0x{:08x}",
        sqlitegraph::backend::native::constants::FLAG_V2_ATOMIC_COMMIT
    );
    println!(
        "TX_STATE_MASK: 0x{:08x}",
        sqlitegraph::backend::native::constants::TX_STATE_MASK
    );
    println!(
        "TX_STATE_IN_PROGRESS: 0x{:08x}",
        sqlitegraph::backend::native::constants::TX_STATE_IN_PROGRESS
    );
    println!(
        "is_tx_in_progress: {}",
        (flags & sqlitegraph::backend::native::constants::TX_STATE_MASK)
            == sqlitegraph::backend::native::constants::TX_STATE_IN_PROGRESS
    );

    let reopened_graph = open_graph(&db_path, &cfg)?;

    println!("=== CHECKPOINT D: After reopen ===");
    let checkpoint_d_count = get_header_node_count_from_disk(&db_path)?;
    println!("Header.node_count on disk: {}", checkpoint_d_count);

    // Verify nodes are still readable
    let mut readable_nodes = 0;
    for &node_id in &node_ids {
        match reopened_graph.get_node(SnapshotId::current(), node_id) {
            Ok(_) => readable_nodes += 1,
            Err(_) => println!("Node {} not readable", node_id),
        }
    }
    println!("Readable nodes: {}/{}", readable_nodes, node_ids.len());

    // Analysis
    println!("\n=== PHASE 73 ANALYSIS ===");
    println!("A) After nodes:    {}", checkpoint_a_count);
    println!("B) After begin:    {}", checkpoint_b_count);
    println!("C) After rollback: {}", checkpoint_c_count);
    println!("D) After reopen:   {}", checkpoint_d_count);

    if checkpoint_d_count == 0 {
        println!("❌ PHASE 73 CORRUPTION CONFIRMED: node_count reset to 0 after reopen");
        return Err("Phase 73 corruption: node_count became 0 after reopen".into());
    } else if checkpoint_d_count != checkpoint_a_count {
        println!(
            "❌ PHASE 73 CORRUPTION: node_count changed from {} to {}",
            checkpoint_a_count, checkpoint_d_count
        );
        return Err(format!(
            "Phase 73 corruption: node_count changed from {} to {}",
            checkpoint_a_count, checkpoint_d_count
        )
        .into());
    } else {
        println!("✅ PHASE 73 SUCCESS: node_count preserved across transaction lifecycle");
    }

    Ok(())
}

/// Helper to read header.node_count directly from disk (bypassing in-memory cache)
fn get_header_node_count_from_disk(
    db_path: &std::path::Path,
) -> Result<u64, Box<dyn std::error::Error>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(db_path)?;

    // Read node_count from offset 16 (see constants::header_offset::NODE_COUNT)
    file.seek(SeekFrom::Start(16))?;
    let mut node_count_bytes = [0u8; 8];
    file.read_exact(&mut node_count_bytes)?;

    let node_count = u64::from_be_bytes(node_count_bytes);
    Ok(node_count)
}
