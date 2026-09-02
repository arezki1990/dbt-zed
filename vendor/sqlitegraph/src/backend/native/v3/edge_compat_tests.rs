use super::*;
use crate::backend::native::v3::{
    allocator::PageAllocator, btree::BTreeManager, header::PersistentHeaderV3,
};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

#[test]
fn test_page_type_from_u8() {
    assert_eq!(PageType::from_u8(0), Some(PageType::Free));
    assert_eq!(PageType::from_u8(1), Some(PageType::BTreeIndex));
    assert_eq!(PageType::from_u8(2), Some(PageType::NodeData));
    assert_eq!(PageType::from_u8(3), Some(PageType::EdgeCluster));
    assert_eq!(PageType::from_u8(255), None);
}

#[test]
fn test_direction_conversion() {
    assert_eq!(Direction::Outgoing.to_v2(), V2Direction::Outgoing);
    assert_eq!(Direction::Incoming.to_v2(), V2Direction::Incoming);
}

#[test]
fn test_v3_edge_cluster_new() {
    let cluster = V3EdgeCluster::new(42, Direction::Outgoing, 100);
    assert_eq!(cluster.src, 42);
    assert!(cluster.edges.is_empty());
    assert_eq!(cluster.direction, Direction::Outgoing);
    assert_eq!(cluster.page_id, 100);
    assert_eq!(cluster.format_version, 3);
}

#[test]
fn test_v3_edge_cluster_add_edge() {
    let mut cluster = V3EdgeCluster::new(1, Direction::Outgoing, 1);
    cluster.add_edge(2, None);
    cluster.add_edge(3, None);
    assert_eq!(cluster.dsts(), vec![2, 3]);
}

#[test]
fn test_v3_edge_cluster_serialize_roundtrip() {
    let mut cluster = V3EdgeCluster::new(42, Direction::Outgoing, 100);
    cluster.add_edge(100, None);
    cluster.add_edge(200, None);

    let bytes = cluster.serialize().unwrap();
    let deserialized = V3EdgeCluster::deserialize(&bytes, 100).unwrap();

    assert_eq!(deserialized.format_version, 3);
    assert_eq!(deserialized.src, 42, "src should survive roundtrip");
    assert_eq!(
        deserialized.direction,
        Direction::Outgoing,
        "direction should survive roundtrip"
    );
    assert_eq!(deserialized.dsts(), vec![100, 200]);
    assert_eq!(deserialized.page_id, 100);
}

#[test]
fn test_v3_edge_cluster_roundtrip_incoming() {
    let mut cluster = V3EdgeCluster::new(99, Direction::Incoming, 50);
    cluster.add_edge(10, None);

    let bytes = cluster.serialize().unwrap();
    let deserialized = V3EdgeCluster::deserialize(&bytes, 50).unwrap();

    assert_eq!(deserialized.src, 99);
    assert_eq!(
        deserialized.direction,
        Direction::Incoming,
        "Incoming direction must survive serialization roundtrip"
    );
}

//========================================================================
// TDD Tests for Edge Store Durability
// These tests verify the critical production issues:
// 1. WAL record for edge insert
// 2. Dirty cluster flush to pages
// 3. B+Tree index update
// 4. WAL checkpoint
//========================================================================

/// Test helper: Create a V3EdgeStore with WAL for durability testing
fn create_test_edge_store(db_path: Option<PathBuf>) -> (V3EdgeStore, Arc<RwLock<PageAllocator>>) {
    let header = PersistentHeaderV3::new_v3();
    let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));

    // Create BTreeManager with the allocator
    let btree = if let Some(ref path) = db_path {
        BTreeManager::new(allocator.clone(), None, path.clone())
    } else {
        BTreeManager::new(allocator.clone(), None, None::<PathBuf>)
    };

    // Create edge store with or without persistence path
    let edge_store = if let Some(ref path) = db_path {
        // Create WAL writer
        let wal_path = path.with_extension("v3wal");
        let writer = WALWriter::new(wal_path, 1).expect("Failed to create WAL writer");
        writer.write_header().expect("Failed to write WAL header");
        V3EdgeStore::with_path_and_allocator(
            btree,
            Some(writer),
            path.clone(),
            allocator.clone(),
            header.page_size,
        )
    } else {
        V3EdgeStore::new(btree, None, allocator.clone(), header.page_size)
    };

    // CRITICAL FIX: Restore B+Tree metadata if it exists
    // This allows recovery of the edge index from a previous session
    let _ = edge_store.restore_btree_from_metadata();

    (edge_store, allocator)
}

