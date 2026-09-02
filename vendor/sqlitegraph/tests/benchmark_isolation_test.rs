//! Benchmark Isolation Test
//!
//! Tests whether benchmark iterations have proper isolation or if there's
//! state contamination between runs that could cause node slot corruption.

use sqlitegraph::{GraphConfig, NodeSpec, SnapshotId, open_graph};
use tempfile::TempDir;

#[test]
fn test_benchmark_iteration_isolation() {
    println!("=== BENCHMARK ITERATION ISOLATION TEST ===");

    // Simulate exactly what the benchmark does: multiple iterations with fresh temp dirs
    for iteration in 1..=3 {
        println!("\n--- ITERATION {} ---", iteration);

        // Create fresh temp directory (exactly like benchmark)
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("benchmark.db");

        println!("Created temp directory: {:?}", temp_dir.path());

        // Create V2 native graph (exactly like benchmark)
        let graph =
            open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

        // Create exactly 300 nodes (crossing the 256 boundary like the benchmark)
        let mut node_ids = Vec::new();
        for i in 1..=300 {
            let node_id = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap_or_else(|_| panic!("Failed to insert node {}", i));

            node_ids.push(node_id);

            // Log boundary nodes
            if (250..=260).contains(&i) {
                println!("Created node {} -> node_id {}", i, node_id);
            }
        }

        // Verify critical boundary nodes are accessible through the existing graph instance
        for &node_id in &[255, 256, 257, 258, 259, 260] {
            match graph.get_node(SnapshotId::current(), node_id) {
                Ok(node) => println!("Node {} accessible: {}", node_id, node.name),
                Err(e) => panic!("Failed to read node {}: {}", node_id, e),
            }
        }

        // Drop the graph instance (like what happens between benchmark iterations)
        drop(graph);

        // TempDir will be automatically cleaned up when it goes out of scope
        println!(
            "Iteration {} completed successfully - temp directory will be cleaned up",
            iteration
        );
    }

    println!("\n=== ISOLATION TEST PASSED ===");
    println!("All iterations maintained proper node slot integrity");
}

#[test]
fn test_direct_corruption_reproduction() {
    println!("=== DIRECT CORRUPTION REPRODUCTION TEST ===");

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let db_path = temp_dir.path().join("test.db");

    // Create graph and nodes
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    // Create 300 nodes
    let mut node_ids = Vec::new();
    for i in 1..=300 {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap_or_else(|_| panic!("Failed to insert node {}", i));
        node_ids.push(node_id);
    }

    // Verify node 257 is accessible through the existing graph instance
    match graph.get_node(SnapshotId::current(), 257) {
        Ok(node) => {
            println!("Node 257 accessible after node creation: {}", node.name);
            assert_eq!(node.name, "node_257");
        }
        Err(e) => panic!("Failed to read node 257: {}", e),
    }

    // Drop graph
    drop(graph);

    println!("=== DIRECT REPRODUCTION TEST PASSED ===");
}
