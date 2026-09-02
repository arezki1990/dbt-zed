//! Focused forensics test for get_node performance analysis
//!
//! This test measures the actual internal cost of get_node operations
//! to identify where the 47-160x slowdown comes from.

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::backend::V3Backend;
#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
#[cfg(feature = "v3-forensics")]
use std::sync::atomic::Ordering;

#[cfg(feature = "v3-forensics")]
const SMALL_NODES: usize = 100;
#[cfg(feature = "v3-forensics")]
const MEDIUM_NODES: usize = 10_000;

#[cfg(feature = "v3-forensics")]
fn create_db_with_nodes(
    path: &std::path::Path,
    count: usize,
) -> Result<Vec<i64>, Box<dyn std::error::Error>> {
    let backend = V3Backend::create(path)?;
    let mut node_ids = Vec::new();

    // Insert nodes with sequential IDs
    for i in 1..=count {
        let id = backend.insert_node(NodeSpec {
            kind: format!("Node_{}", i),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(id);
    }

    // Flush to ensure data is on disk
    backend.flush()?;

    Ok(node_ids)
}

#[cfg(feature = "v3-forensics")]
fn print_counter_snapshot() {
    // Read path counters
    let logical_get_node = FORENSIC_COUNTERS
        .logical_get_node_calls
        .load(Ordering::Relaxed);
    let btree_lookup = FORENSIC_COUNTERS.btree_lookup_calls.load(Ordering::Relaxed);
    let page_reads = FORENSIC_COUNTERS.page_read_count.load(Ordering::Relaxed);
    let node_decode = FORENSIC_COUNTERS.node_decode_count.load(Ordering::Relaxed);

    println!("\n  COUNTER SNAPSHOT:");
    println!("    Logical get_node calls:      {}", logical_get_node);
    println!("    B+Tree lookup calls:         {}", btree_lookup);
    println!("    Page reads (total):          {}", page_reads);
    println!("    Node decodes:                {}", node_decode);

    // Cache counters
    let btree_hits = FORENSIC_COUNTERS
        .btree_cache_hit_count
        .load(Ordering::Relaxed);
    let btree_misses = FORENSIC_COUNTERS
        .btree_cache_miss_count
        .load(Ordering::Relaxed);
    let node_hits = FORENSIC_COUNTERS
        .node_page_cache_hit_count
        .load(Ordering::Relaxed);
    let node_misses = FORENSIC_COUNTERS
        .node_page_cache_miss_count
        .load(Ordering::Relaxed);

    let btree_total = btree_hits + btree_misses;
    let btree_hit_rate = if btree_total > 0 {
        (btree_hits as f64 / btree_total as f64) * 100.0
    } else {
        0.0
    };

    let node_total = node_hits + node_misses;
    let node_hit_rate = if node_total > 0 {
        (node_hits as f64 / node_total as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "    B+Tree cache: {}/{} hits ({:.1}%)",
        btree_hits, btree_total, btree_hit_rate
    );
    println!(
        "    Node page cache: {}/{} hits ({:.1}%)",
        node_hits, node_total, node_hit_rate
    );

    // Calculate average page reads per lookup
    if logical_get_node > 0 {
        let avg_pages = page_reads as f64 / logical_get_node as f64;
        println!("    Avg page reads per get_node: {:.1}", avg_pages);
    }
    println!();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_scenario_1_cold_get_node_small() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!(
        "SCENARIO 1: Cold get_node in small DB ({}) nodes",
        SMALL_NODES
    );
    println!("─────────────────────────────────────────────────────────────");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("small.db");

    // Create DB and insert nodes
    let node_ids = create_db_with_nodes(&db_path, SMALL_NODES).unwrap();

    // Reset counters for clean measurement
    FORENSIC_COUNTERS.reset();

    // Open database (counters reset)
    let backend = V3Backend::open(&db_path).unwrap();

    // Do a cold lookup (cache is empty after reset)
    let target_id = node_ids[SMALL_NODES / 2];
    let start = std::time::Instant::now();
    let result = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), target_id);
    let elapsed = start.elapsed();

    println!("  Target node ID: {}", target_id);
    println!("  Result: {:?}", result.is_ok());
    println!("  Latency: {:?}", elapsed);

    // Print counter snapshot
    print_counter_snapshot();

    // Assertions for sanity checking
    assert!(result.is_ok(), "get_node should succeed");
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_scenario_2_cold_get_node_medium() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!(
        "SCENARIO 2: Cold get_node in medium DB ({}) nodes",
        MEDIUM_NODES
    );
    println!("─────────────────────────────────────────────────────────────");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("medium.db");

    // Create DB and insert nodes
    let node_ids = create_db_with_nodes(&db_path, MEDIUM_NODES).unwrap();

    // Reset counters for clean measurement
    FORENSIC_COUNTERS.reset();

    // Open database (counters reset)
    let backend = V3Backend::open(&db_path).unwrap();

    // Do a cold lookup (cache is empty after reset)
    let target_id = node_ids[MEDIUM_NODES / 2];
    let start = std::time::Instant::now();
    let result = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), target_id);
    let elapsed = start.elapsed();

    println!("  Target node ID: {}", target_id);
    println!("  Result: {:?}", result.is_ok());
    println!("  Latency: {:?}", elapsed);

    // Print counter snapshot
    print_counter_snapshot();

    // Assertions for sanity checking
    assert!(result.is_ok(), "get_node should succeed");
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_scenario_3_warm_get_node_small() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("SCENARIO 3: Warm repeated get_node in small DB");
    println!("─────────────────────────────────────────────────────────────");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("warm_small.db");

    let node_ids = create_db_with_nodes(&db_path, SMALL_NODES).unwrap();

    let backend = V3Backend::open(&db_path).unwrap();

    // Prime the cache with a few lookups
    for i in 0..5 {
        let _ = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), node_ids[i]);
    }

    // Reset counters before measuring warm lookups
    FORENSIC_COUNTERS.reset();

    // Measure 100 warm lookups
    let iterations = 100u32;
    let start = std::time::Instant::now();

    for i in 0..iterations {
        let idx = (i as usize) % SMALL_NODES;
        let _ = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), node_ids[idx]);
    }

    let elapsed = start.elapsed();

    println!("  Iterations: {}", iterations);
    println!("  Total time: {:?}", elapsed);
    println!("  Avg latency: {:?}", elapsed / iterations);

    // Print counter snapshot
    print_counter_snapshot();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_scenario_4_warm_get_node_medium() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("SCENARIO 4: Warm repeated get_node in medium DB");
    println!("─────────────────────────────────────────────────────────────");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("warm_medium.db");

    let node_ids = create_db_with_nodes(&db_path, MEDIUM_NODES).unwrap();

    let backend = V3Backend::open(&db_path).unwrap();

    // Prime the cache with a few lookups
    for i in 0..5 {
        let idx = i * 1000;
        let _ = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), node_ids[idx]);
    }

    // Reset counters before measuring warm lookups
    FORENSIC_COUNTERS.reset();

    // Measure 100 warm lookups
    let iterations = 100u32;
    let start = std::time::Instant::now();

    for i in 0..iterations {
        let idx = (i * 100) as usize;
        let _ = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), node_ids[idx]);
    }

    let elapsed = start.elapsed();

    println!("  Iterations: {}", iterations);
    println!("  Total time: {:?}", elapsed);
    println!("  Avg latency: {:?}", elapsed / iterations);

    // Print counter snapshot
    print_counter_snapshot();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_scenario_5_reopen_get_node_medium() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("SCENARIO 5: get_node after reopen (medium DB)");
    println!("─────────────────────────────────────────────────────────────");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("reopen.db");

    let node_ids = create_db_with_nodes(&db_path, MEDIUM_NODES).unwrap();

    // Reset counters
    FORENSIC_COUNTERS.reset();

    // Reopen the database
    let backend = V3Backend::open(&db_path).unwrap();

    // Do a lookup after reopen (cold cache)
    let target_id = node_ids[MEDIUM_NODES / 2];
    let start = std::time::Instant::now();
    let result = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), target_id);
    let elapsed = start.elapsed();

    println!("  Target node ID: {}", target_id);
    println!("  Result: {:?}", result.is_ok());
    println!("  Latency: {:?}", elapsed);

    // Print counter snapshot
    print_counter_snapshot();

    // Assertions for sanity checking
    assert!(result.is_ok(), "get_node should succeed");
}
