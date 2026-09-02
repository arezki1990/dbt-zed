use super::*;

#[test]
fn test_header_size_is_112_bytes() {
    assert_eq!(
        std::mem::size_of::<PersistentHeaderV3>(),
        112,
        "PersistentHeaderV3 must be exactly 112 bytes"
    );
    assert_eq!(
        PERSISTENT_HEADER_V3_SIZE, 112,
        "Calculated header size must be 112 bytes"
    );
}

#[test]
fn test_new_v3_header() {
    let header = PersistentHeaderV3::new_v3();

    assert_eq!(header.magic, V3_MAGIC);
    assert_eq!(header.version, V3_FORMAT_VERSION);
    assert_eq!(header.node_count, 0);
    assert_eq!(header.edge_count, 0);
    assert_eq!(header.root_index_page, 0);
    assert_eq!(header.total_pages, 0);
    assert_eq!(header.btree_height, 0);
    assert_eq!(header.page_size, DEFAULT_PAGE_SIZE as u32);
}

#[test]
fn test_validate_valid_header() {
    let header = PersistentHeaderV3::new_v3();
    assert!(header.validate().is_ok(), "New V3 header should validate");
}

#[test]
fn test_validate_rejects_v2_magic() {
    let mut header = PersistentHeaderV3::new_v3();
    header.magic = V2_MAGIC;

    let result = header.validate();
    assert!(result.is_err(), "Should reject V2 magic");

    match result {
        Err(NativeBackendError::UnsupportedVersion { version, .. }) => {
            assert_eq!(version, 2, "Should report version 2");
        }
        _ => panic!("Should return UnsupportedVersion error"),
    }
}

#[test]
fn test_validate_rejects_wrong_version() {
    let mut header = PersistentHeaderV3::new_v3();
    header.version = 999;

    let result = header.validate();
    assert!(result.is_err(), "Should reject invalid version");

    match result {
        Err(NativeBackendError::UnsupportedVersion { .. }) => {}
        _ => panic!("Should return UnsupportedVersion error"),
    }
}

#[test]
fn test_validate_rejects_invalid_page_size() {
    let mut header = PersistentHeaderV3::new_v3();
    header.page_size = 12345;

    let result = header.validate();
    assert!(result.is_err(), "Should reject invalid page size");
}

#[test]
fn test_validate_rejects_excessive_btree_height() {
    let mut header = PersistentHeaderV3::new_v3();
    header.btree_height = MAX_BTREE_HEIGHT + 1;

    let result = header.validate();
    assert!(result.is_err(), "Should reject excessive B+Tree height");
}

#[test]
fn test_round_trip_serialization() {
    let original = PersistentHeaderV3 {
        magic: V3_MAGIC,
        version: V3_FORMAT_VERSION,
        flags: DEFAULT_V3_FEATURE_FLAGS,
        node_count: 12345,
        edge_count: 67890,
        schema_version: 2,
        reserved: 0,
        node_data_offset: 112,
        edge_data_offset: 2000,
        outgoing_cluster_offset: 3000,
        incoming_cluster_offset: 4000,
        free_space_offset: 5000,
        root_index_page: 42,
        free_page_list_head: 0,
        total_pages: 100,
        page_size: 4096,
        btree_height: 3,
    };

    let bytes = original.to_bytes();
    let restored = PersistentHeaderV3::from_bytes(&bytes).unwrap();

    assert_eq!(restored, original, "Round-trip should preserve all fields");
}

#[test]
fn test_detect_version_v3() {
    let header = PersistentHeaderV3::new_v3();
    let bytes = header.to_bytes();

    let version = PersistentHeaderV3::detect_version(&bytes).unwrap();
    assert_eq!(version, 3, "Should detect V3 version");
}

#[test]
fn test_detect_version_v2() {
    let mut bytes = [0u8; 112];
    bytes[0..8].copy_from_slice(&V2_MAGIC);

    let version = PersistentHeaderV3::detect_version(&bytes).unwrap();
    assert_eq!(version, 2, "Should detect V2 version");
}

#[test]
fn test_offset_constants_match_layout() {
    assert_eq!(offset::MAGIC, 0);
    assert_eq!(offset::VERSION, 8);
    assert_eq!(offset::FLAGS, 12);
    assert_eq!(offset::NODE_COUNT, 16);
    assert_eq!(offset::EDGE_COUNT, 24);
    assert_eq!(offset::SCHEMA_VERSION, 32);
    assert_eq!(offset::RESERVED, 36);
    assert_eq!(offset::NODE_DATA_OFFSET, 40);
    assert_eq!(offset::EDGE_DATA_OFFSET, 48);
    assert_eq!(offset::OUTGOING_CLUSTER_OFFSET, 56);
    assert_eq!(offset::INCOMING_CLUSTER_OFFSET, 64);
    assert_eq!(offset::FREE_SPACE_OFFSET, 72);
    assert_eq!(offset::ROOT_INDEX_PAGE, 80);
    assert_eq!(offset::FREE_PAGE_LIST_HEAD, 88);
    assert_eq!(offset::TOTAL_PAGES, 96);
    assert_eq!(offset::PAGE_SIZE, 104);
    assert_eq!(offset::BTREE_HEIGHT, 108);
}

#[test]
fn test_v3_preserves_v2_layout_prefix() {
    assert_eq!(
        offset::ROOT_INDEX_PAGE,
        80,
        "V3 fields should start at byte 80"
    );
}
