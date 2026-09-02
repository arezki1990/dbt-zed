//! Test Vec clone overhead

use std::time::Instant;

fn main() {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Vec Clone Overhead Test");
    println!("═══════════════════════════════════════════════════════════════\n");

    let data: Vec<i64> = (2..=21).collect();

    // Warmup
    for _ in 0..1000 {
        let _ = data.clone();
    }

    // Test clone of 20-element Vec<i64>
    let start = Instant::now();
    for _ in 0..10000 {
        let _ = data.clone();
    }
    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() / 10000;
    println!("Vec<i64> clone (20 elements): avg {} ns/query", avg_ns);

    // Test AtomicU64 fetch_add overhead
    use std::sync::atomic::{AtomicU64, Ordering};
    let counter = AtomicU64::new(0);

    let start = Instant::now();
    for _ in 0..10000 {
        counter.fetch_add(1, Ordering::Relaxed);
    }
    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() / 10000;
    println!("AtomicU64 fetch_add: avg {} ns/query", avg_ns);

    println!("\n═══════════════════════════════════════════════════════════════");
}
