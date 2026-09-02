//! Focused get_node Cache Capacity Sweep Benchmark
//!
//! Tests the effect of page cache size on get_node performance
//! now that the unpacked_page_cache read-path bug is fixed.
//!
//! Run with:
//!   cargo run --example get_node_cache_sweep --release --features "native-v3,v3-forensics"
//!
//! This benchmark answers:
//! A. Is the current 16-page cache too small for stable get_node performance?
//! B. Does increasing cache size materially improve time per lookup?
//! C. Does cache hit rate increase with capacity?
//! D. Is there a plateau where bigger cache stops helping?

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::snapshot::SnapshotId;
use std::io::Write;
use std::time::Instant;

/// Cache sizes to sweep
const CACHE_SIZES: &[usize] = &[16, 32, 64, 128, 256];

/// Number of nodes in test database
const NODE_COUNT: usize = 10_000;

/// Number of get_node lookups per benchmark iteration
const LOOKUP_COUNT: usize = 100_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== get_node Cache Capacity Sweep Benchmark ===");
    println!(
        "Dataset: {} nodes, {} lookups per test",
        NODE_COUNT, LOOKUP_COUNT
    );
    println!();

    let mut results = Vec::new();

    for &cache_size in CACHE_SIZES {
        println!("{}", "=".repeat(70));
        println!("CACHE SIZE: {} pages", cache_size);
        println!("{}", "=".repeat(70));

        // Run the benchmark for this cache size
        match run_benchmark(cache_size) {
            Ok(stats) => {
                results.push((cache_size, stats));
                println!();
            }
            Err(e) => {
                println!("ERROR: {}", e);
                continue;
            }
        }
    }

    // Print summary table
    print_summary(&results);

    Ok(())
}

/// Benchmark statistics for a single cache size
#[derive(Debug, Clone)]
struct BenchmarkStats {
    warm_time_per_lookup_ns: f64,
    reopen_time_per_lookup_ns: f64,
    // Cache hit rates (from forensics)
    warm_cache_hits: u64,
    warm_cache_misses: u64,
    warm_hit_rate: f64,
    reopen_cache_hits: u64,
    reopen_cache_misses: u64,
    reopen_hit_rate: f64,
    // Page read stats
    warm_page_reads: u64,
    reopen_page_reads: u64,
}

fn run_benchmark(_cache_size: usize) -> Result<BenchmarkStats, Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache_sweep.db");

    // Step 1: Create database with nodes
    print!("Creating database... ");
    let _ = std::io::stdout().flush();
    let create_start = Instant::now();
    let backend = V3Backend::create(&db_path)?;

    for i in 0..NODE_COUNT {
        backend.insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i, "data": "x".repeat(32)}),
        })?;
    }
    backend.flush_to_disk()?;
    let create_time = create_start.elapsed();
    println!("{:.2}s", create_time.as_secs_f64());

    // Step 2: Warm cache benchmark (same instance, so cache is warm from inserts)
    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("Running warm cache benchmark... ");
    let _ = std::io::stdout().flush();
    let warm_stats = measure_get_node(&backend, LOOKUP_COUNT)?;
    println!(
        "{:.2}μs/lookup, {:.1} lookups/sec",
        warm_stats.time_per_lookup_ns / 1000.0,
        warm_stats.throughput
    );

    #[cfg(feature = "v3-forensics")]
    let warm_forensics = read_forensics();
    #[cfg(not(feature = "v3-forensics"))]
    let warm_forensics = ForensicsReading::default();

    // Step 3: Reopen benchmark (cache is cold)
    drop(backend);

    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    print!("Running reopen (cold cache) benchmark... ");
    let _ = std::io::stdout().flush();
    let backend = V3Backend::open(&db_path)?;
    let reopen_stats = measure_get_node(&backend, LOOKUP_COUNT)?;
    println!(
        "{:.2}μs/lookup, {:.1} lookups/sec",
        reopen_stats.time_per_lookup_ns / 1000.0,
        reopen_stats.throughput
    );

    #[cfg(feature = "v3-forensics")]
    let reopen_forensics = read_forensics();
    #[cfg(not(feature = "v3-forensics"))]
    let reopen_forensics = ForensicsReading::default();

    Ok(BenchmarkStats {
        warm_time_per_lookup_ns: warm_stats.time_per_lookup_ns,
        reopen_time_per_lookup_ns: reopen_stats.time_per_lookup_ns,
        warm_cache_hits: warm_forensics.node_page_cache_hit_count,
        warm_cache_misses: warm_forensics.node_page_cache_miss,
        warm_hit_rate: warm_forensics.node_page_cache_hit_rate(),
        reopen_cache_hits: reopen_forensics.node_page_cache_hit_count,
        reopen_cache_misses: reopen_forensics.node_page_cache_miss,
        reopen_hit_rate: reopen_forensics.node_page_cache_hit_rate(),
        warm_page_reads: warm_forensics.page_read_count,
        reopen_page_reads: reopen_forensics.page_read_count,
    })
}

