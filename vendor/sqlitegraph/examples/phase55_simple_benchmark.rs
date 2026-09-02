//! Phase 55 — Simple V2 Performance Characterization
//!
//! Evidence-only measurement without debug output

use sqlitegraph::SnapshotId;
use sqlitegraph::{BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, open_graph};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 55 Simple V2 Performance Characterization ===");
    println!("Evidence-only measurement\n");

    // Small dataset first
    let node_count = 1_000;
    let edge_count = 4_000;
    let seed = 0xCAFE_BABE;

    // Create temporary directory
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("simple_v2_perf.db");

    println!("Database path: {}", db_path.display());

    // Open V2 NativeGraphBackend
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config)?;

    // Node insertion
    println!("Inserting {} nodes...", node_count);
    let start_time = Instant::now();
    let mut node_ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let node_id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(node_id);
    }

    let node_elapsed = start_time.elapsed().as_millis();
    println!(
        "Node insertion: {} ms ({:.1} nodes/sec)",
        node_elapsed,
        node_count as f64 * 1000.0 / node_elapsed as f64
    );

    // Edge insertion
    println!("Inserting {} edges...", edge_count);
    let edges_start_time = Instant::now();

    let mut rng_state: u32 = seed;
    for i in 0..edge_count {
        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let from_idx = rng_state as usize % node_count;

        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let mut to_idx = rng_state as usize % node_count;

        if to_idx == from_idx {
            to_idx = (to_idx + 1) % node_count;
        }

        let _edge_id = graph.insert_edge(EdgeSpec {
            from: node_ids[from_idx],
            to: node_ids[to_idx],
            edge_type: "test_edge".to_string(),
            data: serde_json::json!({"edge_index": i}),
        })?;
    }

    let edge_elapsed = edges_start_time.elapsed().as_millis();
    println!(
        "Edge insertion: {} ms ({:.1} edges/sec)",
        edge_elapsed,
        edge_count as f64 * 1000.0 / edge_elapsed as f64
    );

    // Neighbor queries
    println!("Running neighbor queries...");

    let low_start = Instant::now();
    let _low_neighbors = graph.neighbors(
        SnapshotId::current(),
        node_ids[0],
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    )?;
    let low_elapsed = low_start.elapsed().as_millis();
    println!("Low-degree neighbor query: {} ms", low_elapsed);

    let high_start = Instant::now();
    let _high_neighbors = graph.neighbors(
        SnapshotId::current(),
        node_ids[node_count / 2],
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    )?;
    let high_elapsed = high_start.elapsed().as_millis();
    println!("High-degree neighbor query: {} ms", high_elapsed);

    // File size
    let file_size = std::fs::metadata(&db_path)?.len();
    let bytes_per_node = file_size as f64 / node_count as f64;
    let bytes_per_edge = file_size as f64 / edge_count as f64;

    println!("\n=== RESULTS ===");
    println!(
        "Node insertion: {} ms ({:.1} nodes/sec)",
        node_elapsed,
        node_count as f64 * 1000.0 / node_elapsed as f64
    );
    println!(
        "Edge insertion: {} ms ({:.1} edges/sec)",
        edge_elapsed,
        edge_count as f64 * 1000.0 / edge_elapsed as f64
    );
    println!("Low-degree neighbor query: {} ms", low_elapsed);
    println!("High-degree neighbor query: {} ms", high_elapsed);
    println!("File size: {} bytes", file_size);
    println!("Bytes per node: {:.2}", bytes_per_node);
    println!("Bytes per edge: {:.2}", bytes_per_edge);
    println!("Total entities: {}", node_count + edge_count);

    Ok(())
}
