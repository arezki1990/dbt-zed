//! Test to verify cache is being populated during insert_edge

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Cache Warm-up Test");
    println!("═══════════════════════════════════════════════════════════════\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let graph = open_graph(&db_path, &GraphConfig::native())?;

    // Insert nodes
    let mut node_ids = Vec::new();
    for i in 0..100 {
        let id = graph.insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;
        node_ids.push(id);
    }

    // Insert edges from node 0 to nodes 1-20
    for j in 1..=20 {
        graph.insert_edge(EdgeSpec {
            from: node_ids[0],
            to: node_ids[j as usize],
            edge_type: "test".to_string(),
            data: serde_json::Value::Null,
        })?;
    }
    println!("Inserted 20 edges from node_0\n");

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    // First query
    let start = Instant::now();
    let result = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    let first_ns = start.elapsed().as_nanos();
    println!("First query: {} ns ({} neighbors)", first_ns, result.len());

    // Immediate second query
    let start = Instant::now();
    let result = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    let second_ns = start.elapsed().as_nanos();
    println!(
        "Second query: {} ns ({} neighbors)",
        second_ns,
        result.len()
    );

    // Third query
    let start = Instant::now();
    let result = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    let third_ns = start.elapsed().as_nanos();
    println!("Third query: {} ns ({} neighbors)", third_ns, result.len());

    println!("\n═══════════════════════════════════════════════════════════════");

    Ok(())
}
