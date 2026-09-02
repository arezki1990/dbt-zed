//! Read-only profiling benchmark for V3 get_node
//!
//! Run with:
//!   cargo run --example v3_readonly_profile --release --features "native-v3,v3-forensics"
//!
//! Profile with:
//!   perf record -g --call-graph dwarf -- target/release/examples/v3_readonly_profile
//!   perf report --stdio
//!
//! Cache capacity sweep:
//!   for size in 16 32 64 128; do
//!     ./target/release/examples/v3_readonly_profile $size
//!   done

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::snapshot::SnapshotId;
use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 && args[1] == "setup" {
        // Setup mode: create database and exit
        let db_path = "/tmp/v3_profile_readonly.db";
        println!("Creating database with 10K nodes at {}...", db_path);
        let backend = V3Backend::create(db_path)?;

        for i in 0..10_000 {
            backend.insert_node(sqlitegraph::backend::NodeSpec {
                kind: "TestKind".to_string(),
                name: format!("node_{:05}", i),
                file_path: None,
                data: serde_json::json!({"value": i}),
            })?;
        }
        backend.flush_to_disk()?;
        println!("Setup complete. Run without 'setup' flag to profile reads.");
        return Ok(());
    }

    // Parse cache size from command line (default 16)
    let cache_capacity = if args.len() > 1 && args[1] != "setup" {
        args[1].parse::<usize>().unwrap_or(16)
    } else {
        16
    };

    // Read mode: profile only reads
    let db_path = "/tmp/v3_profile_readonly.db";

    if !std::path::Path::new(db_path).exists() {
        eprintln!("Database not found. Run with 'setup' flag first:");
        eprintln!("  {} setup", args[0]);
        std::process::exit(1);
    }

    println!(
        "Opening database for read-only profiling with cache capacity {}...",
        cache_capacity
    );
    let backend = V3Backend::open(db_path)?;
    let snapshot_id = SnapshotId::current();

    // Warm up cache
    for i in 0..100 {
        let node_id = (i as i64) * 100 + 1;
        let _ = backend.get_node(snapshot_id, node_id);
    }

    println!("Running read-only benchmark (100K lookups)...");
    let start = Instant::now();

    for i in 0..100_000 {
        let node_id = (i % 1000) as i64 * 10 + 1;
        let _ = backend.get_node(snapshot_id, node_id);
    }

    let duration = start.elapsed();
    println!("Done.");
    println!("Total time: {:?}", duration);
    println!("Time per lookup: {:?}", duration / 100_000);

    // Print forensic counters
    #[cfg(feature = "v3-forensics")]
    {
        use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
        println!(
            "\n=== FORENSIC COUNTERS (cache_capacity: {}) ===",
            cache_capacity
        );
        println!(
            "logical_get_node_calls:    {}",
            FORENSIC_COUNTERS
                .logical_get_node_calls
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_lookup_calls:        {}",
            FORENSIC_COUNTERS
                .btree_lookup_calls
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_traversal_depth:     {}",
            FORENSIC_COUNTERS
                .btree_traversal_depth_total
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "page_read_count:           {}",
            FORENSIC_COUNTERS
                .page_read_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_unpack_count:    {}",
            FORENSIC_COUNTERS
                .node_page_unpack_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_linear_scan_steps:    {}",
            FORENSIC_COUNTERS
                .node_linear_scan_steps
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_cache_hit_count: {}",
            FORENSIC_COUNTERS
                .node_page_cache_hit_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_cache_miss:      {}",
            FORENSIC_COUNTERS
                .node_page_cache_miss
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    Ok(())
}
