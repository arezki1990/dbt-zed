//! Cold Path Decomposition Benchmark
//!
//! Decomposes the V3 cold-start penalty into:
//! A. pure open() time
//! B. open() + first get_node time
//! C. open() + second get_node time
//! D. open() + first neighbors time
//! E. open() + second neighbors time
//!
//! Run with:
//!   cargo run --example cold_path_decomposition --release --features "native-v3,v3-forensics"

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::snapshot::SnapshotId;
use std::fs;
use std::io::Write;
use std::time::Instant;

const NODE_COUNT_SMALL: usize = 1_000;
const NODE_COUNT_MEDIUM: usize = 10_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== COLD PATH DECOMPOSITION BENCHMARK ===");
    println!();

    // Run for both small and medium datasets
    run_decomposition("small", NODE_COUNT_SMALL)?;
    run_decomposition("medium", NODE_COUNT_MEDIUM)?;

    Ok(())
}

fn run_decomposition(
    dataset_name: &str,
    node_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "=".repeat(70));
    println!("DATASET: {} ({} nodes)", dataset_name, node_count);
    println!("{}", "=".repeat(70));
    println!();

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cold_decomposition.db");

    // Step 1: Create database (not timed - this is setup)
    print!("Creating database... ");
    let _ = std::io::stdout().flush();
    let backend = V3Backend::create(&db_path)?;
    for i in 0..node_count {
        backend.insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i, "data": "x".repeat(32)}),
        })?;
    }
    backend.flush_to_disk()?;
    drop(backend);

    // Get file size for context
    let file_size = fs::metadata(&db_path)?.len();
    println!(
        "Done (file size: {:.2} MB)",
        file_size as f64 / 1024.0 / 1024.0
    );
    println!();

    // Reset forensic counters before measurements
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    // === A. Pure open() time ===
    print!("A. Pure open()... ");
    let _ = std::io::stdout().flush();
    let open_start = Instant::now();
    let backend = V3Backend::open(&db_path)?;
    let open_time = open_start.elapsed();
    println!("{:.2} ms", open_time.as_secs_f64() * 1000.0);

    #[cfg(feature = "v3-forensics")]
    let forensics_after_open = read_forensics();

    // === B. First get_node after open ===
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("B. First get_node (node {})... ", node_count / 2);
    let _ = std::io::stdout().flush();
    let first_get_start = Instant::now();
    let _ = backend.get_node(SnapshotId::current(), (node_count / 2) as i64);
    let first_get_time = first_get_start.elapsed();
    println!("{:.2} µs", first_get_time.as_secs_f64() * 1_000_000.0);

    #[cfg(feature = "v3-forensics")]
    let forensics_after_first_get = read_forensics();

    // === C. Second get_node (warm cache) ===
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("C. Second get_node (node {})... ", node_count / 3);
    let _ = std::io::stdout().flush();
    let second_get_start = Instant::now();
    let _ = backend.get_node(SnapshotId::current(), (node_count / 3) as i64);
    let second_get_time = second_get_start.elapsed();
    println!("{:.2} µs", second_get_time.as_secs_f64() * 1_000_000.0);

    #[cfg(feature = "v3-forensics")]
    let forensics_after_second_get = read_forensics();

    // === D. First neighbors after open ===
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("D. First neighbors (node {})... ", node_count / 2);
    let _ = std::io::stdout().flush();
    let first_neighbors_start = Instant::now();
    let query1 = sqlitegraph::backend::NeighborQuery {
        direction: sqlitegraph::backend::BackendDirection::Outgoing,
        edge_type: None,
    };
    let _ = backend.neighbors(SnapshotId::current(), (node_count / 2) as i64, query1);
    let first_neighbors_time = first_neighbors_start.elapsed();
    println!("{:.2} µs", first_neighbors_time.as_secs_f64() * 1_000_000.0);

    #[cfg(feature = "v3-forensics")]
    let _forensics_after_first_neighbors = read_forensics();

    // === E. Second neighbors (warm cache) ===
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("E. Second neighbors (node {})... ", node_count / 3);
    let _ = std::io::stdout().flush();
    let second_neighbors_start = Instant::now();
    let query2 = sqlitegraph::backend::NeighborQuery {
        direction: sqlitegraph::backend::BackendDirection::Outgoing,
        edge_type: None,
    };
    let _ = backend.neighbors(SnapshotId::current(), (node_count / 3) as i64, query2);
    let second_neighbors_time = second_neighbors_start.elapsed();
    println!(
        "{:.2} µs",
        second_neighbors_time.as_secs_f64() * 1_000_000.0
    );

    #[cfg(feature = "v3-forensics")]
    let _forensics_after_second_neighbors = read_forensics();

    // === Summary ===
    println!();
    println!("--- SUMMARY ---");
    println!("Operation                    | Time          | Speedup");
    println!("-----------------------------|---------------|----------");
    println!(
        "open()                       | {:>8.2} ms   | (baseline)",
        open_time.as_secs_f64() * 1000.0
    );
    println!(
        "first get_node               | {:>8.2} µs   | {:.1}x vs second",
        first_get_time.as_secs_f64() * 1_000_000.0,
        first_get_time.as_secs_f64() / second_get_time.as_secs_f64()
    );
    println!(
        "second get_node (warm)       | {:>8.2} µs   | (baseline)",
        second_get_time.as_secs_f64() * 1_000_000.0
    );
    println!(
        "first neighbors              | {:>8.2} µs   | {:.1}x vs second",
        first_neighbors_time.as_secs_f64() * 1_000_000.0,
        first_neighbors_time.as_secs_f64() / second_neighbors_time.as_secs_f64()
    );
    println!(
        "second neighbors (warm)      | {:>8.2} µs   | (baseline)",
        second_neighbors_time.as_secs_f64() * 1_000_000.0
    );

    // === Forensic details ===
    #[cfg(feature = "v3-forensics")]
    {
        println!();
        println!("--- FORENSIC COUNTERS AFTER OPEN ---");
        println!(
            "  btree_lookups:             {}",
            forensics_after_open.btree_lookup_calls
        );
        println!(
            "  page_reads:                 {}",
            forensics_after_open.page_read_count
        );
        println!(
            "  node_page_unpacks:          {}",
            forensics_after_open.node_decode_count
        );
        println!(
            "  node_cache_hits:            {}",
            forensics_after_open.node_page_cache_hit_count
        );
        println!(
            "  node_cache_misses:          {}",
            forensics_after_open.node_page_cache_miss_count
        );

        println!();
        println!("--- FORENSIC COUNTERS: FIRST GET_NODE ---");
        println!(
            "  btree_lookups:             {}",
            forensics_after_first_get.btree_lookup_calls
        );
        println!(
            "  page_reads:                 {}",
            forensics_after_first_get.page_read_count
        );
        println!(
            "  node_page_unpacks:          {}",
            forensics_after_first_get.node_decode_count
        );
        println!(
            "  node_cache_hits:            {}",
            forensics_after_first_get.node_page_cache_hit_count
        );
        println!(
            "  node_cache_misses:          {}",
            forensics_after_first_get.node_page_cache_miss_count
        );

        println!();
        println!("--- FORENSIC COUNTERS: SECOND GET_NODE (WARM) ---");
        println!(
            "  btree_lookups:             {}",
            forensics_after_second_get.btree_lookup_calls
        );
        println!(
            "  page_reads:                 {}",
            forensics_after_second_get.page_read_count
        );
        println!(
            "  node_page_unpacks:          {}",
            forensics_after_second_get.node_decode_count
        );
        println!(
            "  node_cache_hits:            {}",
            forensics_after_second_get.node_page_cache_hit_count
        );
        println!(
            "  node_cache_misses:          {}",
            forensics_after_second_get.node_page_cache_miss_count
        );
    }

    println!();
    Ok(())
}

