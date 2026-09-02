//! Test to diagnose index restoration staleness check issue

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    println!("=== INDEX STALENESS DIAGNOSTIC TEST ===\n");

    // Step 1: Create database with 1000 nodes
    println!("Step 1: Creating database with 1000 nodes...");
    let backend = V3Backend::create(&db_path)?;
    for i in 0..1000 {
        backend.insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i}),
        })?;
    }

    // Check node_count BEFORE flush
    let header_before_flush = backend.header();
    let node_count_before_flush = header_before_flush.node_count;
    println!("  Node count before flush: {}", node_count_before_flush);

    backend.flush_to_disk()?;
    drop(backend);

    // Check index file
    let index_path = db_path.with_extension("v3index");
    println!("  Index file exists: {}", index_path.exists());
    if index_path.exists() {
        let metadata = fs::metadata(&index_path)?;
        println!("  Index file size: {} bytes", metadata.len());

        // Read the stored node count from index file
        let mut file = std::fs::File::open(&index_path)?;
        use std::io::Read;

        // Skip magic (4) and version (4)
        let mut skip = [0u8; 8];
        file.read_exact(&mut skip)?;

        // Read node count
        let mut node_count_bytes = [0u8; 8];
        file.read_exact(&mut node_count_bytes)?;
        let stored_node_count = u64::from_be_bytes(node_count_bytes);
        println!("  Stored node count in index: {}", stored_node_count);
    }

    // Step 2: Read header from DB file directly
    println!("\nStep 2: Reading DB header directly...");
    use std::io::Read;
    let mut db_file = std::fs::File::open(&db_path)?;
    let mut header_bytes = vec![0u8; sqlitegraph::backend::native::v3::V3_HEADER_SIZE as usize];
    db_file.read_exact(&mut header_bytes)?;

    // Parse header to get node_count field
    // node_count is at offset 16 (based on V3 header layout)
    let node_count_in_header = u64::from_be_bytes([
        header_bytes[16],
        header_bytes[17],
        header_bytes[18],
        header_bytes[19],
        header_bytes[20],
        header_bytes[21],
        header_bytes[22],
        header_bytes[23],
    ]);
    println!("  Node count in DB header: {}", node_count_in_header);

    // Step 3: Open backend and see what happens
    println!("\nStep 3: Opening backend (will try restore or rebuild)...");
    let backend = V3Backend::open(&db_path)?;
    let header_after_open = backend.header();
    let node_count_after_open = header_after_open.node_count;
    println!("  Node count after open: {}", node_count_after_open);

    // Test a query to verify indexes are working
    let node = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), 500);
    println!("  Query test (node 500): found={}", node.is_ok());

    Ok(())
}