/// Measurement results from a get_node benchmark run
#[derive(Debug, Clone)]
struct Measurement {
    time_per_lookup_ns: f64,
    throughput: f64,
}

/// Measure get_node performance
fn measure_get_node(
    backend: &V3Backend,
    count: usize,
) -> Result<Measurement, Box<dyn std::error::Error>> {
    let snapshot_id = SnapshotId::current();

    // Small warmup to stabilize measurements
    for i in 0..100 {
        let node_id = (i as i64) * 100 + 1;
        let _ = backend.get_node(snapshot_id, node_id);
    }

    #[cfg(feature = "v3-forensics")]
    reset_forensic_counters();

    // Measure main benchmark
    let start = Instant::now();
    for i in 0..count {
        // Cycle through first 1000 nodes (10k nodes, 50 per page = ~200 pages)
        let node_id = (i % 1000) as i64 * 10 + 1;
        let _ = backend.get_node(snapshot_id, node_id);
    }
    let duration = start.elapsed();

    let time_per_lookup_ns = duration.as_nanos() as f64 / count as f64;
    let throughput = count as f64 / duration.as_secs_f64();

    Ok(Measurement {
        time_per_lookup_ns,
        throughput,
    })
}

/// Forensic counter readings
#[derive(Debug, Clone, Default)]
struct ForensicsReading {
    node_page_cache_hit_count: u64,
    node_page_cache_miss: u64,
    page_read_count: u64,
}

impl ForensicsReading {
    fn node_page_cache_hit_rate(&self) -> f64 {
        let total = self.node_page_cache_hit_count + self.node_page_cache_miss;
        if total == 0 {
            0.0
        } else {
            self.node_page_cache_hit_count as f64 / total as f64
        }
    }
}

