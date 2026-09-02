//! Crash Test Child Process
//!
//! This is a child process that performs continuous edge insertion
//! and can be terminated abruptly to test crash recovery.
//!
//! Used by v2_crash_simulation_test.rs

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, open_graph};
use std::env;
use std::fs;
use std::process::exit;
use std::time::{Duration, Instant};

fn main() {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: {} <db_path> <node_count> <edges_per_report> [multi_edge_factor]",
            args[0]
        );
        exit(1);
    }

    let db_path = &args[1];
    let node_count: usize = args[2].parse().expect("Invalid node_count");
    let edges_per_report: usize = args[3].parse().expect("Invalid edges_per_report");
    let multi_edge_factor: usize = if args.len() > 4 {
        args[4].parse().expect("Invalid multi_edge_factor")
    } else {
        1
    };

    println!("Starting crash test child process:");
    println!("  DB path: {}", db_path);
    println!("  Node count: {}", node_count);
    println!("  Edges per report: {}", edges_per_report);
    println!("  Multi-edge factor: {}", multi_edge_factor);

    // Create/open the graph
    let graph = match open_graph(db_path, &GraphConfig::native()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to open graph: {}", e);
            exit(1);
        }
    };

    let start_time = Instant::now();

    // Create nodes if file doesn't exist or is empty
    let file_size = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let should_create_nodes = file_size < 4096; // Assume empty if < 4KB

    if should_create_nodes {
        println!("Creating {} nodes...", node_count);
        let mut node_ids = Vec::with_capacity(node_count);
        for i in 0..node_count {
            let node_id = graph
                .insert_node(NodeSpec {
                    kind: "CrashTestNode".to_string(),
                    name: format!("crash_node_{}", i),
                    file_path: None,
                    data: serde_json::json!({
                        "process_id": std::process::id(),
                        "node_index": i,
                        "created_at": "2024-01-01T00:00:00Z",
                    }),
                })
                .expect("Failed to insert node");
            node_ids.push(node_id);

            if i > 0 && i % 10000 == 0 {
                println!("  Created {}/{} nodes", i, node_count);
            }
        }
        println!("✅ Node creation completed");
    }

    // Continuous edge insertion loop
    let mut edge_count = 0;
    let mut rng_state = 0xCAFEBABEu64;
    let next_report = edges_per_report;

    println!("Starting edge insertion loop...");
    loop {
        // Generate deterministic edge
        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let from_idx = (rng_state as usize) % node_count;

        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let mut to_idx = (rng_state as usize) % node_count;

        // Avoid self-loops
        if to_idx == from_idx {
            to_idx = (to_idx + 1) % node_count;
        }

        // Insert edge(s) with multi-edge support
        for multi_idx in 0..multi_edge_factor {
            graph
                .insert_edge(EdgeSpec {
                    from: (from_idx + 1) as i64,
                    to: (to_idx + 1) as i64,
                    edge_type: if multi_edge_factor > 1 {
                        format!("crash_edge_multi_{}", multi_idx)
                    } else {
                        "crash_edge".to_string()
                    },
                    data: serde_json::json!({
                        "process_id": std::process::id(),
                        "edge_index": edge_count + multi_idx,
                        "multi_index": multi_idx,
                        "timestamp": "2024-01-01T00:00:00Z",
                        "elapsed_ms": start_time.elapsed().as_millis(),
                        "rng_state": rng_state,
                    }),
                })
                .expect("Failed to insert edge");
        }

        edge_count += multi_edge_factor;

        // Report progress
        if edge_count >= next_report {
            let elapsed = start_time.elapsed();
            let rate = edge_count as f64 / elapsed.as_secs_f64();
            println!(
                "PROGRESS: {} edges inserted ({:.1} edges/sec, {:.1}s elapsed)",
                edge_count,
                rate,
                elapsed.as_secs_f64()
            );
        }

        // Brief sleep to avoid overwhelming the system
        if edge_count % 1000 == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
