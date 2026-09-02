//! Direct V3EdgeStore Performance Test (Raw Cache Performance)
//!
//! This test bypasses the Graph API and B+tree layers to measure
//! the raw V3EdgeStore cache performance.
//!
//! KEY FINDING: V3EdgeStore uses RwLock<HashMap<..., Arc<[i64]>>>
//! The Arc::clone() makes neighbor queries zero-copy after cache hit.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  Direct V3EdgeStore Cache Performance (Simulated)");
    println!("═══════════════════════════════════════════════════════════════════\n");

    // Test 1: Raw HashMap lookup (no locking)
    {
        let cache: HashMap<(i64, u8), Arc<[i64]>> = [(
            (1, 0),
            Arc::from((2..=21).collect::<Vec<i64>>().into_boxed_slice()),
        )]
        .into_iter()
        .collect();

        let start = Instant::now();
        for _ in 0..100000 {
            if let Some(neighbors) = cache.get(&(1, 0)) {
                let _ = neighbors.clone(); // Arc::clone
            }
        }
        let avg_ns = start.elapsed().as_nanos() / 100000;
        println!(
            "1. Raw HashMap (no lock):     {:>6} ns/query  (baseline)",
            avg_ns
        );
    }

    // Test 2: RwLock read guard + HashMap lookup
    {
        let cache = RwLock::new(HashMap::<(i64, u8), Arc<[i64]>>::from([(
            (1, 0),
            Arc::from((2..=21).collect::<Vec<i64>>().into_boxed_slice()),
        )]));

        // Warmup
        for _ in 0..1000 {
            let c = cache.read();
            if let Some(n) = c.get(&(1, 0)) {
                let _ = n.clone();
            }
        }

        let start = Instant::now();
        for _ in 0..100000 {
            let c = cache.read(); // RwLock read guard
            if let Some(neighbors) = c.get(&(1, 0)) {
                let _ = neighbors.clone(); // Arc::clone
            }
        }
        let avg_ns = start.elapsed().as_nanos() / 100000;
        println!(
            "2. RwLock<HashMap> (hot):     {:>6} ns/query  (V3EdgeStore cache)",
            avg_ns
        );
    }

    // Test 3: Double RwLock (V3Backend.edge_store pattern)
    {
        let inner = RwLock::new(HashMap::<(i64, u8), Arc<[i64]>>::from([(
            (1, 0),
            Arc::from((2..=21).collect::<Vec<i64>>().into_boxed_slice()),
        )]));
        let outer = RwLock::new(inner); // V3Backend.edge_store: RwLock<V3EdgeStore>

        // Warmup
        for _ in 0..1000 {
            let store = outer.read();
            let c = store.read();
            if let Some(n) = c.get(&(1, 0)) {
                let _ = n.clone();
            }
        }

        let start = Instant::now();
        for _ in 0..100000 {
            let store = outer.read(); // Outer RwLock (V3Backend.edge_store)
            let c = store.read(); // Inner RwLock (V3EdgeStore.cache)
            if let Some(neighbors) = c.get(&(1, 0)) {
                let _ = neighbors.clone(); // Arc::clone
            }
        }
        let avg_ns = start.elapsed().as_nanos() / 100000;
        println!(
            "3. RwLock<RwLock<HashMap>>:   {:>6} ns/query  (V3Backend full path)",
            avg_ns
        );
    }

    // Test 4: Compare with Vec clone (what old code did)
    {
        let cache = RwLock::new(HashMap::<(i64, u8), Vec<i64>>::from([(
            (1, 0),
            (2..=21).collect::<Vec<i64>>(),
        )]));

        let start = Instant::now();
        for _ in 0..100000 {
            let c = cache.read();
            if let Some(neighbors) = c.get(&(1, 0)) {
                let _ = neighbors.clone(); // Vec::clone (copies all 20 i64s)
            }
        }
        let avg_ns = start.elapsed().as_nanos() / 100000;
        println!(
            "4. Vec::clone (old way):      {:>6} ns/query  (alloc + copy)",
            avg_ns
        );
    }

    println!("\n═══════════════════════════════════════════════════════════════════");
    println!("  INTERPRETATION");
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  • Raw HashMap:       Baseline - memory read speed");
    println!("  • Single RwLock:     + parking_lot overhead (~20-30ns)");
    println!("  • Double RwLock:     + second lock acquisition (~50-100ns)");
    println!("  • Arc::clone:        Single atomic increment (~2-5ns)");
    println!("  • Vec::clone:        Heap alloc + memcpy of all elements");
    println!();
    println!("  The '70× faster' in benchmark report assumes:");
    println!("    - Cache is HOT (no B+tree lookup)");
    println!("    - Using Arc<[i64]> (not Vec<i64>)");
    println!("    - Minimal lock contention");
    println!("═══════════════════════════════════════════════════════════════════");
}
