use super::constants::*;
use super::*;
use crate::backend::native::types::NodeFlags;

#[test]
fn test_constants() {
    assert_eq!(PAGE_HEADER_SIZE, 32);
    assert_eq!(MAX_PAGE_SIZE, 4096);
    assert_eq!(USABLE_SIZE, 4064);
    const { assert!(MAX_NODE_CAPACITY > 0) };
    const { assert!(MAX_NODE_CAPACITY <= 100) };
}

#[test]
fn test_new_page() {
    let page = NodePage::new(42);
    assert_eq!(page.page_id, 42);
    assert_eq!(page.next_page_id, 0);
    assert_eq!(page.node_count(), 0);
    assert!(page.is_empty());
    assert!(!page.is_full());
    assert!(!page.has_overflow());
}

#[test]
fn test_page_with_capacity() {
    let page = NodePage::with_capacity(100, 20);
    assert_eq!(page.page_id, 100);
    assert_eq!(page.node_count(), 0);
    assert!(page.nodes.capacity() >= 20);
}

#[test]
fn test_add_node() {
    let page = &mut NodePage::new(1);

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

    assert!(page.add_node(node).is_ok());
    assert_eq!(page.node_count(), 1);
    assert!(!page.is_empty());
}

#[test]
fn test_add_multiple_nodes() {
    let page = &mut NodePage::new(1);

    for i in 0..10 {
        let node = NodeRecordV3::new_inline(
            i,
            NodeFlags::empty(),
            i as u16 * 10,
            i as u16 * 20,
            vec![i as u8; 20],
            i as u64 * 1000,
            i as u32 * 5,
            i as u64 * 2000,
            i as u32 * 3,
        );
        assert!(page.add_node(node).is_ok());
    }

    assert_eq!(page.node_count(), 10);
}

#[test]
fn test_pack_unpack_round_trip() {
    let page = &mut NodePage::new(1);

    for i in 0..5 {
        let node = NodeRecordV3::new_inline(
            i * 100,
            NodeFlags::empty(),
            i as u16 * 10,
            i as u16 * 20,
            format!("node_{}_data", i).into_bytes(),
            i as u64 * 1000,
            i as u32 * 5,
            i as u64 * 2000,
            i as u32 * 3,
        );
        page.add_node(node).unwrap();
    }

    let bytes = page.pack().unwrap();
    assert_eq!(bytes.len(), MAX_PAGE_SIZE);

    let restored = NodePage::unpack(&bytes).unwrap();
    assert_eq!(restored.page_id, 1);
    assert_eq!(restored.node_count(), 5);
    assert_eq!(restored.nodes.len(), 5);

    for (i, node) in restored.nodes.iter().enumerate() {
        assert_eq!(node.id(), (i * 100) as i64);
        assert_eq!(node.kind_offset, (i * 10) as u16);
        assert_eq!(node.name_offset, (i * 20) as u16);
    }
}

#[test]
fn test_pack_unpack_preserves_all_fields() {
    let page = &mut NodePage::new(99);
    page.next_page_id = 200;

    let node = NodeRecordV3::new_inline(
        -12345,
        NodeFlags::DELETED,
        42,
        84,
        b"Test node data for full preservation".to_vec(),
        0x123456789ABCDEF0,
        42,
        0xFEDCBA9876543210,
        99,
    );

    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert_eq!(restored.page_id, 99);
    assert_eq!(restored.next_page_id, 200);
    assert_eq!(restored.node_count(), 1);

    let restored_node = &restored.nodes[0];
    assert_eq!(restored_node.id(), -12345);
    assert_eq!(restored_node.flags, NodeFlags::DELETED);
    assert_eq!(restored_node.kind_offset, 42);
    assert_eq!(restored_node.name_offset, 84);
    assert_eq!(
        restored_node.data_inline,
        Some(b"Test node data for full preservation".to_vec())
    );
    assert_eq!(restored_node.outgoing_cluster_offset, 0x123456789ABCDEF0);
    assert_eq!(restored_node.outgoing_edge_count, 42);
    assert_eq!(restored_node.incoming_cluster_offset, 0xFEDCBA9876543210);
    assert_eq!(restored_node.incoming_edge_count, 99);
}

#[test]
fn test_checksum_validation() {
    let page = &mut NodePage::new(1);

    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, b"data".to_vec(), 0, 0, 0, 0);
    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    assert!(NodePage::unpack(&bytes).is_ok());

    let mut corrupted = bytes;
    corrupted[constants::CHECKSUM_OFFSET] ^= 0xFF;
    assert!(NodePage::unpack(&corrupted).is_err());
}

#[test]
fn test_empty_page_round_trip() {
    let page = NodePage::new(0);

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert_eq!(restored.page_id, 0);
    assert_eq!(restored.node_count(), 0);
    assert!(restored.is_empty());
}

