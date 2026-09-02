//! Debug test to find the exact page 42 issue

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use std::io::{Read, Seek, SeekFrom};
use tempfile::TempDir;

#[test]
fn debug_page_42_issue() {
    println!("\n=== Debugging page 42 issue ===\n");

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_page_42.graph");

    // Create database and insert nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert nodes one by one and check file size
        for i in 0..2500 {
            match backend.insert_node(NodeSpec {
                kind: "T".to_string(),
                name: format!("n{}", i),
                file_path: None,
                data: serde_json::json!({"x": i}),
            }) {
                Ok(_) => {}
                Err(e) => {
                    println!("FAILED at node {}: {:?}", i, e);
                    println!(
                        "File size at failure: {}",
                        std::fs::metadata(&db_path).unwrap().len()
                    );

                    // Check what page we're trying to read
                    let page_size: u64 = 4096;
                    let header_size: u64 = 112;
                    for page_id in 35..45 {
                        let offset = header_size + (page_id - 1) * page_size;
                        let file_size = std::fs::metadata(&db_path).unwrap().len() as u64;
                        println!(
                            "  Page {}: offset={}, file_size={}, readable={}",
                            page_id,
                            offset,
                            file_size,
                            file_size >= offset + page_size
                        );
                    }
                    panic!("Failed at node {}", i);
                }
            }

            // Check file size at intervals
            if i % 100 == 99 {
                let file_size = std::fs::metadata(&db_path).unwrap().len();
                println!("After {} nodes: file size = {}", i + 1, file_size);
            }
        }

        println!("\nSuccessfully inserted 2500 nodes!");
        let final_size = std::fs::metadata(&db_path).unwrap().len();
        println!("Final file size: {}", final_size);

        // Check which pages are actually readable
        let page_size: usize = 4096;
        let header_size: usize = 112;
        let max_page = (final_size / page_size as u64) as usize + 5;

        println!("\nChecking page readability:");
        let mut file = std::fs::File::open(&db_path).unwrap();
        for page_id in 1..max_page {
            let offset = header_size + (page_id - 1) * page_size;
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut buffer = vec![0u8; page_size];
            match file.read_exact(&mut buffer) {
                Ok(_) => {
                    let is_zero = buffer.iter().all(|&b| b == 0);
                    if !is_zero {
                        println!("  Page {}: READABLE (offset {})", page_id, offset);
                    } else {
                        println!("  Page {}: ALL ZEROS (offset {})", page_id, offset);
                    }
                }
                Err(_) => {
                    println!("  Page {}: UNREADABLE (offset {})", page_id, offset);
                }
            }
        }
    }

    // Try to reopen
    println!("\nAttempting to reopen database...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS: Database reopened!"),
        Err(e) => println!("FAILED: {:?}", e),
    }
}
