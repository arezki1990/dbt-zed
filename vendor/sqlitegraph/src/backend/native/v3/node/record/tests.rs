use super::*;

#[test]
fn test_constants() {
    assert_eq!(FIXED_METADATA_SIZE, 44);
    assert_eq!(MAX_INLINE_DATA, 64);
    assert_eq!(constants::ID_OFFSET, 0);
    assert_eq!(constants::FLAGS_OFFSET, 8);
    assert_eq!(constants::KIND_OFFSET, 12);
    assert_eq!(constants::NAME_OFFSET, 14);
    assert_eq!(constants::DATA_LEN_OFFSET, 16);
    assert_eq!(constants::OUTGOING_CLUSTER_OFFSET, 18);
    assert_eq!(constants::OUTGOING_COUNT_OFFSET, 26);
    assert_eq!(constants::INCOMING_CLUSTER_OFFSET, 30);
    assert_eq!(constants::INCOMING_COUNT_OFFSET, 38);
}

#[test]
fn test_new_inline_node() {
    let node = NodeRecordV3::new_inline(
        12345,
        NodeFlags::empty(),
        100,
        200,
        b"test data".to_vec(),
        1000,
        5,
        2000,
        3,
    );

    assert_eq!(node.id(), 12345);
    assert!(node.is_inline());
    assert!(!node.is_external());
    assert_eq!(node.data_len(), 9);
}

#[test]
fn test_new_external_node() {
    let node =
        NodeRecordV3::new_external(12345, NodeFlags::empty(), 100, 200, 5000, 100, 0, 5, 0, 3);

    assert_eq!(node.id(), 12345);
    assert!(!node.is_inline());
    assert!(node.is_external());
    assert_eq!(node.data_len(), 100);
}

#[test]
fn test_inline_data_max_size() {
    let max_data = vec![0xFFu8; MAX_INLINE_DATA];
    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, max_data.clone(), 0, 0, 0, 0);
    assert!(node.is_inline());
    assert_eq!(node.data_len(), MAX_INLINE_DATA as u16);
}

#[test]
#[should_panic(expected = "Inline data exceeds MAX_INLINE_DATA")]
fn test_inline_data_too_large_panics() {
    let too_large = vec![0xFFu8; MAX_INLINE_DATA + 1];
    let _ = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, too_large, 0, 0, 0, 0);
}

#[test]
fn test_serialize_inline_node() {
    let node = NodeRecordV3::new_inline(
        -12345,
        NodeFlags::empty(),
        100,
        200,
        b"Hello, V3!".to_vec(),
        1000,
        5,
        2000,
        3,
    );

    let serialized = node.serialize().unwrap();
    assert_eq!(serialized.len(), FIXED_METADATA_SIZE + "Hello, V3!".len());
}

#[test]
fn test_serialize_external_node() {
    let node =
        NodeRecordV3::new_external(12345, NodeFlags::empty(), 100, 200, 5000, 100, 0, 5, 0, 3);

    let serialized = node.serialize().unwrap();
    assert_eq!(serialized.len(), FIXED_METADATA_SIZE + 8);
}

#[test]
fn test_round_trip_inline() {
    let original = NodeRecordV3::new_inline(
        999999,
        NodeFlags::DELETED,
        42,
        84,
        b"Test node data for round-trip".to_vec(),
        1111,
        10,
        2222,
        20,
    );

    let serialized = original.serialize().unwrap();
    let restored = NodeRecordV3::deserialize(&serialized).unwrap();

    assert_eq!(restored.id(), original.id());
    assert_eq!(restored.flags, original.flags);
    assert_eq!(restored.kind_offset, original.kind_offset);
    assert_eq!(restored.name_offset, original.name_offset);
    assert_eq!(restored.data_len(), original.data_len());
    assert_eq!(restored.data_inline, original.data_inline);
    assert_eq!(
        restored.outgoing_cluster_offset,
        original.outgoing_cluster_offset
    );
    assert_eq!(restored.outgoing_edge_count, original.outgoing_edge_count);
    assert_eq!(
        restored.incoming_cluster_offset,
        original.incoming_cluster_offset
    );
    assert_eq!(restored.incoming_edge_count, original.incoming_edge_count);
}

