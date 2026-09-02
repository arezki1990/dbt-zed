//! Quick LRU cache performance test
//! Run with: cargo run --example cache_perf_test --features native-v3

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::snapshot::SnapshotId;
use std::time::Instant;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("SQLiteGraph v2.1.0 - LRU Cache Performance Test");
    println!("==============================================\n");

    let temp = TempDir::new()?;
    let db_path = temp.path().join("cache_test.db");

    // Create backend
    let backend = sqlitegraph::backend::native::v3::V3Backend::create(&db_path)?;

    // Insert 10,000 nodes and collect actual IDs
    println!("Inserting 10,000 nodes...");
    let start = Instant::now();
    let mut node_ids = Vec::new();
    for i in 0..10000 {
        let id = backend.insert_node(sqlitegraph::backend::NodeSpec {
            kind: "Test".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;
        node_ids.push(id);
    }
    let insert_time = start.elapsed();
    println!("  Insert time: {:?}", insert_time);
    println!("  Average: {:?} per node", insert_time / 10000);
    println!("  First ID: {}, Last ID: {}", node_ids[0], node_ids[9999]);
    println!();

    // First 1000 lookups (cold cache)
    println!("First 1000 lookups (cold cache)...");
    let start = Instant::now();
    for node_id in node_ids.iter().take(1000) {
        backend.get_node(SnapshotId::current(), *node_id)?;
    }
    let cold_time = start.elapsed();
    println!("  Total time: {:?}", cold_time);
    println!("  Average: {:?} per lookup\n", cold_time / 1000);

    // Second 1000 lookups (warm cache)
    println!("Second 1000 lookups (warm cache)...");
    let start = Instant::now();
    for node_id in node_ids.iter().take(1000) {
        backend.get_node(SnapshotId::current(), *node_id)?;
    }
    let warm_time = start.elapsed();
    println!("  Total time: {:?}", warm_time);
    println!("  Average: {:?} per lookup\n", warm_time / 1000);

    // Calculate speedup
    let speedup = cold_time.as_nanos() as f64 / warm_time.as_nanos() as f64;
    println!("Results:");
    println!("  Cold cache: {:?}", cold_time / 1000);
    println!("  Warm cache: {:?}", warm_time / 1000);
    println!("  Speedup: {:.2}×", speedup);

    if speedup >= 2.0 {
        println!("  ✅ LRU cache working! (≥2× speedup achieved)");
    } else {
        println!("  ⚠️  Cache speedup below 2× threshold");
    }

    Ok(())
}
