//! Test to verify NodePages are written correctly at their page_ids
//! Run with: cargo test --features native-v3 check_nodepages -- --nocapture

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use std::fs::File;
use std::io::{Read, Seek};
use tempfile::TempDir;

#[test]
fn check_nodepages_at_correct_offsets() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_check_test.db");

    println!("\n=== Creating database with 100 nodes ===\n");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 100 nodes
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i, "data": "test data here".repeat(5)}),
                })
                .unwrap();
        }

        println!("Flushing...");
        backend.flush().unwrap();
    }

    // Now read the file and check specific page offsets
    println!("\n=== Checking specific page offsets ===\n");

    let mut file = File::open(&db_path).unwrap();
    let file_size = file.metadata().unwrap().len();
    println!(
        "File size: {} bytes ({} pages)",
        file_size,
        (file_size - 112) / 4096 + 1
    );

    // Check pages that NodeStore wrote: 2, 4, 5, 8, etc.
    let node_store_pages = [2, 4, 5, 8]; // Expected first few NodeStore pages

    for page_id in node_store_pages {
        let offset = 112 + (page_id - 1) * 4096;

        if offset as usize > file_size as usize {
            println!("Page {}: offset {} beyond file size", page_id, offset);
            break;
        }

        file.seek(std::io::SeekFrom::Start(offset as u64)).unwrap();
        let mut buffer = vec![0u8; 64]; // Read first 64 bytes (header + some data)

        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read < 32 {
            println!("Page {}: only {} bytes (partial read)", page_id, bytes_read);
            continue;
        }

        let read_page_id = u64::from_be_bytes(buffer[0..8].try_into().unwrap());
        let next_page_id = u64::from_be_bytes(buffer[8..16].try_into().unwrap());
        let node_count = u16::from_be_bytes(buffer[16..18].try_into().unwrap());
        let used_bytes = u16::from_be_bytes(buffer[18..20].try_into().unwrap());
        let base_id = u64::from_be_bytes(buffer[20..28].try_into().unwrap());

        println!(
            "Page {} (offset {}): page_id={}, next_page_id={}, node_count={}, used_bytes={}, base_id={}",
            page_id, offset, read_page_id, next_page_id, node_count, used_bytes, base_id
        );

        // Check if this looks like a valid NodePage
        let is_valid = read_page_id == page_id as u64 && used_bytes > 0 && used_bytes < 4000;
        if is_valid {
            println!("  ✓ Valid NodePage");
        } else {
            println!("  ❌ NOT a valid NodePage!");
        }
    }

    // Try to reopen
    println!("\n=== Attempting to reopen ===\n");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("✓ Reopen succeeded"),
        Err(e) => println!("❌ Reopen failed: {}", e),
    }
}
