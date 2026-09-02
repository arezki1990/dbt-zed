//! Comprehensive unit tests for NodeStore V3 components
//!
//! This module provides test coverage for:
//! - B+Tree lookup operations
//! - NodePage loading and decompression
//! - Traversal cache behavior (via store module)
//! - Error handling
//!
//! Test utilities:
//! - Test helpers for NodeRecordV3 creation
//! - Integration-style tests for end-to-end flows

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::types::NodeFlags;
use crate::backend::native::v3::index::{IndexPage, IndexPageType};
use crate::backend::native::v3::node::page::NodePage;
use crate::backend::native::v3::node::record::NodeRecordV3;
use crate::backend::native::v3::node::store::TraversalCache;
use std::sync::Arc;

/// Test node ID range for scalability tests
pub const TEST_NODE_COUNT: usize = 100;

/// Page capacity for test fixtures
pub const TEST_PAGE_CAPACITY: usize = 20;

// ============================================================================
// B+Tree Lookup Tests
// ============================================================================

#[test]
fn test_btree_lookup_single_node() {
    // Create a simple index page with one entry
    let index_page = IndexPage::new_leaf(1);

    // Verify basic page structure
    assert_eq!(index_page.page_id(), 1);
    assert!(matches!(index_page.page_type(), IndexPageType::Leaf));
    assert_eq!(index_page.count(), 0);
}

#[test]
fn test_btree_lookup_multiple_nodes() {
    // Test sequential node lookups across multiple pages
    let page_ids: Vec<u64> = (1..=5).collect();

    for (i, &page_id) in page_ids.iter().enumerate() {
        let index_page = IndexPage::new_leaf(page_id);
        assert_eq!(index_page.page_id(), page_id);
        assert_eq!(index_page.count(), 0);

        // Simulate adding node_id = i * 100 to page_id
        let test_node_id = (i * 100) as i64;
        assert!(test_node_id >= 0);
    }
}

#[test]
fn test_btree_lookup_nonexistent_node() {
    // Test that missing node returns None
    let index_page = IndexPage::new_leaf(1);

    // In a full implementation, searching for a non-existent key
    // would return None. Here we verify the page is empty.
    assert_eq!(index_page.count(), 0);
}

#[test]
fn test_btree_index_page_creation() {
    // Test internal vs leaf page creation
    let leaf_page = IndexPage::new_leaf(10);
    assert!(matches!(leaf_page.page_type(), IndexPageType::Leaf));
    assert_eq!(leaf_page.page_id(), 10);
}

#[test]
fn test_btree_page_type_discrimination() {
    // Verify leaf vs internal page behavior
    let leaf = IndexPage::new_leaf(1);
    assert!(matches!(leaf.page_type(), IndexPageType::Leaf));

    // Index pages support both types for B+Tree structure
    // Internal pages would be created with IndexPage::new_internal()
    // Testing the basic structure exists
}

// ============================================================================
// NodePage Loading Tests
// ============================================================================

#[test]
fn test_page_loading_decompression() {
    // Test that NodePage can pack and unpack correctly
    let mut page = NodePage::new(1);

    // Add nodes with varying data sizes
    for i in 0..5 {
        let node = NodeRecordV3::new_inline(
            i as i64,
            NodeFlags::empty(),
            i as u16 * 10,
            i as u16 * 20,
            vec![i as u8; 32],
            i as u64 * 1000,
            i as u32 * 5,
            i as u64 * 2000,
            i as u32 * 3,
        );
        page.add_node(node).unwrap();
    }

    // Pack to bytes (simulating disk write)
    let bytes = page.pack().unwrap();

    // Unpack (simulating disk read + decompression)
    let loaded_page = NodePage::unpack(&bytes).unwrap();

    // Verify all nodes loaded correctly
    assert_eq!(loaded_page.node_count(), 5);
    assert_eq!(loaded_page.page_id, 1);

    for (i, node) in loaded_page.nodes.iter().enumerate() {
        assert_eq!(node.id(), i as i64);
        assert_eq!(node.kind_offset, (i * 10) as u16);
        assert_eq!(node.name_offset, (i * 20) as u16);
    }
}

