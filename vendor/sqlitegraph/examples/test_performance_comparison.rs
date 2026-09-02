//! SQLite vs V3 backend microbenchmark.
//!
//! This example is a quick release-mode sanity check, not a substitute for the
//! Criterion suites under `benches/`. It measures three narrow warm-cache
//! operations:
//!
//! 1. POINT LOOKUP: Single node lookup by ID (B+tree traversal)
//! 2. ADJACENCY FETCH: Getting neighbors of a node (cached)
//! 3. TRAVERSAL: BFS/DFS through multiple hops (bulk operation)
//!
//! Run it with:
//!
//! `cargo run --release --example test_performance_comparison --features native-v3`

use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  SQLiteGraph Backend Performance Comparison");
    println!("═══════════════════════════════════════════════════════════════════");
    println!();
    println!("  This test measures three distinct operations:");
    println!("  1. POINT LOOKUP:    Single node lookup by ID");
    println!("  2. ADJACENCY FETCH: Getting neighbors (warm cache)");
    println!("  3. TRAVERSAL:       BFS through graph (bulk operation)");
    println!("═══════════════════════════════════════════════════════════════════\n");

    test_point_lookup()?;
    test_adjacency_fetch()?;
    test_traversal()?;

    println!("\n═══════════════════════════════════════════════════════════════════");
    println!("  SUMMARY: How to interpret these numbers");
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  • This example runs a warm-cache microbenchmark in release mode.");
    println!("  • It is useful for fast sanity checks and regression spotting.");
    println!("  • For workload-level backend comparisons, run the Criterion suites:");
    println!("      cargo bench --features native-v3 --bench backend_comparison");
    println!("      cargo bench --features native-v3 --bench sqlite_v3_comparison");
    println!("  • Do not generalize these numbers across different graph shapes,");
    println!("    cache states, storage media, or hardware without rerunning.");
    println!("═══════════════════════════════════════════════════════════════════");

    Ok(())
}

fn test_point_lookup() -> Result<(), Box<dyn std::error::Error>> {
    println!("───────────────────────────────────────────────────────────────────");
    println!("  TEST 1: POINT LOOKUP (Single node by ID)");
    println!("───────────────────────────────────────────────────────────────────");

    let sqlite_time;
    let v3_time;

    // SQLite
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::sqlite())?;

        let mut node_ids = Vec::new();
        for i in 0..1000 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })?;
            node_ids.push(id);
        }

        let snapshot = SnapshotId::current();
        let target = node_ids[500];

        for _ in 0..100 {
            let _ = graph.get_node(snapshot, target)?;
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let _ = graph.get_node(snapshot, target)?;
        }
        sqlite_time = start.elapsed().as_nanos() / 10000;
        println!("  SQLite:  {} ns/lookup  (B-tree optimized)", sqlite_time);
    }

    // V3
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::native())?;

        let mut node_ids = Vec::new();
        for i in 0..1000 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })?;
            node_ids.push(id);
        }

        let snapshot = SnapshotId::current();
        let target = node_ids[500];

        for _ in 0..100 {
            let _ = graph.get_node(snapshot, target)?;
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let _ = graph.get_node(snapshot, target)?;
        }
        v3_time = start.elapsed().as_nanos() / 10000;
        println!("  V3:      {} ns/lookup  (B+tree + page decode)", v3_time);
    }

    if sqlite_time < v3_time {
        let speedup = v3_time as f64 / sqlite_time as f64;
        println!(
            "\n  Result: SQLite is {:.1}× faster for point lookups",
            speedup
        );
        println!("  Why: SQLite's B-tree has decades of optimization\n");
    } else {
        let speedup = sqlite_time as f64 / v3_time as f64;
        println!("\n  Result: V3 is {:.1}× faster for point lookups", speedup);
        println!("  Why: V3's B+tree + page decode keeps lookups cache-warm\n");
    }

    Ok(())
}

