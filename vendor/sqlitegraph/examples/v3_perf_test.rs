//! Quick V3 performance test
//!
//! Run with: cargo run --features native-v3 --example v3_perf_test

use sqlitegraph::{NodeSpec, backend::GraphBackend, backend::native::v3::V3Backend};
use std::time::Instant;

fn main() {
    let temp = tempfile::TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("test.db")).unwrap();

    let start = Instant::now();
    let count = 1000;

    for i in 0..count {
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }

    let elapsed = start.elapsed();
    let per_ms = count as f64 / elapsed.as_millis() as f64;

    println!("V3 Insert {} nodes in {:?}", count, elapsed);
    println!("Rate: {:.2} nodes/ms", per_ms);
    println!("Expected: 10-100x faster than before (was ~0.1-1 nodes/ms with syncs)");
}
