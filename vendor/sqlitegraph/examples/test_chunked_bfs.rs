//! Test chunked BFS performance on various graph topologies

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::algorithm::parallel_bfs;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NodeSpec};
use std::time::Instant;

fn create_star_graph(backend: &V3Backend, size: i64) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create center node
    let center = backend
        .insert_node(NodeSpec {
            kind: "test".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!(null),
        })
        .unwrap();
    node_ids.push(center);

    // Create surrounding nodes
    for i in 1..size {
        let node = backend
            .insert_node(NodeSpec {
                kind: "test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();
        node_ids.push(node);

        // Connect to center
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: node,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();
    }

    node_ids
}

fn main() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let backend = V3Backend::create(&db_path).unwrap();

    println!("=== Chunked BFS Performance Test ===\n");

    // Test different graph sizes
    for size in [100, 500, 1000, 5000, 10000] {
        let node_ids = create_star_graph(&backend, size);
        let start = node_ids[0];

        // Warm up
        let _ = parallel_bfs(&backend, start, None);

        // Measure
        let start_time = Instant::now();
        let result = parallel_bfs(&backend, start, None).unwrap();
        let elapsed = start_time.elapsed();

        println!(
            "Size: {:>5} | Time: {:>8.2?} | Visited: {}",
            size, elapsed, result.total_visited
        );
    }

    println!("\n✓ Chunked BFS test complete");
}