#[test]
fn test_round_trip_external() {
    let original =
        NodeRecordV3::new_external(888888, NodeFlags::empty(), 10, 20, 7777, 200, 0, 15, 0, 25);

    let serialized = original.serialize().unwrap();
    let restored = NodeRecordV3::deserialize(&serialized).unwrap();

    assert_eq!(restored.id(), original.id());
    assert_eq!(restored.flags, original.flags);
    assert_eq!(restored.kind_offset, original.kind_offset);
    assert_eq!(restored.name_offset, original.name_offset);
    assert_eq!(restored.data_len(), original.data_len());
    assert!(restored.is_external());
}

#[test]
fn test_full_id_encoding() {
    let test_ids = vec![0, 1, -1, 1000000, -1000000, i64::MAX, i64::MIN];

    for id in test_ids {
        let node = NodeRecordV3::new_inline(id, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);
        let serialized = node.serialize().unwrap();
        let restored = NodeRecordV3::deserialize(&serialized).unwrap();
        assert_eq!(
            restored.id(),
            id,
            "ID {} should be preserved through round-trip",
            id
        );
    }
}

#[test]
fn test_serialized_size_calculation() {
    let empty = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![], 0, 0, 0, 0);
    assert_eq!(empty.serialized_size(), FIXED_METADATA_SIZE);

    let small = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0, 0, vec![1u8; 10], 0, 0, 0, 0);
    assert_eq!(small.serialized_size(), FIXED_METADATA_SIZE + 10);

    let max = NodeRecordV3::new_inline(
        1,
        NodeFlags::empty(),
        0,
        0,
        vec![2u8; MAX_INLINE_DATA],
        0,
        0,
        0,
        0,
    );
    assert_eq!(max.serialized_size(), FIXED_METADATA_SIZE + MAX_INLINE_DATA);
}

#[test]
fn test_edge_cluster_offsets_preserved() {
    let node = NodeRecordV3::new_inline(
        1,
        NodeFlags::empty(),
        0,
        0,
        vec![],
        0x123456789ABCDEF0,
        42,
        0xFEDCBA9876543210,
        99,
    );

    let serialized = node.serialize().unwrap();
    let restored = NodeRecordV3::deserialize(&serialized).unwrap();

    assert_eq!(restored.outgoing_cluster_offset, 0x123456789ABCDEF0);
    assert_eq!(restored.outgoing_edge_count, 42);
    assert_eq!(restored.incoming_cluster_offset, 0xFEDCBA9876543210);
    assert_eq!(restored.incoming_edge_count, 99);
}

#[test]
fn test_deserialize_insufficient_bytes() {
    let short_data = vec![0u8; 10];
    let result = NodeRecordV3::deserialize(&short_data);
    assert!(result.is_err());
}

#[test]
fn test_flags_encoding() {
    let flags = NodeFlags::DELETED;
    let node = NodeRecordV3::new_inline(1, flags, 0, 0, vec![], 0, 0, 0, 0);

    let serialized = node.serialize().unwrap();
    let restored = NodeRecordV3::deserialize(&serialized).unwrap();

    assert_eq!(restored.flags, flags);
    assert!(restored.flags.contains(NodeFlags::DELETED));
}

#[test]
fn test_string_table_offsets() {
    let node = NodeRecordV3::new_inline(1, NodeFlags::empty(), 0x1234, 0x5678, vec![], 0, 0, 0, 0);

    let serialized = node.serialize().unwrap();
    let restored = NodeRecordV3::deserialize(&serialized).unwrap();

    assert_eq!(restored.kind_offset, 0x1234);
    assert_eq!(restored.name_offset, 0x5678);
}
