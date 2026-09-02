//! Simple test for NodeStore batch mode

use parking_lot::RwLock;
use std::sync::Arc;
use tempfile::TempDir;

use sqlitegraph::backend::native::types::NodeFlags;
use sqlitegraph::backend::native::v3::node::NodeRecordV3;
use sqlitegraph::backend::native::v3::{
    NodeStore, PageAllocator, PersistentHeaderV3, btree::BTreeManager,
};

fn main() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create header
    let header = PersistentHeaderV3::new_v3();

    // Create store
    let mut store = NodeStore::new(&header, db_path.clone());

    // Initialize with btree and shared allocator
    let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
    let btree = BTreeManager::new(Arc::clone(&allocator), None, db_path.clone());
    store.initialize(btree, Arc::clone(&allocator), None);

    // Test 1: Insert without batch mode
    println!("Test 1: Immediate mode");
    let node1 = NodeRecordV3::new_inline(0, NodeFlags::empty(), 0, 0, vec![1, 2, 3], 0, 0, 0, 0);
    match store.insert_node(node1, None) {
        Ok(id) => println!("  Inserted node {}", id),
        Err(e) => println!("  Error: {}", e),
    }

    // Test 2: Insert with batch mode
    println!("\nTest 2: Batch mode");
    store.begin_batch();
    println!("  Batch mode enabled");

    let node2 = NodeRecordV3::new_inline(0, NodeFlags::empty(), 0, 0, vec![4, 5, 6], 0, 0, 0, 0);
    match store.insert_node(node2, None) {
        Ok(id) => {
            println!(
                "  Staged node {} (dirty_pages: {})",
                id,
                store.dirty_page_count()
            );

            // Try second insert
            let node3 =
                NodeRecordV3::new_inline(0, NodeFlags::empty(), 0, 0, vec![7, 8, 9], 0, 0, 0, 0);
            match store.insert_node(node3, None) {
                Ok(id2) => println!(
                    "  Staged node {} (dirty_pages: {})",
                    id2,
                    store.dirty_page_count()
                ),
                Err(e) => println!("  Second insert error: {}", e),
            }
        }
        Err(e) => println!("  First insert error: {}", e),
    }

    // Commit
    match store.commit_batch() {
        Ok(count) => println!("  Committed {} pages", count),
        Err(e) => println!("  Commit error: {}", e),
    }

    println!("\nDone!");
}