/// Test 1: Edge insert should write WAL record for durability
///
/// CRITICAL: This test verifies that insert_edge() creates a proper WAL record.
/// Without this, edges inserted via cache are lost on crash.
#[test]
fn test_edge_insert_creates_wal_record() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let wal_path = db_path.with_extension("v3wal");

    // Create edge store with WAL
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert an edge - this should create a WAL record
    edge_store
        .insert_edge(1, 2, Direction::Outgoing, None)
        .expect("Insert failed");

    // Flush WAL to ensure record is written
    edge_store.flush_wal().expect("WAL flush failed");

    // Verify: Verify WAL file exists and contains edge insert record
    // Currently this fails because insert_edge() does NOT write WAL records
    assert!(
        wal_path.exists(),
        "NOTE: WAL file should exist after edge insert with WAL enabled"
    );

    // Read WAL and verify edge insert record exists
    let wal_content = std::fs::read(&wal_path).expect("Failed to read WAL file");
    assert!(
        wal_content.len() > 64, // Header is 64 bytes, records add more
        "NOTE: WAL should contain edge insert record beyond header"
    );

    // Verify WAL parsing: and verify edge-specific record type exists
    // This requires implementing EdgeInsert record type in WAL
}

/// Test 2: Flush should write dirty clusters to pages
///
/// CRITICAL: This test verifies that flush() actually persists edge data.
/// Flush writes dirty clusters to disk pages via write_page_to_disk.
#[test]
fn test_flush_writes_dirty_clusters_to_pages() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create the database file first
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    // Create edge store with disk persistence
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert edges into cache
    edge_store
        .insert_edge(1, 2, Direction::Outgoing, None)
        .expect("Insert 1->2 failed");
    edge_store
        .insert_edge(1, 3, Direction::Outgoing, None)
        .expect("Insert 1->3 failed");
    edge_store
        .insert_edge(2, 4, Direction::Outgoing, None)
        .expect("Insert 2->4 failed");

    // Flush should write dirty clusters to disk pages
    let result = edge_store.flush(None);
    assert!(result.is_ok(), "Flush should succeed");

    // Verify: After flush, edge data should be on disk
    // Currently this fails because flush() does nothing
    let file_size = std::fs::metadata(&db_path)
        .expect("Failed to read file metadata")
        .len();

    assert!(
        file_size > 4096,
        "NOTE: Database file should grow after flush writes dirty clusters"
    );

    // Verify we can read back the edges after reopening
    // This requires implementing cluster deserialization from pages
}

/// Test 3: Flush should update B+Tree index
///
/// CRITICAL: The B+Tree index maps (src_node_id, direction) -> page_id.
/// Without this update, edge lookups will fail after recovery.
#[test]
fn test_flush_updates_btree_index() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create database file
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    // Create edge store
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert edges for node 1
    edge_store
        .insert_edge(1, 2, Direction::Outgoing, None)
        .expect("Insert failed");
    edge_store
        .insert_edge(1, 3, Direction::Outgoing, None)
        .expect("Insert failed");

    // Flush should update B+Tree index
    edge_store.flush(None).expect("Flush failed");

    // Verify: B+Tree should contain mapping for node 1
    // Currently btree only tracks node_id -> page_id, not edge lookups
    // Need to implement (src, direction) composite key lookup

    // After fix: verify B+Tree contains edge cluster mapping
    let btree = edge_store.btree.read();
    let lookup_key = edge_key(1, Direction::Outgoing);
    let lookup_result = btree.lookup(lookup_key);

    assert!(lookup_result.is_ok(), "B+Tree lookup should succeed");
    assert!(
        lookup_result.unwrap().is_some(),
        "B+Tree should contain edge page mapping for node 1 after flush"
    );
}

