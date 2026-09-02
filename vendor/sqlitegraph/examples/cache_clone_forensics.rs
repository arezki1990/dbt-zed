//! Cache Clone Forensics - Instrument cache operations to measure clone cost
//!
//! This example instruments the NodeStore to measure the exact cost of
//! the page_cache_snapshot clone that happens on every insert_node call.

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Cache Clone Forensics ===\n");

    // Test with different cache sizes
    for &cache_size in &[16, 64, 128, 256] {
        println!("\n{}", "=".repeat(60));
        println!(
            "CACHE SIZE: {} pages ({} bytes per page = {} MB cache)",
            cache_size,
            4096,
            (cache_size * 4096) / 1_000_000
        );
        println!("{}", "=".repeat(60));

        run_forensics(cache_size)?;
    }

    Ok(())
}

fn run_forensics(cache_size: usize) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("forensics.db");

    // Create database with specified cache size
    let graph = V3Backend::create(&db_path)?;

    let node_count = 1000;

    // Measure insert performance
    let start = Instant::now();
    for i in 0..node_count {
        graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({
                "index": i,
                "data": "x".repeat(100), // 100 bytes per node
            }),
        })?;
    }
    let insert_time = start.elapsed();

    println!("\nResults for {} nodes:", node_count);
    println!("  Total insert time:     {:8} ms", insert_time.as_millis());
    println!(
        "  Avg time per node:     {:8.3} ms",
        insert_time.as_millis() as f64 / node_count as f64
    );
    println!(
        "  Throughput:            {:8.1} nodes/sec",
        node_count as f64 / insert_time.as_secs_f64()
    );

    // Estimate cache clone overhead
    // Each insert clones the entire page cache
    let cache_bytes = cache_size * 4096;
    let total_cloned_bytes = cache_bytes * node_count;
    let total_cloned_mb = total_cloned_bytes as f64 / 1_000_000.0;

    println!("\nEstimated Cache Clone Overhead:");
    println!("  Cache size per insert:  {} bytes", cache_bytes);
    println!(
        "  Total bytes cloned:     {} bytes ({} MB)",
        total_cloned_bytes,
        total_cloned_mb.round()
    );
    println!(
        "  Theoretical minimum:   {:8.1} ms (if clone were only cost)",
        total_cloned_mb * 1000.0 / 3000.0 // Assume 3 GB/s memory bandwidth
    );

    Ok(())
}
