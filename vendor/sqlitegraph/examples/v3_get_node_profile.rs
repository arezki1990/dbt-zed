//! Focused V3 get_node profiling benchmark
//!
//! Run with:
//!   cargo run --example v3_get_node_profile --release --features "native-v3,v3-forensics"
//!
//! Profile with:
//!   perf record -g --call-graph dwarf -- target/release/examples/v3_get_node_profile
//!   perf report --stdio

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::snapshot::SnapshotId;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("profile.db");

    println!("Creating database with 10K nodes...");
    let backend = V3Backend::create(&db_path)?;

    // Create 10K nodes to get realistic page distribution
    for i in 0..10_000 {
        backend.insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i}),
        })?;
    }
    backend.flush_to_disk()?;
    drop(backend);

    println!("Reopening for warm cache...");
    let backend = V3Backend::open(&db_path)?;
    let snapshot_id = SnapshotId::current();

    // Warm up cache
    for i in 0..100 {
        let node_id = (i as i64) * 100 + 1;
        let _ = backend.get_node(snapshot_id, node_id);
    }

    println!("Running focused get_node benchmark (100K lookups)...");
    let start = Instant::now();

    // Do 100K get_node calls - enough for profiling
    for i in 0..100_000 {
        // Cycle through first 1000 nodes
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
        println!("\n=== FORENSIC COUNTERS ===");
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
                .node_page_cache_miss_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    Ok(())
}
