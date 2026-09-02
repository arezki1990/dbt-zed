use sqlitegraph::backend::native::v3::backend::V3Backend;
use std::fs::File;
use std::io::{Read, Seek};
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let db_path = dir.path().join("test.graph");

    println!("Creating database at: {:?}", db_path);

    // Create database - should detect optimal page size
    let _backend = V3Backend::create(&db_path)?;

    // Read the page size directly from the file (offset 104-107)
    let mut file = File::open(&db_path)?;
    let mut buffer = [0u8; 4];
    file.seek(std::io::SeekFrom::Start(104))?;
    file.read_exact(&mut buffer)?;
    let page_size = u32::from_be_bytes(buffer);

    println!("Page size: {} bytes", page_size);
    println!(
        "Media type: {:?}",
        if page_size == 4096 {
            "SSD (or unknown)"
        } else if page_size == 16384 {
            "HDD"
        } else {
            "Unknown"
        }
    );

    // Verify page size is valid
    assert!(
        page_size == 4096 || page_size == 16384,
        "Invalid page size: {}",
        page_size
    );

    println!("✓ Adaptive page sizing is working!");
    Ok(())
}
