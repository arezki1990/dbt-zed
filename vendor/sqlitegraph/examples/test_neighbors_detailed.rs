//! Detailed breakdown of V3 neighbors hot path
//!
//! Measures each component separately to identify the bottleneck

use sqlitegraph::{BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId};
use std::sync::Arc;
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== DETAILED V3 NEIGHBORS HOT PATH ANALYSIS ===\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    // Create graph
    let graph = sqlitegraph::open_graph(&db_path, &GraphConfig::native())?;
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

    // Get the backend to test internal operations
    // Note: This requires accessing the backend which may not be public API
    // For now, we'll test through the public API but with different patterns

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    // Warm up cache
    for _ in 0..100 {
        let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    }

    println!("Cache warmed. Running detailed breakdown...\n");

    const ITERATIONS: usize = 10000;

    // Test 1: Full neighbors() call through public API
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let result = graph.neighbors(snapshot, node_ids[0], query.clone())?;
        // Use the result to prevent optimization
        std::hint::black_box(result);
    }
    let full_time = start.elapsed();
    let full_ns = full_time.as_nanos() as f64 / ITERATIONS as f64;
    println!("1. Full neighbors() call:        {:.2} ns/query", full_ns);

    // Test 2: Benchmark Vec copy from Arc<[i64]>
    let test_data: Arc<[i64]> = Arc::from((1..=20).collect::<Vec<_>>().into_boxed_slice());
    let start = Instant::now();
    let mut total_len = 0;
    for _ in 0..ITERATIONS {
        let vec = test_data.to_vec();
        total_len += vec.len(); // Use the result
    }
    let vec_copy_time = start.elapsed();
    let vec_ns = vec_copy_time.as_nanos() as f64 / ITERATIONS as f64;
    println!("2. Arc<[i64]> to Vec (20 el):  {:.2} ns/query", vec_ns);
    println!("   (verified with side effect: total_len={})", total_len);

    // Test 3: Snapshot validation (just the comparison)
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = snapshot.as_lsn() == 0;
        std::hint::black_box(false);
    }
    let snap_time = start.elapsed();
    let snap_ns = snap_time.as_nanos() as f64 / ITERATIONS as f64;
    println!("3. Snapshot validation:          {:.2} ns/query", snap_ns);

    // Calculate remaining overhead
    let remaining = full_ns - vec_ns - snap_ns;
    println!(
        "\n4. Remaining overhead:             {:.2} ns/query",
        remaining
    );

    println!("\n=== BREAKDOWN PERCENTAGES ===");
    println!("Vec copy:    {:.1}%", (vec_ns / full_ns) * 100.0);
    println!("Validation:  {:.1}%", (snap_ns / full_ns) * 100.0);
    println!("Overhead:    {:.1}%", (remaining / full_ns) * 100.0);

    println!("\n=== ANALYSIS ===");
    if remaining > 20000.0 {
        println!(
            "⚠️  BOTTLENECK: Remaining overhead is {:.2} µs/query",
            remaining / 1000.0
        );
        println!("   This includes: lock acquisition, HashMap lookup, Arc clone");
        println!("   Possible causes:");
        println!("   - RwLock read() overhead (even though uncontended)");
        println!("   - HashMap lookup overhead");
        println!("   - Multiple nested lock acquisitions (edge_store + cache)");
    }

    Ok(())
}