fn test_adjacency_fetch() -> Result<(), Box<dyn std::error::Error>> {
    println!("───────────────────────────────────────────────────────────────────");
    println!("  TEST 2: ADJACENCY FETCH (Get neighbors - warm cache)");
    println!("───────────────────────────────────────────────────────────────────");

    let sqlite_time;
    let v3_time;

    // SQLite
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::sqlite())?;

        let mut node_ids = Vec::new();
        for i in 0..100 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })?;
            node_ids.push(id);
        }

        for j in 1..=20 {
            graph.insert_edge(EdgeSpec {
                from: node_ids[0],
                to: node_ids[j],
                edge_type: "test".to_string(),
                data: serde_json::Value::Null,
            })?;
        }

        let snapshot = SnapshotId::current();
        let query = NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        };

        for _ in 0..100 {
            let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
        }
        sqlite_time = start.elapsed().as_nanos() / 10000;
        println!(
            "  SQLite:  {} ns/fetch  (prepared statement + index)",
            sqlite_time
        );
    }

    // V3
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::native())?;

        let mut node_ids = Vec::new();
        for i in 0..100 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })?;
            node_ids.push(id);
        }

        for j in 1..=20 {
            graph.insert_edge(EdgeSpec {
                from: node_ids[0],
                to: node_ids[j],
                edge_type: "test".to_string(),
                data: serde_json::Value::Null,
            })?;
        }

        let snapshot = SnapshotId::current();
        let query = NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        };

        for _ in 0..100 {
            let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let _ = graph.neighbors(snapshot, node_ids[0], query.clone())?;
        }
        v3_time = start.elapsed().as_nanos() / 10000;
        println!("  V3:      {} ns/fetch  (HashMap + Arc::clone)", v3_time);
    }

    let ratio = sqlite_time as f64 / v3_time as f64;
    if ratio > 1.0 {
        println!("\n  Result: V3 is {:.1}× faster for adjacency fetch", ratio);
    } else {
        println!(
            "\n  Result: SQLite is {:.1}× faster for adjacency fetch",
            1.0 / ratio
        );
    }
    println!("  Note: Both are fast; difference is in API overhead\n");

    Ok(())
}

fn test_traversal() -> Result<(), Box<dyn std::error::Error>> {
    println!("───────────────────────────────────────────────────────────────────");
    println!("  TEST 3: TRAVERSAL (BFS - 3 hops from start)");
    println!("───────────────────────────────────────────────────────────────────");
    println!("  Traversal behavior depends on graph shape, cache state, and backend layout\n");

    let sqlite_time;
    let v3_time;

    // SQLite
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::sqlite())?;

        let mut node_ids = Vec::new();
        for i in 0..100 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })?;
            node_ids.push(id);
        }

        for i in 0..99 {
            graph.insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "next".to_string(),
                data: serde_json::Value::Null,
            })?;
        }

        let snapshot = SnapshotId::current();

        let start = Instant::now();
        for _ in 0..1000 {
            let _ = graph.bfs(snapshot, node_ids[0], 3)?;
        }
        // Use nanos to avoid rounding to 0 when the average per-BFS
        // is under 1 ms (TEST 3 is intentionally a fast micro-op).
        sqlite_time = start.elapsed().as_nanos() as f64 / 1000.0 / 1_000_000.0;
        println!("  SQLite:  {:.6} ms/BFS  (3 hops, 100 nodes)", sqlite_time);
    }

    // V3
    {
        let temp_dir = tempdir()?;
        let graph = open_graph(temp_dir.path().join("test.db"), &GraphConfig::native())?;

        let mut node_ids = Vec::new();
        for i in 0..100 {
            let id = graph.insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })?;
            node_ids.push(id);
        }

        for i in 0..99 {
            graph.insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "next".to_string(),
                data: serde_json::Value::Null,
            })?;
        }

        let snapshot = SnapshotId::current();

        let start = Instant::now();
        for _ in 0..1000 {
            let _ = graph.bfs(snapshot, node_ids[0], 3)?;
        }
        v3_time = start.elapsed().as_nanos() as f64 / 1000.0 / 1_000_000.0;
        println!("  V3:      {:.6} ms/BFS  (3 hops, 100 nodes)", v3_time);
    }

    if v3_time < sqlite_time {
        let speedup = sqlite_time / v3_time;
        println!("\n  Result: V3 is {:.1}× faster for traversal", speedup);
        println!("  Why: Contiguous adjacency storage reduces I/O\n");
    } else {
        let speedup = v3_time / sqlite_time;
        println!("\n  Result: SQLite is {:.1}× faster for traversal", speedup);
        println!("  Why: SQLite outperformed V3 on this dataset/hardware\n");
    }

    Ok(())
}
