//! Forensic test to check header state before/after flush and reopen
//! Run with: cargo test --features native-v3 header_forensics -- --nocapture

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::header::PersistentHeaderV3;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use std::fs::File;
use std::io::Read;
use tempfile::TempDir;

#[test]
fn header_forensics_test() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3_header_forensics.db");

    println!("\n=== Phase 1: Create backend ===\n");

    // Create backend
    let backend = V3Backend::create(&db_path).unwrap();

    // Check initial header
    let header = backend.header();
    println!("Initial header:");
    println!("  node_count: {}", header.node_count);
    println!("  root_index_page: {}", header.root_index_page);
    println!("  btree_height: {}", header.btree_height);
    println!("  total_pages: {}", header.total_pages);

    println!("\n=== Phase 2: Insert one node ===\n");

    let node_id = backend
        .insert_node(NodeSpec {
            kind: "TEST_KIND".to_string(),
            name: "TEST_NODE".to_string(),
            file_path: None,
            data: serde_json::json!({"test": "data"}),
        })
        .unwrap();
    println!("Inserted node: {}", node_id);

    // Check header after insert (before flush)
    let header_after_insert = backend.header();
    println!("\nHeader after insert (before flush):");
    println!("  node_count: {}", header_after_insert.node_count);
    println!("  root_index_page: {}", header_after_insert.root_index_page);
    println!("  btree_height: {}", header_after_insert.btree_height);
    println!("  total_pages: {}", header_after_insert.total_pages);

    println!("\n=== Phase 3: Flush ===\n");

    backend.flush().unwrap();
    println!("Flush complete");

    // Check header after flush
    let header_after_flush = backend.header();
    println!("\nHeader after flush:");
    println!("  node_count: {}", header_after_flush.node_count);
    println!("  root_index_page: {}", header_after_flush.root_index_page);
    println!("  btree_height: {}", header_after_flush.btree_height);
    println!("  total_pages: {}", header_after_flush.total_pages);

    println!("\n=== Phase 4: Read header directly from file ===\n");

    let mut file = File::open(&db_path).unwrap();
    let mut header_bytes = vec![0u8; 112];
    file.read_exact(&mut header_bytes).unwrap();
    let file_header = PersistentHeaderV3::from_bytes(&header_bytes).unwrap();
    println!("Header from file:");
    println!("  node_count: {}", file_header.node_count);
    println!("  root_index_page: {}", file_header.root_index_page);
    println!("  btree_height: {}", file_header.btree_height);
    println!("  total_pages: {}", file_header.total_pages);

    println!("\n=== Phase 5: Reopen ===\n");

    drop(backend);
    let backend2 = V3Backend::open(&db_path).unwrap();

    let reopened_header = backend2.header();
    println!("Header after reopen:");
    println!("  node_count: {}", reopened_header.node_count);
    println!("  root_index_page: {}", reopened_header.root_index_page);
    println!("  btree_height: {}", reopened_header.btree_height);
    println!("  total_pages: {}", reopened_header.total_pages);

    println!("\n=== Phase 6: Try to read node ===\n");

    use sqlitegraph::snapshot::SnapshotId;
    match backend2.get_node(SnapshotId::current(), node_id) {
        Ok(node) => {
            println!("✓ Node found: kind={}, name={}", node.kind, node.name);
        }
        Err(e) => {
            println!("❌ Error reading node: {:?}", e);
        }
    }

    // Verify expectations
    assert_eq!(
        file_header.node_count, 1,
        "File header should have node_count=1"
    );
    assert!(
        file_header.root_index_page > 0,
        "File header should have non-zero root_index_page"
    );

    println!("\n✓ All header checks passed!");
}
