//! V3 Neighbor Query Performance Test (Corrected)
//!
//! This test properly separates:
//! 1. Point lookup cost (B+tree traversal: node_id → page_id)
//! 2. Adjacency fetch cost (cached neighbor list read)
//!
//! KEY INSIGHT:
//! - Cold path: B+tree lookup + cache miss + disk read
//! - Hot path:  HashMap lookup + Arc::clone (zero-copy)
//!
//! The benchmark report's "70× faster" is for HOT path (cached adjacency).
//! The "slow" quick test was measuring COLD path (full lookup chain).

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  V3 Neighbor Query Performance Test (Point Lookup + Adjacency)");
    println!("═══════════════════════════════════════════════════════════════════\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let graph = open_graph(&db_path, &GraphConfig::native())?;
    println!("✅ Graph created with native backend\n");

    // Insert 100 nodes
    let start = Instant::now();
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
    let insert_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("Inserted 100 nodes in {:.2}ms", insert_ms);

    // Insert edges from node 0 to nodes 1-20
    let start = Instant::now();
    for j in 1..=20 {
        graph.insert_edge(EdgeSpec {
            from: node_ids[0],
            to: node_ids[j as usize],
            edge_type: "test".to_string(),
            data: serde_json::Value::Null,
        })?;
    }
    let edge_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("Inserted 20 edges from node_0 in {:.2}ms\n", edge_ms);

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    println!("───────────────────────────────────────────────────────────────────");
    println!("  COLD CACHE (First Query - Full Path)");
    println!("───────────────────────────────────────────────────────────────────");

    // First query (cold cache) - this is: B+tree lookup + cache miss + populate
    let start = Instant::now();
    let result = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    let cold_ns = start.elapsed().as_nanos();
    println!("Cold query:  {} ns ({} neighbors)", cold_ns, result.len());
    println!("  → Includes: B+tree traversal + cache miss + neighbor fetch\n");

    println!("───────────────────────────────────────────────────────────────────");
    println!("  WARM CACHE (After 100 Warmup Queries)");
    println!("───────────────────────────────────────────────────────────────────");

    // Warm up queries - populate cache
    for _ in 0..100 {
        let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    }

    // Benchmark 10,000 HOT queries (cached adjacency)
    let start = Instant::now();
    for _ in 0..10000 {
        let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
    }
    let total_time = start.elapsed();
    let hot_ns = total_time.as_nanos() / 10000;

    println!("Hot queries: {} queries in {:?}", 10000, total_time);
    println!("  Average:   {} ns/query (cached adjacency)", hot_ns);
    println!("  → Includes: HashMap lookup + Arc::clone (zero-copy)\n");

    println!("═══════════════════════════════════════════════════════════════════");
    println!("  RESULTS SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  Cold path (B+tree + cache miss):  {} ns", cold_ns);
    println!("  Hot path  (cached adjacency):     {} ns", hot_ns);
    println!(
        "  Cache warmup benefit:             {}× faster",
        cold_ns as f64 / hot_ns as f64
    );
    println!();

    if hot_ns < 1000 {
        println!("  ✅ EXCELLENT: Hot path < 1 μs (cached adjacency is fast)");
    } else if hot_ns < 5000 {
        println!("  ✅ GOOD: Hot path < 5 μs");
    } else {
        println!("  ⚠️  OK: Hot path {} μs", hot_ns / 1000);
    }

    println!("\n  NOTE: Benchmark report's '70× faster' measures HOT path vs SQLite");
    println!("        (cached adjacency fetch, not cold B+tree lookup)");
    println!("═══════════════════════════════════════════════════════════════════");

    Ok(())
}