#[test]
fn test_overflow_page_link() {
    let mut page = NodePage::new(10);
    page.next_page_id = 20;

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert_eq!(restored.next_page_id, 20);
    assert!(restored.has_overflow());
}

#[test]
fn test_used_size_calculation() {
    let page = &mut NodePage::new(1);

    let empty_node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);

    page.add_node(empty_node).unwrap();
    assert_eq!(page.used_size(), 12);

    let node_with_data =
        NodeRecordV3::new_inline(2, NodeFlags::empty(), 0, 0, vec![1u8; 32], 0, 0, 0, 0);

    page.add_node(node_with_data).unwrap();
    assert_eq!(page.used_size(), 12 + 12 + 32);
}

#[test]
fn test_remaining_capacity() {
    let page = &mut NodePage::new(1);
    assert_eq!(page.remaining_capacity(), USABLE_SIZE);

    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![0u8; 50], 0, 0, 0, 0);

    page.add_node(node).unwrap();
    assert!(page.remaining_capacity() < USABLE_SIZE);
}

#[test]
fn test_space_efficiency() {
    let page = &mut NodePage::new(1);
    assert_eq!(page.space_efficiency(), 0.0);

    let node = NodeRecordV3::new_inline(
        1,
        NodeFlags::empty(),
        0,
        0,
        vec![1u8; FIXED_METADATA_SIZE],
        0,
        0,
        0,
        0,
    );

    page.add_node(node).unwrap();

    let efficiency = page.space_efficiency();
    assert!(efficiency > 0.0);
    assert!(efficiency < 1.0);
}

#[test]
fn test_disk_size() {
    let page = NodePage::new(1);
    assert_eq!(page.disk_size(), MAX_PAGE_SIZE);
}

#[test]
fn test_pack_returns_exact_size() {
    let page = NodePage::new(1);
    let bytes = page.pack().unwrap();
    assert_eq!(bytes.len(), MAX_PAGE_SIZE);
}

#[test]
fn test_insufficient_bytes_error() {
    let short_data = vec![0u8; 100];
    let result = NodePage::unpack(&short_data);
    assert!(result.is_err());
}

#[test]
fn test_full_id_encoding_preserved() {
    let page = &mut NodePage::new(1);
    let test_ids = vec![0, 1, 100, 101, 1000, 1001];

    for id in &test_ids {
        let node = NodeRecordV3::new_inline(*id, NodeFlags::empty(), 10, 20, vec![], 0, 5, 0, 3);
        page.add_node(node).unwrap();
    }

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    for (i, node) in restored.nodes.iter().enumerate() {
        assert_eq!(node.id(), test_ids[i], "ID at index {} not preserved", i);
    }
}

#[test]
fn test_page_capacity_limits() {
    let page = &mut NodePage::new(1);

    for count in 0..50 {
        let node = NodeRecordV3::new_inline(
            count as i64,
            NodeFlags::empty(),
            0,
            0,
            vec![count as u8; 50],
            0,
            0,
            0,
            0,
        );

        if page.add_node(node).is_err() {
            break;
        }
    }

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert!(
        restored.node_count() >= 20,
        "Should fit at least 20 nodes, got {}",
        restored.node_count()
    );
}

#[test]
fn test_max_inline_data_node() {
    let page = &mut NodePage::new(1);

    let max_data = vec![0xFFu8; MAX_INLINE_DATA];
    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, max_data, 0, 0, 0, 0);

    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert_eq!(restored.node_count(), 1);
    assert_eq!(
        restored.nodes[0].data_inline.as_ref().unwrap().len(),
        MAX_INLINE_DATA
    );
}

#[test]
fn test_external_node_record() {
    let page = &mut NodePage::new(1);

    let node = NodeRecordV3::new_external(1, NodeFlags::empty(), 0, 0, 5000, 200, 0, 5, 0, 3);

    page.add_node(node).unwrap();

    let bytes = page.pack().unwrap();
    let restored = NodePage::unpack(&bytes).unwrap();

    assert_eq!(restored.node_count(), 1);
    assert!(restored.nodes[0].is_external());
    assert_eq!(restored.nodes[0].data_len(), 200);
}

#[test]
fn test_multiple_page_ids() {
    for page_id in [0, 1, 100, u64::MAX] {
        let page = NodePage::new(page_id);
        assert_eq!(page.page_id, page_id);

        let bytes = page.pack().unwrap();
        let restored = NodePage::unpack(&bytes).unwrap();
        assert_eq!(restored.page_id, page_id);
    }
}

#[test]
fn test_default_trait() {
    let page = NodePage::default();
    assert_eq!(page.page_id, 0);
    assert!(page.is_empty());
}
