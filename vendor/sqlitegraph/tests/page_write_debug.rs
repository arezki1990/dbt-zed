//! Debug test to trace page writes

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn debug_page_writes() {
    println!("\n=== Debugging page writes ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_debug.graph");

    // Create database and insert nodes
    {
        println!("Creating database...");
        let backend = V3Backend::create(&db_path).unwrap();
        println!(
            "Initial file size: {}",
            std::fs::metadata(&db_path).unwrap().len()
        );

        // Insert a few nodes and check file size after each
        for i in 0..3000 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();

            if i % 100 == 0 {
                let file_size = std::fs::metadata(&db_path).unwrap().len();
                println!("After {} nodes: file size = {} bytes", i + 1, file_size);
            }
        }

        let final_size = std::fs::metadata(&db_path).unwrap().len();
        println!("\nFinal file size after 100 nodes: {} bytes", final_size);

        // Check which pages exist by trying to read them
        let page_size: u64 = 4096;
        let header_size: u64 = 112;
        let max_pages_to_check = (final_size / page_size) + 2;

        println!("\nChecking which pages are readable...");
        for page_id in 1..max_pages_to_check {
            let offset = header_size + (page_id - 1) * page_size;
            if offset + page_size <= final_size as u64 {
                // Try to read this page
                let mut file = std::fs::File::open(&db_path).unwrap();
                use std::io::{Read, Seek, SeekFrom};
                file.seek(SeekFrom::Start(offset)).unwrap();
                let mut buffer = vec![0u8; page_size as usize];
                match file.read_exact(&mut buffer) {
                    Ok(_) => {
                        // Check if page is all zeros (uninitialized)
                        let is_zero = buffer.iter().all(|&b| b == 0);
                        if !is_zero {
                            println!("  Page {}: contains data (offset = {})", page_id, offset);
                        }
                    }
                    Err(_) => {
                        println!("  Page {}: UNREADABLE at offset {}", page_id, offset);
                    }
                }
            }
        }
    }

    // Try to reopen
    println!("\nAttempting to reopen database...");
    match V3Backend::open(&db_path) {
        Ok(_) => {
            println!("SUCCESS: Database reopened!");
        }
        Err(e) => {
            println!("FAILED: {:?}", e);
        }
    }
}
