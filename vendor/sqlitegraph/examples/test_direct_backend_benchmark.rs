//! Direct V3Backend benchmark - bypasses SqliteGraph wrapper
//!
//! Tests the call path at different levels to isolate where overhead comes from:
//! 1. Full neighbors() call through V3Backend (GraphBackend trait)
//! 2. Compare to baseline RwLock+HashMap performance

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec, SnapshotId,
    backend::native::v3::V3Backend,
};
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== DIRECT V3BACKEND BENCHMARK ===\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    // Create V3 backend directly
    let backend = V3Backend::create(&db_path)?;
    let mut node_ids = Vec::new();
    for i in 0..100 {
        let id = backend.insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;
        node_ids.push(id);
    }

    // Insert edges from node 0 to nodes 1-20
    for j in 1..=20 {
        backend.insert_edge(EdgeSpec {
            from: node_ids[0],
            to: node_ids[j as usize],
            edge_type: "test".to_string(),
            data: serde_json::Value::Null,
        })?;
    }

    // Drop backend to flush
    drop(backend);

    // Reopen to test cached behavior
    let backend = V3Backend::open(&db_path)?;

    println!(
        "Graph created with {} nodes and 20 edges from node_0\n",
        node_ids.len()
    );

    const ITERATIONS: usize = 10000;
    let src_node = node_ids[0];

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    // Warm up cache
    for _ in 0..100 {
        let _ = backend.neighbors(snapshot, src_node, query.clone())?;
    }

    println!("Cache warmed. Running benchmarks...\n");

    // Test 1: V3Backend::neighbors() through GraphBackend trait
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let neighbors = backend.neighbors(snapshot, src_node, query.clone())?;
        std::hint::black_box(neighbors);
    }
    let time1 = start.elapsed();
    let ns1 = time1.as_nanos() as f64 / ITERATIONS as f64;
    println!("1. V3Backend::neighbors():       {:.2} ns/query", ns1);

    // Test 2: Baseline comparison with simple RwLock+HashMap
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc as StdArc;

    let cache = RwLock::new(HashMap::<i64, StdArc<[i64]>>::new());
    let test_value: StdArc<[i64]> = StdArc::from((1..=20).collect::<Vec<i64>>().into_boxed_slice());
    {
        let mut cache_write = cache.write();
        cache_write.insert(src_node, test_value.clone());
    }

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let cache_read = cache.read();
        if let Some(neighbors) = cache_read.get(&src_node) {
            std::hint::black_box(neighbors.clone());
        }
    }
    let time2 = start.elapsed();
    let ns2 = time2.as_nanos() as f64 / ITERATIONS as f64;
    println!("2. Baseline RwLock+HashMap:      {:.2} ns/query", ns2);

    println!("\n=== COMPARISON ===");
    println!("V3Backend::neighbors():          {:.2} ns/query", ns1);
    println!("Baseline RwLock+HashMap:         {:.2} ns/query", ns2);
    println!("  Ratio: {:.1}x slower", ns1 / ns2);

    if ns1 > 10000.0 {
        println!("\n⚠️  BOTTLENECK CONFIRMED:");
        println!("   V3Backend::neighbors() is {:.2} µs/query", ns1 / 1000.0);
        println!(
            "   This is {:.1}x slower than baseline lock+hash",
            ns1 / ns2
        );
        println!("\n   Next: Need to profile V3Backend::neighbors() internals");
        println!("   - validate_snapshot_for_v3() call");
        println!("   - edge_store.read() acquisition");
        println!("   - edge_store.outgoing() call");
        println!("   - to_vec() conversion");
    }

    Ok(())
}
