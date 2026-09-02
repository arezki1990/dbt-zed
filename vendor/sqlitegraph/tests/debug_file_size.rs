//! Debug test to understand file size behavior
//!
//! Run with: cargo test --features native-v3 test_debug_file_size --test debug_file_size -- --nocapture

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_debug_file_size() {
    println!("\n=== Debug: File size during 10K node creation ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_debug.graph");

    // Create database
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert nodes and check file size after each 1000
        for i in 0..10000 {
            backend
                .insert_node(NodeSpec {
                    kind: "T".to_string(),
                    name: format!("n{}", i),
                    file_path: None,
                    data: serde_json::json!({"x": i}),
                })
                .unwrap();

            if i > 0 && i % 1000 == 0 {
                let metadata = std::fs::metadata(&db_path).unwrap();
                println!("After {} nodes: file size = {} bytes", i, metadata.len());
            }
        }

        // Final file size
        let metadata = std::fs::metadata(&db_path).unwrap();
        println!("\nFinal file size: {} bytes", metadata.len());

        // Calculate expected size for 10000 nodes
        // Each node page holds about 50-100 nodes (with compression)
        // So we'd expect roughly 100-200 node pages plus B+Tree pages
        let expected_pages = 10000 / 50; // Rough estimate
        println!("Expected minimum pages: {}", expected_pages);
    }
}
