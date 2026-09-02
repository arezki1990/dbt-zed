//! Test to verify FileCoordinator writes are persisting
//! Run with: cargo test --features native-v3 test_file_write -- --nocapture

use sqlitegraph::backend::native::v3::{FileCoordinator, PersistentHeaderV3};
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_file_coordinator_write() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_file_test.db");

    println!("\n=== Testing FileCoordinator direct writes ===\n");

    // Create header first
    let header = PersistentHeaderV3::new_v3();
    let header_bytes = header.to_bytes();
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&db_path)
            .unwrap();
        file.write_all(&header_bytes).unwrap();
        file.sync_all().unwrap();
    }
    println!("Wrote header ({} bytes)", header_bytes.len());

    // Create file coordinator
    let coordinator = FileCoordinator::create(&db_path).unwrap();
    println!("Created FileCoordinator");

    // Write a test page with identifiable pattern
    let mut test_page = [0u8; 4096];
    // Write page_id = 42 at offset 0-7
    test_page[0..8].copy_from_slice(&42u64.to_be_bytes());
    // Write a pattern at offset 18-19 (used_bytes field)
    test_page[18..20].copy_from_slice(&1234u16.to_be_bytes());
    // Write a pattern at offset 32 (data start)
    test_page[32] = 0xDE;
    test_page[33] = 0xAD;
    test_page[34] = 0xBE;
    test_page[35] = 0xEF;

    println!("Writing test page (page_id=42, used_bytes=1234)...");
    coordinator.write_page(2, &test_page).unwrap();

    println!("\n=== Verifying write by reading directly ===\n");

    // Read directly from file
    let mut file = std::fs::File::open(&db_path).unwrap();
    use std::io::{Read, Seek};

    // Skip header (112 bytes) and go to page 2
    let page_offset = 112 + 4096;
    file.seek(std::io::SeekFrom::Start(page_offset as u64))
        .unwrap();

    let mut buffer = vec![0u8; 4096];
    let bytes_read = file.read(&mut buffer).unwrap();
    println!("Read {} bytes from offset {}", bytes_read, page_offset);

    let page_id = u64::from_be_bytes(buffer[0..8].try_into().unwrap());
    let used_bytes = u16::from_be_bytes(buffer[18..20].try_into().unwrap());
    let pattern = u32::from_be_bytes(buffer[32..36].try_into().unwrap());

    println!(
        "Page data: page_id={}, used_bytes={}, pattern={:08x}",
        page_id, used_bytes, pattern
    );

    if page_id == 42 && used_bytes == 1234 && pattern == 0xDEADBEEF {
        println!("✓ FileCoordinator write verified!");
    } else {
        println!("❌ FileCoordinator write FAILED!");
    }
}
