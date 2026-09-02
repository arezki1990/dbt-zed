//! Test to isolate RwLock overhead

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;

fn main() {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  RwLock Overhead Test");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Test 1: Direct RwLock<HashMap> access (like V3EdgeStore.cache)
    {
        let cache = RwLock::new(HashMap::<i64, Vec<i64>>::new());
        {
            let mut c = cache.write().unwrap();
            c.insert(1, (2..=21).collect::<Vec<i64>>());
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let c = cache.read().unwrap();
            let _ = c.get(&1).cloned();
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() / 10000;
        println!("Direct RwLock<HashMap>: avg {} ns/query", avg_ns);
    }

    // Test 2: Nested RwLock (RwLock<RwLock<HashMap>>) - like V3Backend.edge_store
    {
        let inner = RwLock::new(HashMap::<i64, Vec<i64>>::new());
        let outer = RwLock::new(inner);

        {
            let inner_lock = outer.read().unwrap();
            let mut map = inner_lock.write().unwrap();
            map.insert(1, (2..=21).collect::<Vec<i64>>());
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let inner_lock = outer.read().unwrap(); // First lock
            let map = inner_lock.read().unwrap(); // Second lock
            let _ = map.get(&1).cloned();
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() / 10000;
        println!("Nested RwLock (read+read): avg {} ns/query", avg_ns);
    }

    // Test 3: Parking lot vs std RwLock
    {
        use parking_lot::RwLock as ParkingLotRwLock;
        let cache = ParkingLotRwLock::new(HashMap::<i64, Vec<i64>>::new());
        {
            let mut c = cache.write();
            c.insert(1, (2..=21).collect::<Vec<i64>>());
        }

        let start = Instant::now();
        for _ in 0..10000 {
            let c = cache.read();
            let _ = c.get(&1).cloned();
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() / 10000;
        println!("ParkingLot RwLock<HashMap>: avg {} ns/query", avg_ns);
    }

    println!("\n═══════════════════════════════════════════════════════════════");
}