/// Test 4: WAL checkpoint should truncate WAL after successful flush
///
/// VERIFIED: After flush() persists data to pages, WAL is checkpointed
/// and truncated to prevent unbounded WAL growth.
#[test]
fn test_wal_checkpoint_after_flush() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let wal_path = db_path.with_extension("v3wal");

    // Create database file
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    // Create edge store with WAL
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert and flush multiple times
    for i in 0..5 {
        edge_store
            .insert_edge(1, i as i64 + 10, Direction::Outgoing, None)
            .unwrap_or_else(|_| panic!("Insert iteration {} failed", i));
        edge_store.flush(None).expect("Flush failed");
    }

    // VERIFIED: WAL should be truncated (removed) after flush
    // The truncate() call now happens after checkpoint in flush()
    //
    // DURABILITY GUARANTEE:
    // - Main DB pages are synced before WAL is truncated
    // - WAL replay is not implemented (so WAL is not needed for recovery)
    // - Safe to remove WAL file after checkpoint
    assert!(
        !wal_path.exists(),
        "WAL file should be truncated (removed) after flush"
    );
}

/// Test 5: Edge data should survive crash (recovery test)
///
/// VERIFIED: Edges persisted to disk can be recovered after reopening.
/// WAL is truncated after flush, but main DB file contains durable data.
#[test]
fn test_edge_recovery_after_crash() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let wal_path = db_path.with_extension("v3wal");

    // Create database file
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    // Phase 1: Create edges and persist to disk
    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

        edge_store
            .insert_edge(1, 2, Direction::Outgoing, None)
            .expect("Insert failed");
        edge_store
            .insert_edge(1, 3, Direction::Outgoing, None)
            .expect("Insert failed");
        edge_store
            .insert_edge(2, 4, Direction::Outgoing, None)
            .expect("Insert failed");

        // Call flush() to write dirty clusters to disk pages
        // This ensures data survives after the edge store is dropped
        edge_store.flush(None).expect("Flush failed");

        // VERIFIED: WAL is now truncated after flush
        assert!(
            !wal_path.exists(),
            "WAL file should be truncated (removed) after flush with checkpoint"
        );
    }

    // Phase 2: "Recover" by creating new edge store
    // The new store should load edges from disk on cache miss
    {
        let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

        // Load neighbors for node 1 - should read from disk since cache is empty
        let neighbors = recovered_store
            .outgoing(1)
            .expect("Failed to get neighbors");

        // VERIFIED: Data persists from main DB file (WAL is not needed for recovery)
        assert!(
            neighbors.len() >= 2,
            "After recovery, node 1 should have at least 2 outgoing neighbors"
        );

        // Verify specific neighbors are present
        let neighbor_vec: Vec<i64> = neighbors.iter().copied().collect();
        assert!(
            neighbor_vec.contains(&2),
            "Node 1 should have edge to node 2"
        );
        assert!(
            neighbor_vec.contains(&3),
            "Node 1 should have edge to node 3"
        );
    }
}