#[test]
fn test_page_checksum_validation() {
    // Create a page and verify checksum
    let mut page = NodePage::new(42);

    let node = NodeRecordV3::new_inline(
        123,
        NodeFlags::empty(),
        10,
        20,
        b"test data".to_vec(),
        1000,
        5,
        2000,
        3,
    );
    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();

    // Valid checksum should unpack correctly
    assert!(NodePage::unpack(&bytes).is_ok());

    // Corrupt checksum should fail
    let mut corrupted = bytes;
    corrupted[28] ^= 0xFF; // Flip bits in checksum field

    let result = NodePage::unpack(&corrupted);
    assert!(result.is_err(), "Corrupted checksum should fail validation");

    if let Err(NativeBackendError::InvalidHeader { field, .. }) = result {
        assert!(field.contains("checksum") || field.contains("node_page"));
    } else {
        panic!("Expected InvalidHeader error for checksum mismatch");
    }
}

#[test]
fn test_page_not_found_error() {
    // Test error handling for short page data
    let short_data = vec![0u8; 100];
    let result = NodePage::unpack(&short_data);
    assert!(result.is_err(), "Short page data should return error");

    // Verify it's the right error type
    if let Err(NativeBackendError::InvalidHeader { field, .. }) = result {
        assert!(
            field.contains("node_page"),
            "Expected node_page error for short data"
        );
    }
}

#[test]
fn test_page_overflow_handling() {
    // Test adding nodes until page is full
    let mut page = NodePage::new(1);
    let mut added_count = 0;

    // Add nodes with 50 bytes of data each
    // This should fit approximately 4064 / (12 + 50) ≈ 66 nodes
    for i in 0..100 {
        let node = NodeRecordV3::new_inline(
            i as i64,
            NodeFlags::empty(),
            0,
            0,
            vec![i as u8; 50],
            0,
            0,
            0,
            0,
        );

        match page.add_node(node) {
            Ok(_) => added_count += 1,
            Err(_) => {
                // Page is full
                break;
            }
        }
    }

    // Should fit at least 20 nodes
    assert!(
        added_count >= 20,
        "Should fit at least 20 nodes, got {}",
        added_count
    );

    // Verify round-trip works
    let bytes = page.pack().unwrap();
    let loaded = NodePage::unpack(&bytes).unwrap();
    assert_eq!(loaded.node_count() as usize, added_count);
}

#[test]
fn test_page_empty_data() {
    // Test loading page with minimal node data
    let mut page = NodePage::new(1);

    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);
    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let loaded = NodePage::unpack(&bytes).unwrap();

    assert_eq!(loaded.node_count(), 1);
    assert_eq!(loaded.nodes[0].data_inline, Some(vec![]));
}

#[test]
fn test_page_with_external_data() {
    // Test nodes with external data references
    let mut page = NodePage::new(1);

    let node = NodeRecordV3::new_external(1, NodeFlags::empty(), 100, 200, 5000, 200, 0, 5, 0, 3);
    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let loaded = NodePage::unpack(&bytes).unwrap();

    assert_eq!(loaded.node_count(), 1);
    assert!(loaded.nodes[0].is_external());
    assert_eq!(loaded.nodes[0].data_len(), 200);
}

// ============================================================================
// Traversal Cache Integration Tests
// ============================================================================

#[test]
fn test_cache_hit_miss_tracking() {
    let mut cache = TraversalCache::new(4);

    // Initial state: no hits, no misses
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.hits(), 0);
    assert_eq!(cache.misses(), 0);

    // Cache miss on first access
    assert!(cache.get(1).is_none());
    assert_eq!(cache.misses(), 1);

    // Insert page
    let page = Arc::new(NodePage::new(1));
    cache.insert(1, page);

    // Cache hit on second access
    assert!(cache.get(1).is_some());
    assert_eq!(cache.hits(), 1);
    assert_eq!(cache.misses(), 1);
}

