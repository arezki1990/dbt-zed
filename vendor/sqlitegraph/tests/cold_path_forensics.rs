//! Cold-Path Forensics: Measure real open() vs first-read vs warm-read costs
//!
//! This test measures the actual cold-path behavior now that the rebuild_indexes()
//! preload bug is fixed. We separate:
//! 1. Pure open() cost
//! 2. First get_node after open (cold)
//! 3. Second get_node (warm)
//! 4. First neighbors after open (cold)
//! 5. Second neighbors (warm)
//!
//! Run with: cargo test --features native-v3,v3-forensics --test cold_path_forensics -- --nocapture

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
#[cfg(feature = "v3-forensics")]
use std::sync::atomic::Ordering;

#[cfg(feature = "v3-forensics")]
const SMALL_NODES: usize = 1_000;
#[cfg(feature = "v3-forensics")]
const SMALL_EDGES: usize = 5_000;
#[cfg(feature = "v3-forensics")]
const MEDIUM_NODES: usize = 10_000;
#[cfg(feature = "v3-forensics")]
const MEDIUM_EDGES: usize = 50_000;

/// Helper to create a test database
#[cfg(feature = "v3-forensics")]
fn create_test_db(
    node_count: usize,
    edge_count: usize,
) -> Result<(tempfile::TempDir, std::path::PathBuf, i64), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let backend = V3Backend::create(&db_path)?;

    // Insert nodes
    for i in 0..node_count {
        backend.insert_node(NodeSpec {
            kind: format!("NodeKind{}", i % 10),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
    }

    // Insert random edges
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    for _ in 0..edge_count {
        let from = rng.gen_range(1..=node_count as i64);
        let to = rng.gen_range(1..=node_count as i64);
        backend.insert_edge(EdgeSpec {
            from,
            to,
            edge_type: format!("edge_{}", rng.gen_range(0..10)),
            data: serde_json::json!({}),
        })?;
    }

    backend.flush()?;

    // Target a node in the middle
    let target_id = (node_count / 2) as i64;

    Ok((temp_dir, db_path, target_id))
}

/// Print counter snapshot with phase label
#[cfg(feature = "v3-forensics")]
fn print_counters(phase: &str) {
    let btree_lookups = FORENSIC_COUNTERS.btree_lookup_calls.load(Ordering::Relaxed);
    let page_reads = FORENSIC_COUNTERS.page_read_count.load(Ordering::Relaxed);
    let node_decodes = FORENSIC_COUNTERS.node_decode_count.load(Ordering::Relaxed);

    let btree_hits = FORENSIC_COUNTERS
        .btree_cache_hit_count
        .load(Ordering::Relaxed);
    let btree_misses = FORENSIC_COUNTERS
        .btree_cache_miss_count
        .load(Ordering::Relaxed);
    let node_page_hits = FORENSIC_COUNTERS
        .node_page_cache_hit_count
        .load(Ordering::Relaxed);
    let node_page_misses = FORENSIC_COUNTERS
        .node_page_cache_miss_count
        .load(Ordering::Relaxed);

    let edge_hits = FORENSIC_COUNTERS
        .edge_cache_hit_count
        .load(Ordering::Relaxed);
    let edge_misses = FORENSIC_COUNTERS
        .edge_cache_miss_count
        .load(Ordering::Relaxed);
    let edge_page_reads = FORENSIC_COUNTERS
        .edge_page_read_count
        .load(Ordering::Relaxed);

    println!("\n  [{}] COUNTERS:", phase);
    println!("    B+Tree lookups:              {}", btree_lookups);
    println!("    Page reads (total):          {}", page_reads);
    println!("    Node decodes:                {}", node_decodes);
    println!(
        "    B+Tree cache:                {} hits/{} misses",
        btree_hits, btree_misses
    );
    println!(
        "    Node page cache:             {} hits/{} misses",
        node_page_hits, node_page_misses
    );
    println!(
        "    Edge cache:                  {} hits/{} misses",
        edge_hits, edge_misses
    );
    println!("    Edge page reads:             {}", edge_page_reads);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_cold_path_forensics_small() {
    println!("\n═══════════════════════════════════════════════════════════════════════");
    println!(
        "COLD-PATH FORENSICS: SMALL DATASET ({} nodes, {} edges)",
        SMALL_NODES, SMALL_EDGES
    );
    println!("═══════════════════════════════════════════════════════════════════════");

    let (_temp_dir, db_path, target_id) = create_test_db(SMALL_NODES, SMALL_EDGES).unwrap();

    // Reset counters
    FORENSIC_COUNTERS.reset();
    print_counters("INITIAL (after reset)");

    // PHASE 1: Pure open()
    let start = std::time::Instant::now();
    let backend = V3Backend::open(&db_path).unwrap();
    let open_duration = start.elapsed();
    print_counters("AFTER OPEN");
    println!("  => Open took: {:?}", open_duration);

    // PHASE 2: First get_node (cold)
    let start = std::time::Instant::now();
    let result1 = backend.get_node(SnapshotId::current(), target_id);
    let first_get_duration = start.elapsed();
    println!("  => First get_node result: {:?}", result1.is_ok());
    print_counters("AFTER FIRST get_node");
    println!("  => First get_node took: {:?}", first_get_duration);

    // PHASE 3: Second get_node (warm)
    let start = std::time::Instant::now();
    let result2 = backend.get_node(SnapshotId::current(), target_id);
    let second_get_duration = start.elapsed();
    println!("  => Second get_node result: {:?}", result2.is_ok());
    print_counters("AFTER SECOND get_node");
    println!("  => Second get_node took: {:?}", second_get_duration);

    // PHASE 4: First neighbors (cold)
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let start = std::time::Instant::now();
    let neighbors1 = backend.neighbors(SnapshotId::current(), target_id, query);
    let first_neighbors_duration = start.elapsed();
    println!(
        "  => First neighbors result: {:?} nodes",
        neighbors1.as_ref().map(|n| n.len()).ok()
    );
    print_counters("AFTER FIRST neighbors");
    println!("  => First neighbors took: {:?}", first_neighbors_duration);

    // PHASE 5: Second neighbors (warm)
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let start = std::time::Instant::now();
    let neighbors2 = backend.neighbors(SnapshotId::current(), target_id, query);
    let second_neighbors_duration = start.elapsed();
    println!(
        "  => Second neighbors result: {:?} nodes",
        neighbors2.as_ref().map(|n| n.len()).ok()
    );
    print_counters("AFTER SECOND neighbors");
    println!(
        "  => Second neighbors took: {:?}",
        second_neighbors_duration
    );

    // Summary
    println!("\n  ────────────────────────────────────────────────────────────────────");
    println!("  SUMMARY: Small Dataset");
    println!("  ────────────────────────────────────────────────────────────────────");
    println!("    Pure open():               {:>8.2?}", open_duration);
    println!(
        "    First get_node (cold):      {:>8.2?}",
        first_get_duration
    );
    println!(
        "    Second get_node (warm):     {:>8.2?}",
        second_get_duration
    );
    println!(
        "    First neighbors (cold):     {:>8.2?}",
        first_neighbors_duration
    );
    println!(
        "    Second neighbors (warm):    {:>8.2?}",
        second_neighbors_duration
    );
    println!(
        "    Cold/Warm get_node ratio:   {:.2}x",
        first_get_duration.as_secs_f64() / second_get_duration.as_secs_f64()
    );
    println!(
        "    Cold/Warm neighbors ratio:  {:.2}x",
        first_neighbors_duration.as_secs_f64() / second_neighbors_duration.as_secs_f64()
    );

    // Basic sanity checks
    assert!(result1.is_ok(), "First get_node should succeed");
    assert!(result2.is_ok(), "Second get_node should succeed");
    assert!(neighbors1.is_ok(), "First neighbors should succeed");
    assert!(neighbors2.is_ok(), "Second neighbors should succeed");
}

#[test]
#[cfg(feature = "v3-forensics")]
fn test_cold_path_forensics_medium() {
    println!("\n═══════════════════════════════════════════════════════════════════════");
    println!(
        "COLD-PATH FORENSICS: MEDIUM DATASET ({} nodes, {} edges)",
        MEDIUM_NODES, MEDIUM_EDGES
    );
    println!("═══════════════════════════════════════════════════════════════════════");

    let (_temp_dir, db_path, target_id) = create_test_db(MEDIUM_NODES, MEDIUM_EDGES).unwrap();

    // Reset counters
    FORENSIC_COUNTERS.reset();
    print_counters("INITIAL (after reset)");

    // PHASE 1: Pure open()
    let start = std::time::Instant::now();
    let backend = V3Backend::open(&db_path).unwrap();
    let open_duration = start.elapsed();
    print_counters("AFTER OPEN");
    println!("  => Open took: {:?}", open_duration);

    // PHASE 2: First get_node (cold)
    let start = std::time::Instant::now();
    let result1 = backend.get_node(SnapshotId::current(), target_id);
    let first_get_duration = start.elapsed();
    println!("  => First get_node result: {:?}", result1.is_ok());
    print_counters("AFTER FIRST get_node");
    println!("  => First get_node took: {:?}", first_get_duration);

    // PHASE 3: Second get_node (warm)
    let start = std::time::Instant::now();
    let result2 = backend.get_node(SnapshotId::current(), target_id);
    let second_get_duration = start.elapsed();
    println!("  => Second get_node result: {:?}", result2.is_ok());
    print_counters("AFTER SECOND get_node");
    println!("  => Second get_node took: {:?}", second_get_duration);

    // PHASE 4: First neighbors (cold)
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let start = std::time::Instant::now();
    let neighbors1 = backend.neighbors(SnapshotId::current(), target_id, query);
    let first_neighbors_duration = start.elapsed();
    println!(
        "  => First neighbors result: {:?} nodes",
        neighbors1.as_ref().map(|n| n.len()).ok()
    );
    print_counters("AFTER FIRST neighbors");
    println!("  => First neighbors took: {:?}", first_neighbors_duration);

    // PHASE 5: Second neighbors (warm)
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let start = std::time::Instant::now();
    let neighbors2 = backend.neighbors(SnapshotId::current(), target_id, query);
    let second_neighbors_duration = start.elapsed();
    println!(
        "  => Second neighbors result: {:?} nodes",
        neighbors2.as_ref().map(|n| n.len()).ok()
    );
    print_counters("AFTER SECOND neighbors");
    println!(
        "  => Second neighbors took: {:?}",
        second_neighbors_duration
    );

    // Summary
    println!("\n  ────────────────────────────────────────────────────────────────────");
    println!("  SUMMARY: Medium Dataset");
    println!("  ────────────────────────────────────────────────────────────────────");
    println!("    Pure open():               {:>8.2?}", open_duration);
    println!(
        "    First get_node (cold):      {:>8.2?}",
        first_get_duration
    );
    println!(
        "    Second get_node (warm):     {:>8.2?}",
        second_get_duration
    );
    println!(
        "    First neighbors (cold):     {:>8.2?}",
        first_neighbors_duration
    );
    println!(
        "    Second neighbors (warm):    {:>8.2?}",
        second_neighbors_duration
    );
    println!(
        "    Cold/Warm get_node ratio:   {:.2}x",
        first_get_duration.as_secs_f64() / second_get_duration.as_secs_f64()
    );
    println!(
        "    Cold/Warm neighbors ratio:  {:.2}x",
        first_neighbors_duration.as_secs_f64() / second_neighbors_duration.as_secs_f64()
    );

    // Basic sanity checks
    assert!(result1.is_ok(), "First get_node should succeed");
    assert!(result2.is_ok(), "Second get_node should succeed");
    assert!(neighbors1.is_ok(), "First neighbors should succeed");
    assert!(neighbors2.is_ok(), "Second neighbors should succeed");
}
