//! Debug test to understand the overflow
//!
//! Run with: cargo test --features native-v3 debug_overflow -- --nocapture

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use tempfile::TempDir;

#[test]
fn debug_overflow() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("debug_overflow.db");

    println!("\n=== Debug Overflow Test ===\n");

    let backend = V3Backend::create(&db_path).unwrap();

    // Try to insert exactly the nodes that would cause overflow
    // Based on the error: data 4065 > 4096 - 32 = 4064 (USABLE_SIZE)
    // So we're 1 byte over

    let mut last_id = 0;
    for batch in 0..30 {
        println!("\nBatch {}:", batch);

        for i in 0..100 {
            let idx = batch * 100 + i;
            let kind = format!("Kind{}", idx % 10);
            let name = format!("process_data_{}", idx);
            let data = serde_json::json!({
                "index": idx,
                "payload": "x".repeat(40),
            });

            match backend.insert_node(sqlitegraph::backend::NodeSpec {
                kind: kind.clone(),
                name: name.clone(),
                file_path: None,
                data: data.clone(),
            }) {
                Ok(id) => {
                    last_id = id;
                    if idx % 500 == 0 {
                        println!("  Node {} inserted (id={})", idx, id);
                    }
                }
                Err(e) => {
                    println!("  ❌ FAILED at node {}: {:?}", idx, e);
                    println!("  Last successful node was {}", last_id);
                    panic!("Failed at node {}", idx);
                }
            }
        }

        // Flush every 100 nodes
        println!("  Flushing...");
        backend.flush().unwrap();
    }

    println!("\n✓ All {} nodes inserted successfully", last_id);

    // Try to reopen
    drop(backend);
    println!("Reopening database...");
    let _backend2 = V3Backend::open(&db_path).unwrap();
    println!("✓ Database reopened successfully");

    println!("\n✓ Debug test passed!");
}
