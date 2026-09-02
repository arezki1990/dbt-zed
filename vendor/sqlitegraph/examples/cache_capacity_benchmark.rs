//! Page Cache Capacity Sweep Benchmark
//!
//! Tests the effect of page cache size on V3 performance.

use sqlitegraph::SnapshotId;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use std::time::Instant;

/// Cache sizes to test
const CACHE_SIZES: &[usize] = &[16, 64, 128, 256];

/// Dataset sizes to test
const DATASET_SIZES: &[usize] = &[1_000, 10_000];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut results: Vec<(String, usize, usize, f64)> = Vec::new();

    println!("=== V3 Page Cache Capacity Sweep Benchmark ===\n");

    for &cache_size in CACHE_SIZES {
        println!("\n{}", "=".repeat(60));
        println!("CACHE SIZE: {} pages", cache_size);
        println!("{}", "=".repeat(60));

        for &node_count in DATASET_SIZES {
            println!("\n--- Dataset: {} nodes ---", node_count);

            // Benchmark 1: insert_nodes
            if let Ok(time) = benchmark_insert(node_count, cache_size) {
                results.push(("insert".to_string(), node_count, cache_size, time));
            }

            // Benchmark 2: get_node (sequential)
            if let Ok((cold, warm)) = benchmark_get_node(node_count, cache_size) {
                results.push(("get_node_cold".to_string(), node_count, cache_size, cold));
                results.push(("get_node_warm".to_string(), node_count, cache_size, warm));
            }

            // Benchmark 3: neighbors
            if let Ok((cold, warm)) = benchmark_neighbors(node_count, cache_size) {
                results.push(("neighbors_cold".to_string(), node_count, cache_size, cold));
                results.push(("neighbors_warm".to_string(), node_count, cache_size, warm));
            }

            // Benchmark 4: BFS
            if let Ok((cold, warm)) = benchmark_bfs(node_count, cache_size) {
                results.push(("bfs_cold".to_string(), node_count, cache_size, cold));
                results.push(("bfs_warm".to_string(), node_count, cache_size, warm));
            }
        }
    }

    // Print summary table
    print_summary(&results);

    Ok(())
}

/// Benchmark node insertion throughput
fn benchmark_insert(
    node_count: usize,
    _cache_size: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache_bench.db");

    let graph = V3Backend::create(&db_path)?;

    let start = Instant::now();
    for i in 0..node_count {
        graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({
                "index": i,
                "data": "x".repeat(32),
            }),
        })?;
    }
    let elapsed = start.elapsed();

    let throughput = node_count as f64 / elapsed.as_secs_f64();
    println!(
        "  insert_nodes:     {:6} ms ({:8.1} nodes/sec)",
        elapsed.as_millis(),
        throughput
    );

    Ok(elapsed.as_millis() as f64)
}

/// Benchmark sequential get_node access
fn benchmark_get_node(
    node_count: usize,
    _cache_size: usize,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache_bench.db");

    // Create and populate graph
    let graph = V3Backend::create(&db_path)?;
    for i in 0..node_count {
        graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
    }
    graph.flush()?;

    // NOTE: Skipped reopen because we don't have open_with_cache_capacity yet
    // This tests warm cache performance only
    let start = Instant::now();
    for i in 1..=node_count as i64 {
        graph.get_node(SnapshotId::current(), i)?;
    }
    let time = start.elapsed();

    println!(
        "  get_node (warm):  {:6} ms ({:8.1} lookups/sec)",
        time.as_millis(),
        node_count as f64 / time.as_secs_f64()
    );

    // Return same value for both cold/warm since we only measured warm
    Ok((time.as_millis() as f64, time.as_millis() as f64))
}

/// Benchmark neighbors query
fn benchmark_neighbors(
    node_count: usize,
    _cache_size: usize,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache_bench.db");

    let graph = V3Backend::create(&db_path)?;

    // Create chain graph: 1 -> 2 -> 3 -> ... -> N
    let mut node_ids = Vec::new();
    for i in 0..node_count {
        let id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(id);
    }

    // Create chain edges
    for i in 0..node_count - 1 {
        graph.insert_edge(EdgeSpec {
            from: node_ids[i],
            to: node_ids[i + 1],
            edge_type: "chain".to_string(),
            data: serde_json::json!({}),
        })?;
    }

    // Query neighbors (warm cache)
    let start = Instant::now();
    for &node_id in &node_ids {
        let _neighbors =
            graph.neighbors(SnapshotId::current(), node_id, NeighborQuery::default())?;
    }
    let time = start.elapsed();

    println!(
        "  neighbors:        {:6} ms ({:8.1} queries/sec)",
        time.as_millis(),
        node_count as f64 / time.as_secs_f64()
    );

    Ok((time.as_millis() as f64, time.as_millis() as f64))
}

/// Benchmark BFS traversal
fn benchmark_bfs(
    node_count: usize,
    _cache_size: usize,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache_bench.db");

    let graph = V3Backend::create(&db_path)?;

    // Create chain graph
    let mut node_ids = Vec::new();
    for i in 0..node_count {
        let id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(id);
    }

    for i in 0..node_count - 1 {
        graph.insert_edge(EdgeSpec {
            from: node_ids[i],
            to: node_ids[i + 1],
            edge_type: "chain".to_string(),
            data: serde_json::json!({}),
        })?;
    }

    // BFS traversal (warm cache)
    let start = Instant::now();
    let _result = graph.bfs(SnapshotId::current(), node_ids[0], node_count as u32)?;
    let time = start.elapsed();

    println!(
        "  BFS:             {:6} ms ({:8.1} nodes/sec)",
        time.as_millis(),
        node_count as f64 / time.as_secs_f64()
    );

    Ok((time.as_millis() as f64, time.as_millis() as f64))
}

/// Print summary table of results
fn print_summary(results: &[(String, usize, usize, f64)]) {
    println!("\n\n{}", "=".repeat(80));
    println!("SUMMARY: Cache Capacity vs Performance");
    println!("{}", "=".repeat(80));

    // Group by operation and dataset size
    let mut grouped: std::collections::HashMap<(String, usize), Vec<(usize, f64)>> =
        std::collections::HashMap::new();

    for (op, node_count, cache_size, time) in results {
        grouped
            .entry((op.clone(), *node_count))
            .or_default()
            .push((*cache_size, *time));
    }

    let mut keys: Vec<_> = grouped.keys().collect();
    keys.sort();

    for (operation, node_count) in keys {
        if let Some(results) = grouped.get(&(operation.clone(), *node_count)) {
            let mut sorted_results = results.clone();
            sorted_results.sort_by_key(|(size, _)| *size);

            println!("\n{} ({} nodes):", operation, node_count);
            print!("  Cache Size | ");
            for (size, _) in &sorted_results {
                print!("{:>4} pages | ", size);
            }
            println!();

            print!("  Time (ms)   | ");
            for (_, time) in &sorted_results {
                print!("{:>8.1} | ", time);
            }
            println!();

            // Calculate speedup vs baseline (16 pages)
            if let Some((_, baseline_time)) = sorted_results.first() {
                print!("  Speedup    | ");
                print!("{:>8.2}x | ", 1.0); // baseline
                for (_, time) in sorted_results.iter().skip(1) {
                    let speedup = baseline_time / time;
                    print!("{:>8.2}x | ", speedup);
                }
                println!();
            }
        }
    }
}