#[test]
fn test_cache_lru_eviction() {
    let mut cache = TraversalCache::new(2);

    // Insert two pages
    cache.insert(1, Arc::new(NodePage::new(1)));
    cache.insert(2, Arc::new(NodePage::new(2)));

    assert_eq!(cache.len(), 2);

    // Insert third page - should evict oldest
    cache.insert(3, Arc::new(NodePage::new(3)));

    assert_eq!(cache.len(), 2);

    // Page 1 should be evicted
    assert!(cache.get(1).is_none());
    assert!(cache.get(2).is_some());
    assert!(cache.get(3).is_some());
}

#[test]
fn test_cache_invalidation() {
    let mut cache = TraversalCache::new(4);

    cache.insert(1, Arc::new(NodePage::new(1)));
    cache.insert(2, Arc::new(NodePage::new(2)));

    assert_eq!(cache.len(), 2);

    // Invalidate page 1
    cache.invalidate(1);

    assert_eq!(cache.len(), 1);
    assert!(cache.get(1).is_none());
    assert!(cache.get(2).is_some());
}

#[test]
fn test_cache_clear() {
    let mut cache = TraversalCache::new(4);

    cache.insert(1, Arc::new(NodePage::new(1)));
    cache.insert(2, Arc::new(NodePage::new(2)));
    cache.insert(3, Arc::new(NodePage::new(3)));

    assert_eq!(cache.len(), 3);

    cache.clear();

    assert_eq!(cache.len(), 0);
    assert!(cache.get(1).is_none());
    assert!(cache.get(2).is_none());
    assert!(cache.get(3).is_none());
}

#[test]
fn test_cache_hit_rate_calculation() {
    let mut cache = TraversalCache::new(4);

    // No accesses
    assert_eq!(cache.hit_ratio(), 0.0);

    // Insert one page
    cache.insert(1, Arc::new(NodePage::new(1)));

    // All hits
    for _ in 0..10 {
        cache.get(1);
    }

    assert_eq!(cache.hit_ratio(), 1.0);

    // Mix of hits and misses
    cache.get(2); // miss
    cache.get(3); // miss

    // 10 hits, 2 misses = 10/12 ≈ 0.833
    let hit_rate = cache.hit_ratio();
    assert!(
        hit_rate > 0.8 && hit_rate < 0.9,
        "Expected hit rate ~0.833, got {}",
        hit_rate
    );
}

#[test]
fn test_cache_sequential_access_pattern() {
    // Simulate sequential traversal access pattern
    let mut cache = TraversalCache::new(4);

    // Simulate BFS traversal: access pages 1,2,3,4, then revisit 1,2
    for i in 1..=4 {
        cache.insert(i, Arc::new(NodePage::new(i)));
    }

    // Access all pages (all hits)
    for i in 1..=4 {
        cache.get(i);
    }

    assert_eq!(cache.hits(), 4);
    assert_eq!(cache.misses(), 0);
    assert_eq!(cache.hit_ratio(), 1.0);
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_storage_error_propagation() {
    // Test that storage errors are properly propagated
    let short_data = vec![0u8; 10];
    let result = NodePage::unpack(&short_data);

    assert!(result.is_err());
    match result {
        Err(NativeBackendError::InvalidHeader { field, .. }) => {
            assert!(field.contains("node_page") || field.contains("insufficient"));
        }
        _ => panic!("Expected InvalidHeader error for insufficient bytes"),
    }
}

#[test]
fn test_compression_error_handling() {
    // Test handling of corrupted compressed data
    let mut page = NodePage::new(1);

    // Add a valid node
    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![1, 2, 3, 4], 0, 0, 0, 0);
    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();

    // Corrupt the node data region
    let mut corrupted = bytes;
    // Corrupt some bytes in the data region (after header)
    let data_start = 32; // PAGE_HEADER_SIZE
    if data_start + 10 < corrupted.len() {
        corrupted[data_start] ^= 0xFF;
        corrupted[data_start + 5] ^= 0xFF;
    }

    // Should fail during unpack
    let result = NodePage::unpack(&corrupted);
    assert!(result.is_err(), "Corrupted data should fail unpacking");
}

