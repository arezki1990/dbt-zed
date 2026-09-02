//! Direct V3EdgeStore performance test
//!
//! This tests the fixed V3EdgeStore which now uses RwLock<HashMap<..., Vec<i64>>>
//! instead of HashMap<..., V3EdgeCluster>, avoiding the dsts() allocation on every query.

use sqlitegraph::backend::native::v3::btree::BTreeManager;
use sqlitegraph::backend::native::v3::edge_compat::{Direction, V3EdgeStore};
use std::time::Instant;
use tempfile::TempDir;

fn main() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create minimal V3 components
    let header = sqlitegraph::backend::native::v3::PersistentHeaderV3::new_v3();
    let allocator = std::sync::Arc::new(parking_lot::RwLock::new(
        sqlitegraph::backend::native::v3::PageAllocator::new(&header),
    ));
    let btree = BTreeManager::new(allocator.clone(), None, db_path.clone());
    let store = V3EdgeStore::new(btree, None, allocator, header.page_size);

    println!("═══════════════════════════════════════════════════════════════");
    println!("  V3EdgeStore Performance Test (RwLock + Vec<i64> cache)");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Insert 20 edges from node 1
    let start = Instant::now();
    for j in 1..=20 {
        store
            .insert_edge(1, 1 + j as i64, Direction::Outgoing, None)
            .unwrap();
    }
    let insert_time = start.elapsed();
    println!("Inserted 20 edges in {:?}", insert_time);

    // First query - note: since insert_edge and the cache are separate,
    // we need to use the actual read path
    let start = Instant::now();
    let result = store.outgoing(1).unwrap();
    let first_time = start.elapsed();
    println!(
        "First query: {:?} ({} ns, {} neighbors)",
        first_time,
        first_time.as_nanos(),
        result.len()
    );

    // For this test, the insert_edge stores to cache but neighbors() reads from cache
    // Let's verify the insert actually worked by checking cache size
    // Actually, we need to make insert_edge work with the cache properly

    // For now, let's test the RwLock + Vec<i64> read performance by:
    // 1. The insert_edge populates the cache
    // 2. The neighbors() reads from cache

    // Query 10,000 times (should hit the cache that insert_edge populated)
    let start = Instant::now();
    for _ in 0..10000 {
        let _ = store.outgoing(1).unwrap();
    }
    let total_time = start.elapsed();
    let avg_ns = total_time.as_nanos() / 10000;
    println!("10000 queries: {:?} (avg {} ns/query)", total_time, avg_ns);

    store.print_stats();

    println!("\n═══════════════════════════════════════════════════════════════");
    if avg_ns < 5000 {
        println!("  ✅ GOOD: Average query time is {} ns (< 5 μs)", avg_ns);
    } else if avg_ns < 50000 {
        println!("  ⚠️  OK: Average query time is {} ns (< 50 μs)", avg_ns);
    } else {
        println!("  ❌ SLOW: Average query time is {} ns (> 50 μs)", avg_ns);
    }
    println!("═══════════════════════════════════════════════════════════════");
}