/// Forensic counter readings
#[cfg(feature = "v3-forensics")]
#[derive(Debug, Clone, Default)]
struct ForensicsReading {
    btree_lookup_calls: u64,
    page_read_count: u64,
    node_decode_count: u64,
    node_page_cache_hit_count: u64,
    node_page_cache_miss_count: u64,
}

/// Read forensic counters
#[cfg(feature = "v3-forensics")]
fn read_forensics() -> ForensicsReading {
    use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
    ForensicsReading {
        btree_lookup_calls: FORENSIC_COUNTERS
            .btree_lookup_calls
            .load(std::sync::atomic::Ordering::Relaxed),
        page_read_count: FORENSIC_COUNTERS
            .page_read_count
            .load(std::sync::atomic::Ordering::Relaxed),
        node_decode_count: FORENSIC_COUNTERS
            .node_decode_count
            .load(std::sync::atomic::Ordering::Relaxed),
        node_page_cache_hit_count: FORENSIC_COUNTERS
            .node_page_cache_hit_count
            .load(std::sync::atomic::Ordering::Relaxed),
        node_page_cache_miss_count: FORENSIC_COUNTERS
            .node_page_cache_miss_count
            .load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// Reset forensic counters
#[cfg(feature = "v3-forensics")]
fn reset_forensic_counters() {
    use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
    FORENSIC_COUNTERS
        .btree_lookup_calls
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .page_read_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .node_decode_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .node_page_cache_hit_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .node_page_cache_miss_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
}
