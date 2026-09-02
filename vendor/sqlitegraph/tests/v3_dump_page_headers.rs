//! Diagnostic test to dump page headers
//! Run with: cargo test --features native-v3 dump_headers -- --nocapture

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use std::fs::File;
use std::io::{Read, Seek};
use tempfile::TempDir;

#[test]
fn dump_page_headers() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_dump_test.db");

    println!("\n=== Creating database with 10 nodes ===\n");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 10 nodes
        for i in 0..10 {
            backend
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap();
        }

        println!("Flushing...");
        backend.flush().unwrap();
    }

    // Now read the file and dump first 20 page headers
    println!("\n=== Dumping first 20 page headers ===\n");

    let mut file = File::open(&db_path).unwrap();
    let mut buffer = vec![0u8; 4096];

    // Get file size
    let file_size = file.metadata().unwrap().len();
    println!("File size: {} bytes", file_size);

    // Skip V3 header (112 bytes)
    file.seek(std::io::SeekFrom::Start(112)).unwrap();

    for page_num in 1..=20 {
        let offset = 112 + (page_num - 1) * 4096;
        file.seek(std::io::SeekFrom::Start(offset)).unwrap();

        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read < 32 {
            println!("Page {}: only {} bytes (EOF)", page_num, bytes_read);
            break;
        }

        // Read header fields
        let page_id = u64::from_be_bytes(buffer[0..8].try_into().unwrap());
        let field_8_15 = u64::from_be_bytes(buffer[8..16].try_into().unwrap());
        let field_16_17 = u16::from_be_bytes(buffer[16..18].try_into().unwrap());
        let used_bytes = u16::from_be_bytes(buffer[18..20].try_into().unwrap());
        let field_20_27 = u64::from_be_bytes(buffer[20..28].try_into().unwrap());

        println!(
            "Page {} (offset {}): page_id={}, field8_15={}, field16_17={}, used_bytes={}, field20_27={}",
            page_num, offset, page_id, field_8_15, field_16_17, used_bytes, field_20_27
        );

        // Print first 32 bytes as hex
        print!("  First 32 bytes: ");
        for byte in buffer.iter().take(32) {
            print!("{:02x} ", byte);
        }
        println!();
    }

    // Try to reopen
    println!("\n=== Attempting to reopen ===\n");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("✓ Reopen succeeded"),
        Err(e) => println!("❌ Reopen failed: {}", e),
    }
}
