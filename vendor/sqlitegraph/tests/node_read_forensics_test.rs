//! Node Read Path Forensics Test
//!
//! This test measures the cost breakdown of get_node to identify the dominant cost:
//! - B+Tree traversal depth
//! - Page cache hits/misses
//! - NodePage::unpack() calls
//! - Linear scan steps per unpack
//!
//! Run with: cargo test --test node_read_forensics_test --features "v3-forensics,native-v3" -- --nocapture

// NOTE: This test requires the native-v3 feature to access V3Backend
#![cfg(feature = "native-v3")]

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;

fn print_forensic_counters(_prefix: &str) {
    #[cfg(feature = "v3-forensics")]
    {
        println!("\n=== {} ===", prefix);
        println!(
            "logical_get_node_calls:   {}",
            FORENSIC_COUNTERS
                .logical_get_node_calls
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_lookup_calls:       {}",
            FORENSIC_COUNTERS
                .btree_lookup_calls
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_traversal_depth:    {}",
            FORENSIC_COUNTERS
                .btree_traversal_depth_total
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "page_read_count:          {}",
            FORENSIC_COUNTERS
                .page_read_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_cache_hit_count:    {}",
            FORENSIC_COUNTERS
                .btree_cache_hit_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "btree_cache_miss_count:   {}",
            FORENSIC_COUNTERS
                .btree_cache_miss_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_cache_hit:      {}",
            FORENSIC_COUNTERS
                .node_page_cache_hit_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_cache_miss:     {}",
            FORENSIC_COUNTERS
                .node_page_cache_miss_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_page_unpack_count:   {}",
            FORENSIC_COUNTERS
                .node_page_unpack_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_linear_scan_steps:   {}",
            FORENSIC_COUNTERS
                .node_linear_scan_steps
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "node_decode_count:        {}",
            FORENSIC_COUNTERS
                .node_decode_count
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

#[cfg(feature = "v3-forensics")]
fn reset_counters() {
    FORENSIC_COUNTERS.reset();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_node_read_forensics_cold_cache() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("forensics_cold.db");

    println!("\n═══════════════════════════════════════════════════════════");
    println!("           NODE READ PATH FORENSICS - COLD CACHE             ");
    println!("═══════════════════════════════════════════════════════════");

    // Create database with 1K nodes spread across multiple pages
    let node_count = 1000;
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..node_count {
            let kind = format!("Kind{}", i % 5);
            let name = format!("node_{:05}", i);
            backend
                .insert_node(sqlitegraph::backend::NodeSpec {
                    kind,
                    name,
                    file_path: None,
                    data: serde_json::json!({"value": i}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    // Reset counters for clean measurement
    reset_counters();

    // Reopen (simulates cold cache)
    let backend = V3Backend::open(&db_path).unwrap();
    let snapshot_id = SnapshotId::current();

    // Measure sequential reads of 100 nodes (ensures cold cache for each page)
    let start = Instant::now();
    for i in 0..100 {
        let node_id = (i as i64) * 10 + 1; // Spread across pages
        let _node = backend.get_node(snapshot_id, node_id);
    }
    let duration = start.elapsed();

    print_forensic_counters("COLD CACHE RESULTS (100 nodes, sequential)");
    println!("Total time: {:?}", duration);
    println!("Time per node: {:?}", duration / 100);

    // Calculate averages
    let get_calls = FORENSIC_COUNTERS
        .logical_get_node_calls
        .load(std::sync::atomic::Ordering::Relaxed);
    let btree_depth = FORENSIC_COUNTERS
        .btree_traversal_depth_total
        .load(std::sync::atomic::Ordering::Relaxed);
    let btree_calls = FORENSIC_COUNTERS
        .btree_lookup_calls
        .load(std::sync::atomic::Ordering::Relaxed);
    let unpack_calls = FORENSIC_COUNTERS
        .node_page_unpack_count
        .load(std::sync::atomic::Ordering::Relaxed);
    let scan_steps = FORENSIC_COUNTERS
        .node_linear_scan_steps
        .load(std::sync::atomic::Ordering::Relaxed);

    println!("\n--- PER-OPERATION AVERAGES ---");
    if btree_calls > 0 {
        println!(
            "Avg B+Tree depth per lookup: {:.1}",
            btree_depth as f64 / btree_calls as f64
        );
    }
    if unpack_calls > 0 {
        println!(
            "Avg linear scan steps per unpack: {:.1}",
            scan_steps as f64 / unpack_calls as f64
        );
    }

    // Print full report
    FORENSIC_COUNTERS.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_node_read_forensics_warm_cache() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("forensics_warm.db");

    println!("\n═══════════════════════════════════════════════════════════");
    println!("           NODE READ PATH FORENSICS - WARM CACHE             ");
    println!("═══════════════════════════════════════════════════════════");

    // Create database with 1K nodes
    let node_count = 1000;
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..node_count {
            backend
                .insert_node(sqlitegraph::backend::NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"value": i}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    let backend = V3Backend::open(&db_path).unwrap();
    let snapshot_id = SnapshotId::current();

    // First pass: warm up cache
    for i in 0..100 {
        let node_id = (i as i64) * 10 + 1;
        let _node = backend.get_node(snapshot_id, node_id);
    }

    // Reset counters
    reset_counters();

    // Second pass: measure warm cache performance
    let start = Instant::now();
    for i in 0..100 {
        let node_id = (i as i64) * 10 + 1;
        let _node = backend.get_node(snapshot_id, node_id);
    }
    let duration = start.elapsed();

    print_forensic_counters("WARM CACHE RESULTS (100 nodes, sequential)");
    println!("Total time: {:?}", duration);
    println!("Time per node: {:?}", duration / 100);

    // Calculate averages
    let get_calls = FORENSIC_COUNTERS
        .logical_get_node_calls
        .load(std::sync::atomic::Ordering::Relaxed);
    let btree_depth = FORENSIC_COUNTERS
        .btree_traversal_depth_total
        .load(std::sync::atomic::Ordering::Relaxed);
    let btree_calls = FORENSIC_COUNTERS
        .btree_lookup_calls
        .load(std::sync::atomic::Ordering::Relaxed);
    let unpack_calls = FORENSIC_COUNTERS
        .node_page_unpack_count
        .load(std::sync::atomic::Ordering::Relaxed);
    let scan_steps = FORENSIC_COUNTERS
        .node_linear_scan_steps
        .load(std::sync::atomic::Ordering::Relaxed);

    println!("\n--- PER-OPERATION AVERAGES ---");
    if btree_calls > 0 {
        println!(
            "Avg B+Tree depth per lookup: {:.1}",
            btree_depth as f64 / btree_calls as f64
        );
    }
    if unpack_calls > 0 {
        println!(
            "Avg linear scan steps per unpack: {:.1}",
            scan_steps as f64 / unpack_calls as f64
        );
    }

    FORENSIC_COUNTERS.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_node_read_forensics_same_node() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("forensics_same.db");

    println!("\n═══════════════════════════════════════════════════════════");
    println!("      NODE READ PATH FORENSICS - REPEATED SAME NODE          ");
    println!("═══════════════════════════════════════════════════════════");

    // Create database
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(sqlitegraph::backend::NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    let backend = V3Backend::open(&db_path).unwrap();
    let snapshot_id = SnapshotId::current();

    // Warm up cache
    let _node = backend.get_node(snapshot_id, 50);

    reset_counters();

    // Read same node 1000 times
    let start = Instant::now();
    for _ in 0..1000 {
        let _node = backend.get_node(snapshot_id, 50);
    }
    let duration = start.elapsed();

    print_forensic_counters("REPEATED SAME NODE RESULTS (1000 reads)");
    println!("Total time: {:?}", duration);
    println!("Time per read: {:?}", duration / 1000);

    FORENSIC_COUNTERS.print_report();
}