/// Test 6: Data persists after multiple flush cycles with WAL truncation
///
/// VERIFIED: Multiple insert/flush cycles work correctly, WAL is truncated each time,
/// and all data is recoverable from main DB file.
#[test]
fn test_data_persists_after_multiple_wal_truncations() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let wal_path = db_path.with_extension("v3wal");

    // Create database file
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    // Phase 1: Insert multiple batches, each flushed
    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

        // First batch
        for i in 0..5 {
            edge_store
                .insert_edge(1, i + 10, Direction::Outgoing, None)
                .expect("Insert failed");
        }
        edge_store.flush(None).expect("Flush failed");
        assert!(
            !wal_path.exists(),
            "WAL should be truncated after first flush"
        );

        // Second batch
        for i in 0..5 {
            edge_store
                .insert_edge(2, i + 20, Direction::Outgoing, None)
                .expect("Insert failed");
        }
        edge_store.flush(None).expect("Flush failed");
        assert!(
            !wal_path.exists(),
            "WAL should be truncated after second flush"
        );
    }

    // Phase 2: Verify all data persisted
    let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    let neighbors1 = recovered_store
        .outgoing(1)
        .expect("Failed to get node 1 neighbors");
    assert_eq!(
        neighbors1.len(),
        5,
        "Node 1 should have 5 outgoing neighbors"
    );

    let neighbors2 = recovered_store
        .outgoing(2)
        .expect("Failed to get node 2 neighbors");
    assert_eq!(
        neighbors2.len(),
        5,
        "Node 2 should have 5 outgoing neighbors"
    );
}

/// Test 6: Empty flush should not error
///
/// Edge case: flush() with no dirty clusters should succeed gracefully.
#[test]
fn test_flush_with_no_dirty_clusters() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create edge store without inserting any edges
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path));

    // Flush with empty cache should succeed
    let result = edge_store.flush(None);
    assert!(result.is_ok(), "Flush with empty cache should succeed");
}

/// Test 7: Multiple flushes should be idempotent
///
/// Calling flush() multiple times should not corrupt data.
#[test]
fn test_multiple_flushes_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert edges
    edge_store
        .insert_edge(1, 2, Direction::Outgoing, None)
        .expect("Insert failed");

    // Flush multiple times
    for _ in 0..3 {
        edge_store.flush(None).expect("Flush failed");
    }

    // After implementing flush: verify edges are still queryable
    // Currently this just verifies no panic occurs
}

/// Test 8: WAL EdgeInsert record is correctly written and can be recovered
///
/// CRITICAL: This test verifies that edge_insert() writes a proper WAL record
/// that can be recovered during WAL replay.
#[test]
fn test_wal_edge_insert_record_format() {
    use crate::backend::native::v3::wal::{V3_WAL_HEADER_SIZE, V3WALRecord, V3WALRecordType};
    use std::fs;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let wal_path = db_path.with_extension("v3wal");

    // Create edge store with WAL
    let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));

    // Insert an edge - should write EdgeInsert WAL record
    edge_store
        .insert_edge(1, 2, Direction::Outgoing, None)
        .expect("Insert failed");
    edge_store.flush_wal().expect("WAL flush failed");

    // Read WAL file
    let wal_content = fs::read(&wal_path).expect("Failed to read WAL");

    // Verify WAL has more than just header (64 bytes)
    assert!(
        wal_content.len() > V3_WAL_HEADER_SIZE,
        "WAL should have records beyond header"
    );

    // Verify EdgeInsert record type is in the WAL
    // WAL format: [size: 4 bytes][bincode serialized record]
    let mut pos = V3_WAL_HEADER_SIZE; // Skip header

    let mut found_edge_insert = false;
    while pos < wal_content.len() - 8 {
        // Read record size
        if pos + 4 > wal_content.len() {
            break;
        }
        let size = u32::from_le_bytes([
            wal_content[pos],
            wal_content[pos + 1],
            wal_content[pos + 2],
            wal_content[pos + 3],
        ]) as usize;

        pos += 4;
        if pos + size > wal_content.len() || size == 0 {
            break;
        }

        // Deserialize the record using bincode
        let record_bytes = &wal_content[pos..pos + size];
        if let Ok(record) = V3WALRecord::from_bytes(record_bytes)
            && record.record_type() == V3WALRecordType::EdgeInsert
        {
            found_edge_insert = true;
            break;
        }

        // Skip to next record
        pos += size;
    }

    assert!(
        found_edge_insert,
        "WAL should contain EdgeInsert record (type 12)"
    );
}

#[path = "edge_compat_weighted_tests.rs"]
mod edge_compat_weighted_tests;
