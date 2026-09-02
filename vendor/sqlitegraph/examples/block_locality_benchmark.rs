//! Block-Locality Prototype Benchmark

use sqlitegraph::SnapshotId;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use std::path::PathBuf;
use std::time::Instant;

/// Benchmark configuration
struct BenchmarkConfig {
    name: &'static str,
    node_count: usize,
    access_pattern: AccessPattern,
}

/// Access patterns to test
#[derive(Debug)]
enum AccessPattern {
    Sequential,
    Random,
    BlockSequential,
    BlockSkip,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Block-Locality Prototype Benchmark ===");
    println!("PROTOTYPE: Metadata-only block-aware caching\n");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("block_locality.db");

    // Test with different dataset sizes
    let configs = vec![
        BenchmarkConfig {
            name: "Small Sequential",
            node_count: 1_000,
            access_pattern: AccessPattern::Sequential,
        },
        BenchmarkConfig {
            name: "Small Random",
            node_count: 1_000,
            access_pattern: AccessPattern::Random,
        },
        BenchmarkConfig {
            name: "Small Block-Sequential",
            node_count: 1_000,
            access_pattern: AccessPattern::BlockSequential,
        },
        BenchmarkConfig {
            name: "Small Block-Skip",
            node_count: 1_000,
            access_pattern: AccessPattern::BlockSkip,
        },
        BenchmarkConfig {
            name: "Medium Sequential",
            node_count: 10_000,
            access_pattern: AccessPattern::Sequential,
        },
        BenchmarkConfig {
            name: "Medium Random",
            node_count: 10_000,
            access_pattern: AccessPattern::Random,
        },
        BenchmarkConfig {
            name: "Medium Block-Sequential",
            node_count: 10_000,
            access_pattern: AccessPattern::BlockSequential,
        },
    ];

    for config in configs {
        println!("\n--- {} ({} nodes) ---", config.name, config.node_count);
        run_benchmark(&db_path, &config)?;
    }

    Ok(())
}

fn run_benchmark(
    db_path: &PathBuf,
    config: &BenchmarkConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Clean up any existing database
    if db_path.exists() {
        std::fs::remove_file(db_path)?;
    }

    // Create V3 backend directly (not via open_graph which defaults to V2)
    let graph = V3Backend::create_with_wal(db_path, true)?;

    // PHASE 1: Insert nodes
    println!("Inserting {} nodes...", config.node_count);
    let insert_start = Instant::now();

    for i in 0..config.node_count {
        graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({
                "index": i,
                "data": "x".repeat(32), // 32 bytes of inline data
            }),
        })?;
    }

    let insert_time = insert_start.elapsed();
    println!(
        "Insert time: {} ms ({:.1} nodes/sec)",
        insert_time.as_millis(),
        config.node_count as f64 / insert_time.as_secs_f64()
    );

    // Flush to ensure data is on disk
    graph.flush()?;

    // PHASE 2: Cold get_node benchmark (simulates reopen scenario)
    println!(
        "Running cold get_node benchmark ({:?})...",
        config.access_pattern
    );

    // Clear caches by reopening the database
    drop(graph);
    let graph = V3Backend::open(db_path)?;

    let access_sequence = generate_access_sequence(config.node_count, &config.access_pattern);
    let lookup_start = Instant::now();
    let mut found_count = 0;

    for node_id in &access_sequence {
        if graph.get_node(SnapshotId::current(), *node_id).is_ok() {
            found_count += 1;
        }
    }

    let lookup_time = lookup_start.elapsed();
    println!(
        "Cold lookup time: {} ms ({:.1} lookups/sec)",
        lookup_time.as_millis(),
        config.node_count as f64 / lookup_time.as_secs_f64()
    );
    println!("Found {}/{} nodes", found_count, config.node_count);

    // PHASE 3: Warm get_node benchmark (cached)
    println!("Running warm get_node benchmark...");
    let warm_start = Instant::now();

    for node_id in &access_sequence {
        let _ = graph.get_node(SnapshotId::current(), *node_id);
    }

    let warm_time = warm_start.elapsed();
    println!(
        "Warm lookup time: {} ms ({:.1} lookups/sec)",
        warm_time.as_millis(),
        config.node_count as f64 / warm_time.as_secs_f64()
    );

    // Calculate speedup
    let speedup = lookup_time.as_secs_f64() / warm_time.as_secs_f64();
    println!("Cache speedup: {:.2}x", speedup);

    // File size metrics
    if let Ok(metadata) = std::fs::metadata(db_path) {
        let file_size = metadata.len();
        println!(
            "File size: {} bytes ({:.2} KB)",
            file_size,
            file_size as f64 / 1024.0
        );
        println!(
            "Bytes per node: {:.2}",
            file_size as f64 / config.node_count as f64
        );
    }

    Ok(())
}

fn generate_access_sequence(count: usize, pattern: &AccessPattern) -> Vec<i64> {
    match pattern {
        AccessPattern::Sequential => (1..=count as i64).collect(),
        AccessPattern::Random => {
            use std::collections::HashSet;
            let mut rng = rand::thread_rng();
            let mut sequence = Vec::new();
            let mut seen = HashSet::new();

            while sequence.len() < count {
                let id = rand::Rng::gen_range(&mut rng, 1..=count as i64);
                if seen.insert(id) {
                    sequence.push(id);
                }
            }
            sequence
        }
        AccessPattern::BlockSequential => {
            // Access nodes in blocks: 1-128, then 1-128 again, then 129-256, etc.
            const BLOCK_SIZE: i64 = 128;
            let mut sequence = Vec::new();

            let mut block_start = 1;
            while block_start <= count as i64 {
                let block_end = (block_start + BLOCK_SIZE - 1).min(count as i64);
                for id in block_start..=block_end {
                    sequence.push(id);
                }
                // Access the same block again to test cache retention
                for id in block_start..=block_end {
                    sequence.push(id);
                }
                block_start = block_end + 1;
            }
            sequence
        }
        AccessPattern::BlockSkip => {
            // Access alternating blocks: 1-128, 257-384, 1-128, 257-384, ...
            // This tests whether cache retains pages from "distant" blocks
            const BLOCK_SIZE: i64 = 128;
            let mut sequence = Vec::new();
            let _num_repeats = (count as i64 + BLOCK_SIZE - 1) / BLOCK_SIZE; // Approximate

            for _ in 0..2 {
                let mut block_start = 1;
                while block_start <= count as i64 {
                    let block_end = (block_start + BLOCK_SIZE - 1).min(count as i64);
                    for id in block_start..=block_end {
                        sequence.push(id);
                    }
                    block_start = block_end + 1 + BLOCK_SIZE; // Skip one block
                }
            }
            sequence
        }
    }
}
