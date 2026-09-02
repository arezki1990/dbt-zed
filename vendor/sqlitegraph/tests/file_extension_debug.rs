//! Debug test to understand file extension behavior

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::io::{Seek, SeekFrom, Write};
use tempfile::TempDir;

#[test]
fn test_set_len_behavior() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_set_len.db");

    println!("\n=== Testing set_len() behavior ===\n");

    // Create initial file with small size
    {
        let mut file = File::create(&db_path).unwrap();
        file.write_all(b"HEADER").unwrap();
        file.sync_all().unwrap();
    }

    println!(
        "Initial file size: {}",
        std::fs::metadata(&db_path).unwrap().len()
    );

    // Try to extend file using set_len
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(false)
            .open(&db_path)
            .unwrap();

        let current_len = file.metadata().unwrap().len();
        println!("After open: file size = {}", current_len);

        let required_len = 8192;
        file.set_len(required_len).unwrap();
        println!(
            "After set_len({}): file size = {}",
            required_len,
            file.metadata().unwrap().len()
        );

        // Write to offset 4096
        file.seek(SeekFrom::Start(4096)).unwrap();
        file.write_all(b"DATA_AT_4096").unwrap();
        println!(
            "After write at 4096: file size = {}",
            file.metadata().unwrap().len()
        );

        file.sync_all().unwrap();
        println!("After sync: file size = {}", file.metadata().unwrap().len());
    }

    // Close and reopen to verify persistence
    {
        let file_size = std::fs::metadata(&db_path).unwrap().len();
        println!("\nAfter close and reopen: file size = {}", file_size);

        // Try to read back the data
        let mut file = File::open(&db_path).unwrap();
        file.seek(SeekFrom::Start(4096)).unwrap();
        let mut buffer = vec![0u8; 12];
        file.read_exact(&mut buffer).unwrap();
        println!("Data read back: {:?}", String::from_utf8_lossy(&buffer));
    }
}

#[test]
fn test_write_beyond_eof_without_set_len() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test_eof.db");

    println!("\n=== Testing write beyond EOF without set_len ===\n");

    // Create initial file
    {
        let mut file = File::create(&db_path).unwrap();
        file.write_all(b"HEADER").unwrap();
        file.sync_all().unwrap();
    }

    println!(
        "Initial file size: {}",
        std::fs::metadata(&db_path).unwrap().len()
    );

    // Try to write beyond EOF WITHOUT set_len
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(false)
            .open(&db_path)
            .unwrap();

        file.seek(SeekFrom::Start(10000)).unwrap();
        file.write_all(b"DATA_FAR_OUT").unwrap();
        println!(
            "After write at 10000: file size = {} (before sync)",
            file.metadata().unwrap().len()
        );

        file.sync_all().unwrap();
        println!(
            "After write at 10000: file size = {} (after sync)",
            file.metadata().unwrap().len()
        );
    }

    // Verify
    let file_size = std::fs::metadata(&db_path).unwrap().len();
    println!("\nFinal file size: {}", file_size);
}