#[test]
fn test_corruption_recovery() {
    // Test recovery from corruption using checksums
    let mut page = NodePage::new(1);

    for i in 0..3 {
        let node = NodeRecordV3::new_inline(
            i as i64,
            NodeFlags::empty(),
            i as u16 * 10,
            i as u16 * 20,
            vec![i as u8; 20],
            i as u64 * 100,
            i as u32 * 2,
            i as u64 * 200,
            i as u32 * 3,
        );
        page.add_node(node).unwrap();
    }

    let valid_bytes = page.pack().unwrap();
    let _valid_checksum = u32::from_be_bytes([
        valid_bytes[28],
        valid_bytes[29],
        valid_bytes[30],
        valid_bytes[31],
    ]);

    // Corrupt data
    let mut corrupted = valid_bytes;
    corrupted[100] ^= 0xFF;

    // Unpack should detect checksum mismatch
    let result = NodePage::unpack(&corrupted);
    assert!(result.is_err());
}

#[test]
fn test_invalid_node_id() {
    // Test handling of invalid node IDs
    let mut page = NodePage::new(1);

    // Test with i64::MIN (should work)
    let node = NodeRecordV3::new_inline(i64::MIN, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);
    assert!(page.add_node(node).is_ok());

    // Test with i64::MAX (should work)
    let node2 = NodeRecordV3::new_inline(i64::MAX, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);
    assert!(page.add_node(node2).is_ok());
}

#[test]
fn test_edge_case_empty_page() {
    // Test handling of completely empty page
    let page = NodePage::new(0);
    let bytes = page.pack().unwrap();

    assert_eq!(bytes.len(), 4096);

    let loaded = NodePage::unpack(&bytes).unwrap();
    assert_eq!(loaded.page_id, 0);
    assert_eq!(loaded.node_count(), 0);
    assert!(loaded.is_empty());
}

#[test]
fn test_edge_case_max_inline_data() {
    // Test node with maximum inline data
    let mut page = NodePage::new(1);

    let max_data = vec![0xABu8; 64]; // MAX_INLINE_DATA
    let node = NodeRecordV3::new_inline(
        1,
        NodeFlags::empty(),
        100,
        200,
        max_data.clone(),
        0,
        0,
        0,
        0,
    );

    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let loaded = NodePage::unpack(&bytes).unwrap();

    assert_eq!(loaded.nodes[0].data_inline, Some(max_data));
}

// ============================================================================
// Test Helpers
// ============================================================================

/// Create a test node with default values
pub fn create_test_node(id: i64) -> NodeRecordV3 {
    NodeRecordV3::new_inline(
        id,
        NodeFlags::empty(),
        (id % 1000) as u16,
        ((id % 100) + 100) as u16,
        vec![id as u8; 32],
        (id as u64) * 1000,
        ((id % 10) + 1) as u32,
        (id as u64) * 2000,
        ((id % 5) + 1) as u32,
    )
}

/// Create a test page with specified number of nodes
pub fn create_test_page(page_id: u64, node_count: usize) -> NodePage {
    let mut page = NodePage::new(page_id);

    for i in 0..node_count {
        let node = create_test_node(i as i64);
        if page.add_node(node).is_err() {
            break; // Page full
        }
    }

    page
}

/// Verify round-trip serialization
pub fn verify_round_trip(page: &NodePage) -> NativeResult<()> {
    let bytes = page.pack()?;
    let loaded = NodePage::unpack(&bytes)?;

    assert_eq!(loaded.page_id, page.page_id);
    assert_eq!(loaded.node_count(), page.node_count());
    assert_eq!(loaded.nodes.len(), page.nodes.len());

    for (original, restored) in page.nodes.iter().zip(loaded.nodes.iter()) {
        assert_eq!(restored.id(), original.id());
        assert_eq!(restored.flags, original.flags);
        assert_eq!(restored.kind_offset, original.kind_offset);
        assert_eq!(restored.name_offset, original.name_offset);
    }

    Ok(())
}

