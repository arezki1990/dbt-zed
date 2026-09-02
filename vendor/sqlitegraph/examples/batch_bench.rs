//! Benchmark: Batched vs synchronous inserts
//!
//! Tests if write batching collapses the 43x slowdown vs SQLite

use std::time::Instant;
use tempfile::TempDir;

// V3 imports
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};

fn main() {
    println!("=== Write Batching Benchmark ===\n");

    // Test 1: SQLite in-transaction (baseline)
    println!("1. SQLite Backend (in-transaction)");
    bench_sqlite();

    // Test 2: V3 synchronous (auto-commit mode)
    println!("\n2. V3 Backend - Synchronous (fsync per op)");
    bench_v3_synchronous();

    // Test 3: V3 batched (new batch API)
    println!("\n3. V3 Backend - Batched (single fsync at end)");
    bench_v3_batched();

    // Test 4: V3 with just deferred sync (no batch API overhead)
    println!("\n4. V3 Backend - Deferred sync test");
    bench_v3_deferred_sync();

    println!("\n=== Analysis ===");
    println!("If Test 4 >> Test 2, then sync_header/flush_to_disk are the bottleneck.");
    println!("If Test 4 ≈ Test 2, then node_store page writes are the bottleneck.");
}

fn bench_sqlite() {
    use sqlitegraph::backend::SqliteGraphBackend;

    let backend = SqliteGraphBackend::in_memory().unwrap();
    let count = 1000;

    let start = Instant::now();
    for i in 0..count {
        let _ = backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    println!("   Inserted {} nodes in {:?}", count, elapsed);
    println!(
        "   Per-node: {:.3}ms",
        elapsed.as_secs_f64() * 1000.0 / count as f64
    );
}

fn bench_v3_synchronous() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_sync.db");

    let backend = V3Backend::create(&db_path).unwrap();
    let count = 1000;

    let start = Instant::now();
    for i in 0..count {
        let _ = backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    println!("   Inserted {} nodes in {:?}", count, elapsed);
    println!(
        "   Per-node: {:.3}ms",
        elapsed.as_secs_f64() * 1000.0 / count as f64
    );
}

fn bench_v3_batched() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_batch.db");

    let backend = V3Backend::create(&db_path).unwrap();
    let count = 1000;

    let start = Instant::now();
    {
        let mut batch = backend.begin_batch();
        for i in 0..count {
            if i % 100 == 0 {
                println!("  Inserting node {}...", i);
            }
            match batch.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            }) {
                Ok(_) => {}
                Err(e) => {
                    println!("  ERROR at node {}: {}", i, e);
                    break;
                }
            }
        }
        batch.commit().unwrap();
    }
    let elapsed = start.elapsed();

    println!("   Inserted {} nodes in {:?}", count, elapsed);
    println!(
        "   Per-node: {:.3}ms",
        elapsed.as_secs_f64() * 1000.0 / count as f64
    );
}

fn bench_v3_deferred_sync() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_deferred.db");

    let backend = V3Backend::create(&db_path).unwrap();
    let count = 1000;

    // Use inner methods directly to bypass sync entirely during inserts
    let start = Instant::now();
    for i in 0..count {
        let _ = backend
            .insert_node_inner(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }
    // Single sync at end (using checkpoint for simplicity)
    backend.checkpoint().unwrap();
    let elapsed = start.elapsed();

    println!("   Inserted {} nodes in {:?}", count, elapsed);
    println!(
        "   Per-node: {:.3}ms",
        elapsed.as_secs_f64() * 1000.0 / count as f64
    );
}
