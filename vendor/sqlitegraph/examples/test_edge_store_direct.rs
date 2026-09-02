//! Direct access to V3EdgeStore internals to isolate lock/lookup overhead

use sqlitegraph::{
    EdgeSpec, NodeSpec,
    backend::GraphBackend,
    backend::native::v3::{V3Backend, edge_compat::Direction},
};
use std::time::Instant;
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== DIRECT V3 EDGE STORE PROFILING ===\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    // Create V3 backend directly
    let backend = V3Backend::create(&db_path)?;
    let mut node_ids = Vec::new();
    for i in 0..100 {
        let id = backend.insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;
        node_ids.push(id);
    }

    // Insert edges from node 0 to nodes 1-20
    for j in 1..=20 {
        backend.insert_edge(EdgeSpec {
            from: node_ids[0],
            to: node_ids[j as usize],
            edge_type: "test".to_string(),
            data: serde_json::Value::Null,
        })?;
    }

    // Drop backend to flush
    drop(backend);

    // Reopen to test cached behavior
    let backend = V3Backend::open(&db_path)?;

    println!("Graph created. Testing direct edge_store access...\n");

    // Get edge_store - this requires unsafe access or public API
    // For now, let's measure through the public API but with variations

    const ITERATIONS: usize = 10000;
    let src_node = node_ids[0];

    // Test 1: Full path through neighbors()
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = backend.fetch_outgoing(src_node)?;
        std::hint::black_box(());
    }
    let full_time = start.elapsed();
    let full_ns = full_time.as_nanos() as f64 / ITERATIONS as f64;
    println!("1. backend.outgoing():           {:.2} ns/query", full_ns);

    // Now let's try to understand the call stack by checking what outgoing() does
    println!("\n=== ANALYZING CALL STACK ===");
    println!("backend.outgoing() -> edge_store.read() -> edge_store.neighbors()");
    println!("Each level acquires a RwLock read lock...");

    // Create a simple test to measure RwLock overhead
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc as StdArc;

    let cache = RwLock::new(HashMap::<(i64, Direction), StdArc<[i64]>>::new());
    let key = (src_node, Direction::Outgoing);
    let test_value: StdArc<[i64]> = StdArc::from(
        vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
        ]
        .into_boxed_slice(),
    );

    // Pre-populate cache
    {
        let mut cache_write = cache.write();
        cache_write.insert(key, test_value.clone());
    }

    // Test 2: Just the HashMap lookup with lock
    let start = Instant::now();
    let mut found_count = 0;
    for _ in 0..ITERATIONS {
        let cache_read = cache.read();
        if cache_read.get(&key).is_some() {
            found_count += 1;
        }
    }
    let lock_hash_time = start.elapsed();
    let lock_hash_ns = lock_hash_time.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "\n2. RwLock read + HashMap lookup: {:.2} ns/query",
        lock_hash_ns
    );
    println!("   (found_count={})", found_count);

    // Test 3: Just HashMap lookup (no lock)
    let simple_map =
        std::sync::Mutex::new(std::collections::HashMap::from([(key, test_value.clone())]));
    let start = Instant::now();
    let mut found_count = 0;
    for _ in 0..ITERATIONS {
        let map = simple_map.lock().unwrap();
        if map.get(&key).is_some() {
            found_count += 1;
        }
    }
    let mutex_hash_time = start.elapsed();
    let mutex_hash_ns = mutex_hash_time.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "3. Mutex + Vec lookup:           {:.2} ns/query",
        mutex_hash_ns
    );
    println!("   (found_count={})", found_count);

    // Test 4: Arc clone (no lock)
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = test_value.clone();
        std::hint::black_box(());
    }
    let arc_clone_time = start.elapsed();
    let arc_ns = arc_clone_time.as_nanos() as f64 / ITERATIONS as f64;
    println!("4. Arc clone (20 el):            {:.2} ns/query", arc_ns);

    println!("\n=== COMPARISON ===");
    println!("Full backend.outgoing():  {:.2} ns", full_ns);
    println!("RwLock + HashMap lookup:    {:.2} ns", lock_hash_ns);
    println!("  Ratio: {:.1}x", full_ns / lock_hash_ns);

    Ok(())
}