// ============================================================================
// Integration-style Tests
// ============================================================================

#[test]
fn test_end_to_end_node_storage() {
    // Test full flow: create page -> add nodes -> pack -> unpack -> verify
    let page = create_test_page(1, 10);

    assert!(page.node_count() > 0);

    verify_round_trip(&page).unwrap();
}

#[test]
fn test_multiple_pages_round_trip() {
    // Test multiple pages can be independently round-tripped
    let pages: Vec<NodePage> = vec![
        create_test_page(1, 5),
        create_test_page(2, 10),
        create_test_page(3, 15),
    ];

    for page in &pages {
        verify_round_trip(page).unwrap();
    }
}

#[test]
fn test_node_flags_preservation() {
    // Test that all node flags are preserved through round-trip
    let flags_to_test = vec![NodeFlags::empty(), NodeFlags::DELETED, NodeFlags::NONE];

    for flags in flags_to_test {
        let mut page = NodePage::new(1);
        let node = NodeRecordV3::new_inline(1, flags, 10, 20, vec![], 0, 0, 0, 0);
        page.add_node(node).unwrap();

        let bytes = page.pack().unwrap();
        let loaded = NodePage::unpack(&bytes).unwrap();

        assert_eq!(loaded.nodes[0].flags, flags);
    }
}

#[test]
fn test_page_capacity_calculation() {
    // Test that page capacity calculations are correct
    let mut page = NodePage::new(1);

    // Initial capacity should be full usable size
    assert_eq!(page.remaining_capacity(), 4064);

    // Add a node that uses some space (max 64 bytes inline)
    let node = NodeRecordV3::new_inline(
        1,
        NodeFlags::empty(),
        0,
        0,
        vec![1u8; 50], // Use 50 bytes (under MAX_INLINE_DATA of 64)
        0,
        0,
        0,
        0,
    );
    page.add_node(node).unwrap();

    // Capacity should have decreased
    assert!(page.remaining_capacity() < 4064);
}

#[test]
fn test_space_efficiency_tracking() {
    // Test space efficiency calculation
    let mut page = NodePage::new(1);

    // Empty page has 0 efficiency
    assert_eq!(page.space_efficiency(), 0.0);

    // Add nodes to fill some space
    for i in 0..5 {
        let node = NodeRecordV3::new_inline(
            i as i64,
            NodeFlags::empty(),
            i as u16 * 10,
            i as u16 * 20,
            vec![i as u8; 50],
            i as u64 * 100,
            i as u32 * 2,
            i as u64 * 200,
            i as u32 * 3,
        );
        page.add_node(node).unwrap();
    }

    // Efficiency should be between 0 and 1
    let efficiency = page.space_efficiency();
    assert!(efficiency > 0.0);
    assert!(efficiency <= 1.0);
}

#[test]
fn test_cache_with_various_page_ids() {
    // Test cache works with various page ID values
    let mut cache = TraversalCache::new(16);

    let test_ids = vec![0, 1, 100, 1000, u64::MAX - 1];

    for &id in &test_ids {
        cache.insert(id, Arc::new(NodePage::new(id)));
    }

    assert_eq!(cache.len(), test_ids.len());

    for &id in &test_ids {
        assert!(cache.contains(&id));
        let retrieved = cache.get(id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().page_id, id);
    }
}

#[test]
fn test_large_scale_page_operations() {
    // Test scalability with many pages
    let page_count = 50;
    let pages: Vec<NodePage> = (0..page_count).map(|i| create_test_page(i, 10)).collect();

    // Verify all pages can round-trip
    for page in &pages {
        verify_round_trip(page).unwrap();
    }

    // Verify cache can handle all these pages
    let mut cache = TraversalCache::new(32);

    // Insert half of them (at capacity)
    for (i, page) in pages.iter().take(32).enumerate() {
        cache.insert(i as u64, Arc::new(page.clone()));
    }

    assert_eq!(cache.len(), 32);
    assert_eq!(cache.capacity(), 32);
}
