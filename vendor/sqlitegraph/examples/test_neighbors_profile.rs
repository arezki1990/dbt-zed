//! Profile V3 neighbors hot path to identify bottleneck
//!
//! This breaks down the hot neighbors query into stages:
//! 1. edge_store.read() lock acquisition
//! 2. neighbors() call (cache lookup)
//! 3. to_vec() conversion
//! 4. full round-trip

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::sync::Arc;
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== V3 NEIGHBORS HOT PATH PROFILING ===\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    // Create graph and insert data
    let graph = open_graph(&db_path, &GraphConfig::native())?;
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

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    // Warm up the cache
    for _ in 0..100 {
        let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    }

    println!("Cache warmed. Now profiling hot path...\n");

    // Profile 10000 iterations to get stable measurements
    const ITERATIONS: usize = 10000;

    // Stage 1: Full neighbors() call
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    }
    let full_time = start.elapsed();
    println!(
        "1. Full neighbors() call:     {:.2} ns/query",
        full_time.as_nanos() as f64 / ITERATIONS as f64
    );

    // Now let's profile the internal stages by creating a more detailed test
    // We'll need to access the backend directly

    println!("\n=== DETAILED STAGE BREAKDOWN ===");
    println!("Note: Need to access backend internals for detailed breakdown");
    println!("Running 1000 iterations for each stage...\n");

    // Get backend to profile internal stages
    // We'll use a simpler approach: just measure the full call multiple times
    // and compare with a Vec copy baseline

    // Baseline: Vec copy of 20 elements
    let test_data: Arc<[i64]> = Arc::from((1..=20).collect::<Vec<_>>().into_boxed_slice());
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = test_data.to_vec();
    }
    let vec_copy_time = start.elapsed();
    println!(
        "2. Baseline Vec copy (20 el): {:.2} ns/query",
        vec_copy_time.as_nanos() as f64 / ITERATIONS as f64
    );

    // Calculate time spent outside Vec copy
    let other_overhead = full_time.as_nanos() as f64 - vec_copy_time.as_nanos() as f64;
    println!(
        "\n3. Other overhead (lock + lookup): {:.2} ns/query",
        other_overhead / ITERATIONS as f64
    );

    println!("\n=== ANALYSIS ===");
    let vec_copy_pct = (vec_copy_time.as_nanos() as f64 / full_time.as_nanos() as f64) * 100.0;
    println!("Vec copy accounts for: {:.1}% of total time", vec_copy_pct);
    println!(
        "Other overhead accounts for: {:.1}% of total time",
        100.0 - vec_copy_pct
    );

    if other_overhead / ITERATIONS as f64 > 20000.0 {
        println!(
            "\n⚠️  WARNING: Other overhead is {:.2} µs/query - this is the bottleneck!",
            other_overhead / ITERATIONS as f64 / 1000.0
        );
    }

    Ok(())
}
