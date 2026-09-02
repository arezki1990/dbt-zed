//! Test to check which write code path is being used

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_check_write_path() {
    println!("\n=== Checking write code path ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_write_path.graph");

    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert nodes and check when file grows
        let mut last_size = std::fs::metadata(&db_path).unwrap().len();

        for i in 0..3000 {
            backend
                .insert_node(NodeSpec {
                    kind: "T".to_string(),
                    name: format!("n{}", i),
                    file_path: None,
                    data: serde_json::json!({"x": i}),
                })
                .unwrap();

            let new_size = std::fs::metadata(&db_path).unwrap().len();
            if new_size != last_size {
                if i % 10 == 0 {
                    println!(
                        "Node {}: file grew from {} to {} (+{})",
                        i,
                        last_size,
                        new_size,
                        new_size - last_size
                    );
                }
                last_size = new_size;
            }
        }

        println!("\nFinal file size: {}", last_size);

        // Check how many pages should exist
        let page_size = 4096;
        let header_size = 112;
        let expected_pages = ((last_size - header_size as u64) / page_size as u64) + 1;
        println!("Expected pages (excluding header): {}", expected_pages);
    }

    // Try reopen
    println!("\nReopening...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS!"),
        Err(e) => println!("FAILED: {:?}", e),
    }
}
