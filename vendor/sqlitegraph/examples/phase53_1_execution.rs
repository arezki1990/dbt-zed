//! Phase 53.1 — V2 Benchmark EXECUTION (Evidence-Only)
//!
//! Single case execution test for V2 NativeGraphBackend performance.
//! Measures insertion throughput and neighbor query performance.

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 53.1 V2 Execution Test ===");

    // Configuration
    let node_count = 10_000;
    let edge_count = 40_000;
    let seed = 0xCAFE_BABEu32; // Deterministic seed

    // Create temporary directory
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("v2_execution_test.db");

    println!("Database path: {}", db_path.display());

    // Open V2 NativeGraphBackend
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config)?;

    println!("✅ Graph backend opened successfully");

    // PHASE 1: Insert all nodes
    println!("\n=== Inserting {} nodes ===", node_count);
    let start_time = Instant::now();
    let mut rng_state = seed;

    let mut node_ids = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let node_id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({
                "index": i,
                "seed": seed,
                "created_at": "2025-01-15T00:00:00Z",
            }),
        })?;
        node_ids.push(node_id);

        // Progress reporting every 1000 nodes
        if (i + 1) % 1000 == 0 {
            let elapsed = start_time.elapsed().as_millis();
            let rate = (i + 1) * 1000 / elapsed.max(1) as usize;
            println!(
                "  Inserted {}/{} nodes ({:.1} nodes/sec)",
                i + 1,
                node_count,
                rate
            );
        }
    }

    let nodes_elapsed = start_time.elapsed();
    let nodes_rate = node_count as f64 / nodes_elapsed.as_secs_f64();
    println!(
        "✅ Node insertion completed: {} nodes in {:.2}s ({:.1} nodes/sec)",
        node_count,
        nodes_elapsed.as_secs_f64(),
        nodes_rate
    );

    // PHASE 2: Insert all edges (sparse directed, no multi-edge)
    println!("\n=== Inserting {} edges ===", edge_count);
    let edges_start_time = Instant::now();

    for i in 0..edge_count {
        // Use seeded RNG for deterministic edge generation
        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let from_idx = rng_state as usize % node_count;

        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let mut to_idx = rng_state as usize % node_count;

        // Avoid self-loops
        if to_idx == from_idx {
            to_idx = (to_idx + 1) % node_count;
        }

        let _edge_id = graph.insert_edge(EdgeSpec {
            from: node_ids[from_idx],
            to: node_ids[to_idx],
            edge_type: "test_edge".to_string(),
            data: serde_json::json!({
                "edge_index": i,
                "seed": seed,
                "from_idx": from_idx,
                "to_idx": to_idx,
            }),
        })?;

        // Progress reporting every 5000 edges
        if (i + 1) % 5000 == 0 {
            let elapsed = edges_start_time.elapsed().as_millis();
            let rate = (i + 1) * 1000 / elapsed.max(1) as usize;
            println!(
                "  Inserted {}/{} edges ({:.1} edges/sec)",
                i + 1,
                edge_count,
                rate
            );
        }
    }

    let edges_elapsed = edges_start_time.elapsed();
    let edges_rate = edge_count as f64 / edges_elapsed.as_secs_f64();
    println!(
        "✅ Edge insertion completed: {} edges in {:.2}s ({:.1} edges/sec)",
        edge_count,
        edges_elapsed.as_secs_f64(),
        edges_rate
    );

    // PHASE 3: Neighbor queries
    println!("\n=== Running neighbor queries ===");

    // Find low-degree node (first node)
    let low_degree_node = node_ids[0];
    let low_start = Instant::now();
    let low_neighbors = graph.neighbors(
        SnapshotId::current(),
        low_degree_node,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    )?;
    let low_elapsed = low_start.elapsed();

    println!(
        "Low-degree node {}: {} outgoing neighbors (query time: {:?}ms)",
        low_degree_node,
        low_neighbors.len(),
        low_elapsed.as_millis()
    );

    // Find high-degree node (use deterministic selection - middle node)
    let high_degree_node = node_ids[node_count / 2];
    let high_start = Instant::now();
    let high_neighbors = graph.neighbors(
        SnapshotId::current(),
        high_degree_node,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    )?;
    let high_elapsed = high_start.elapsed();

    println!(
        "High-degree node {}: {} outgoing neighbors (query time: {:?}ms)",
        high_degree_node,
        high_neighbors.len(),
        high_elapsed.as_millis()
    );

    // PHASE 4: File size measurement
    println!("\n=== File size analysis ===");
    let file_size = std::fs::metadata(&db_path)?.len();
    let bytes_per_node = file_size as f64 / node_count as f64;
    let bytes_per_edge = file_size as f64 / edge_count as f64;

    println!("File size: {} bytes", file_size);
    println!("Bytes per node: {:.2}", bytes_per_node);
    println!("Bytes per edge: {:.2}", bytes_per_edge);
    println!(
        "Total entities: {} (nodes + edges)",
        (node_count + edge_count) as u128
    );

    // Final summary
    let total_elapsed = start_time.elapsed();
    println!("\n=== Execution Summary ===");
    println!("Total time: {:.2}s", total_elapsed.as_secs_f64());
    println!("Node rate: {:.1} nodes/sec", nodes_rate);
    println!("Edge rate: {:.1} edges/sec", edges_rate);
    println!(
        "Low-degree query: {} neighbors in {:?}ms",
        low_neighbors.len(),
        low_elapsed.as_millis()
    );
    println!(
        "High-degree query: {} neighbors in {:?}ms",
        high_neighbors.len(),
        high_elapsed.as_millis()
    );
    println!(
        "File efficiency: {:.2} bytes/entity",
        bytes_per_node + bytes_per_edge
    );

    Ok(())
}
