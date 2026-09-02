//! Test 100k node batch insertion

use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use std::time::Instant;
use tempfile::TempDir;

fn main() {
    println!("Testing 100k node batch insertion...\n");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_100k.db");

    let backend = V3Backend::create(&db_path).unwrap();

    let start = Instant::now();
    {
        let mut batch = backend.begin_batch();
        for i in 0..100_000 {
            batch
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap();

            if i > 0 && i % 10000 == 0 {
                println!("  Inserted {} nodes...", i);
            }
        }
        batch.commit().unwrap();
    }

    let elapsed = start.elapsed();
    println!("\nInserted 100000 nodes in {:.2?}", elapsed);
    println!(
        "Throughput: {:.0} nodes/sec",
        100_000.0 / elapsed.as_secs_f64()
    );
    println!(
        "Per-node: {:.4}ms",
        elapsed.as_secs_f64() * 1000.0 / 100_000.0
    );

    println!("\n=== SUCCESS: 100k node batch test passed ===");
}
