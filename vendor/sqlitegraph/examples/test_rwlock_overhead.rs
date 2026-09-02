//! Measure RwLock and HashMap overhead to understand neighbors() bottleneck

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc as StdArc;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== RWLOCK OVERHEAD ANALYSIS ===\n");

    // Simulate V3EdgeStore cache structure
    let cache = RwLock::new(HashMap::<i64, StdArc<[i64]>>::new());

    // Create test data (20 neighbors)
    let test_value: StdArc<[i64]> = StdArc::from((1..=20).collect::<Vec<i64>>().into_boxed_slice());

    const ITERATIONS: usize = 100000;

    // Test 1: RwLock read + HashMap lookup (simulating V3 cache hit)
    let start = Instant::now();
    let mut total = 0;
    for _ in 0..ITERATIONS {
        let cache_read = cache.read();
        if let Some(neighbors) = cache_read.get(&1) {
            total += neighbors.len();
        }
    }
    let time1 = start.elapsed();
    println!(
        "1. RwLock.read() + HashMap::get():     {:.2} ns/op",
        time1.as_nanos() as f64 / ITERATIONS as f64
    );
    println!("   (total={}, ensures not optimized away)", total);

    // Test 2: Just HashMap lookup (no RwLock)
    let simple_map = HashMap::from([(1i64, test_value.clone())]);
    let start = Instant::now();
    let mut total = 0;
    for _ in 0..ITERATIONS {
        if let Some(neighbors) = simple_map.get(&1) {
            total += neighbors.len();
        }
    }
    let time2 = start.elapsed();
    println!(
        "2. HashMap::get() (no lock):         {:.2} ns/op",
        time2.as_nanos() as f64 / ITERATIONS as f64
    );
    println!("   (total={}, ensures not optimized away)", total);

    // Test 3: Arc clone (no lock, no lookup)
    let start = Instant::now();
    let mut total = 0;
    for _ in 0..ITERATIONS {
        let cloned = test_value.clone();
        total += cloned.len();
    }
    let time3 = start.elapsed();
    println!(
        "3. Arc<[i64]>.clone():               {:.2} ns/op",
        time3.as_nanos() as f64 / ITERATIONS as f64
    );
    println!("   (total={}, ensures not optimized away)", total);

    // Test 4: Empty RwLock read (just the lock overhead)
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _guard = cache.read();
    }
    let time4 = start.elapsed();
    println!(
        "4. RwLock.read() (empty scope):     {:.2} ns/op",
        time4.as_nanos() as f64 / ITERATIONS as f64
    );

    println!("\n=== OVERHEAD BREAKDOWN ===");
    let lock_only_ns = time4.as_nanos() as f64 / ITERATIONS as f64;
    let hash_lookup_ns = time2.as_nanos() as f64 / ITERATIONS as f64;
    let full_ns = time1.as_nanos() as f64 / ITERATIONS as f64;

    println!(
        "RwLock overhead only:                {:.2} ns",
        lock_only_ns
    );
    println!(
        "HashMap lookup only:                 {:.2} ns",
        hash_lookup_ns
    );
    println!("Combined (RwLock + HashMap):         {:.2} ns", full_ns);

    let calculated = lock_only_ns + hash_lookup_ns;
    println!("Calculated (lock + lookup):         {:.2} ns", calculated);
    println!("Actual measured:                     {:.2} ns", full_ns);
    println!(
        "Extra overhead:                       {:.2} ns",
        full_ns - calculated
    );

    // Compare to V3 baseline
    println!("\n=== COMPARISON TO V3 BASELINE ===");
    println!("V3 neighbors() hot path:             ~27,600 ns/query");
    println!("Our RwLock+HashMap:                  {:.2} ns/op", full_ns);
    println!(
        "Ratio:                              {:.1}x",
        27600.0 / full_ns
    );

    Ok(())
}