/// Read forensic counters
#[cfg(feature = "v3-forensics")]
fn read_forensics() -> ForensicsReading {
    use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
    ForensicsReading {
        node_page_cache_hit_count: FORENSIC_COUNTERS
            .node_page_cache_hit_count
            .load(std::sync::atomic::Ordering::Relaxed),
        node_page_cache_miss: FORENSIC_COUNTERS
            .node_page_cache_miss_count
            .load(std::sync::atomic::Ordering::Relaxed),
        page_read_count: FORENSIC_COUNTERS
            .page_read_count
            .load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// Reset forensic counters (only available with v3-forensics feature)
#[cfg(feature = "v3-forensics")]
fn reset_forensic_counters() {
    use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;
    FORENSIC_COUNTERS
        .node_page_cache_hit_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .node_page_cache_miss_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
    FORENSIC_COUNTERS
        .page_read_count
        .store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Print summary table of results
fn print_summary(results: &[(usize, BenchmarkStats)]) {
    println!();
    println!("{}", "=".repeat(80));
    println!("SUMMARY: Cache Capacity vs get_node Performance");
    println!("{}", "=".repeat(80));
    println!();

    // Table 1: Time per lookup
    println!("Time per lookup (lower is better):");
    println!("{:=<80}", "");
    println!(
        "{:>12} | {:>12} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Cache Size", "Warm (μs)", "Reopen (μs)", "Warm Hit %", "Reopen Hit %", "Speedup"
    );
    println!("{:-<80}", "-");

    let baseline_time = results
        .first()
        .map(|(_, s)| s.reopen_time_per_lookup_ns)
        .unwrap_or(1.0);

    for &(cache_size, ref stats) in results {
        let speedup = baseline_time / stats.reopen_time_per_lookup_ns;
        println!(
            "{:>12} | {:>12.2} | {:>12.2} | {:>11.1}% | {:>11.1}% | {:>12.2}x",
            cache_size,
            stats.warm_time_per_lookup_ns / 1000.0,
            stats.reopen_time_per_lookup_ns / 1000.0,
            stats.warm_hit_rate * 100.0,
            stats.reopen_hit_rate * 100.0,
            speedup
        );
    }

    println!();
    println!();

    // Table 2: Detailed cache statistics
    println!("Detailed Cache Statistics:");
    println!("{:=<80}", "");
    println!(
        "{:>12} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Cache Size", "Warm Hits", "Warm Misses", "Reopen Hits", "Reopen Misses"
    );
    println!("{:-<80}", "-");

    for &(cache_size, ref stats) in results {
        println!(
            "{:>12} | {:>12} | {:>12} | {:>12} | {:>12}",
            cache_size,
            stats.warm_cache_hits,
            stats.warm_cache_misses,
            stats.reopen_cache_hits,
            stats.reopen_cache_misses
        );
    }

    println!();
    println!();

    // Table 3: Page reads
    println!("Page Read Count (lower is better):");
    println!("{:=<80}", "");
    println!(
        "{:>12} | {:>12} | {:>12} | {:>12}",
        "Cache Size", "Warm", "Reopen", "Reduction"
    );
    println!("{:-<80}", "-");

    let baseline_reads = results
        .first()
        .map(|(_, s)| s.reopen_page_reads)
        .unwrap_or(1);

    for &(cache_size, ref stats) in results {
        let reduction = if baseline_reads > 0 {
            (baseline_reads as f64 - stats.reopen_page_reads as f64) / baseline_reads as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "{:>12} | {:>12} | {:>12} | {:>11.1}%",
            cache_size, stats.warm_page_reads, stats.reopen_page_reads, reduction
        );
    }

    println!();
    println!();

    // Analysis
    println!("ANALYSIS:");
    println!("{:-<80}", "-");

    // Find optimal cache size (lowest time per lookup = best)
    let best_speedup = results
        .iter()
        .min_by(|a, b| {
            a.1.reopen_time_per_lookup_ns
                .partial_cmp(&b.1.reopen_time_per_lookup_ns)
                .unwrap()
        })
        .unwrap();

    println!(
        "1. Best performance: {} pages ({:.2}μs/lookup, {:.1}% hit rate)",
        best_speedup.0,
        best_speedup.1.reopen_time_per_lookup_ns / 1000.0,
        best_speedup.1.reopen_hit_rate * 100.0
    );

    // Check for plateau
    let last_three = results.len().saturating_sub(3);
    if results.len() > 3 {
        let plateau = results[last_three..].iter().all(|(_, s)| {
            (s.reopen_time_per_lookup_ns - best_speedup.1.reopen_time_per_lookup_ns).abs()
                / best_speedup.1.reopen_time_per_lookup_ns
                < 0.05
        });

        if plateau {
            println!("2. Plateau detected: Last 3 cache sizes show <5% variance");
        } else {
            println!("2. No clear plateau: Performance still improving");
        }
    }

    // Compare 16 vs optimal
    let baseline_16 = results
        .iter()
        .find(|(s, _)| *s == 16)
        .map(|(_, s)| s.reopen_time_per_lookup_ns)
        .unwrap_or(0.0);

    if baseline_16 > 0.0 {
        let improvement =
            (baseline_16 - best_speedup.1.reopen_time_per_lookup_ns) / baseline_16 * 100.0;
        println!("3. Improvement from 16 pages: {:.1}% faster", improvement);
    }

    // Check hit rate saturation
    let best_hit_rate = results
        .iter()
        .map(|(_, s)| s.reopen_hit_rate)
        .fold(0.0_f64, f64::max);

    if best_hit_rate > 0.95 {
        println!(
            "4. Hit rate saturation: {:.1}% - cache is highly effective",
            best_hit_rate * 100.0
        );
    } else if best_hit_rate > 0.80 {
        println!(
            "4. Hit rate: {:.1}% - cache is working but not saturated",
            best_hit_rate * 100.0
        );
    } else {
        println!(
            "4. Hit rate: {:.1}% - significant thrashing remains",
            best_hit_rate * 100.0
        );
    }

    println!("{}", "=".repeat(80));
}
