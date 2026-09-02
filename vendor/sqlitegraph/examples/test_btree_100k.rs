//! Minimal test to verify B+Tree handles 100k nodes without MIN_KEYS panic

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use tempfile::TempDir;

fn main() {
    println!("Testing B+Tree with 100k nodes (reproduces the panic scenario)...\n");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_btree_100k.db");

    let backend = V3Backend::create(&db_path).unwrap();

    println!("Inserting 100,000 nodes...");
    for i in 0..100_000 {
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();

        if i > 0 && i % 10_000 == 0 {
            println!("  Inserted {} nodes...", i);
        }
    }

    println!("\n✅ SUCCESS: Inserted 100,000 nodes without MIN_KEYS panic!");
    println!("The B+Tree split fix is working correctly.\n");
}
