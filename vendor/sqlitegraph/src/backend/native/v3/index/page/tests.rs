use super::*;

#[test]
fn test_constants() {
    assert_eq!(constants::PAGE_HEADER_SIZE, 32);
    assert_eq!(MAX_KEYS, 252);
    assert_eq!(MAX_ENTRIES, 252);
    assert_eq!(MAX_CHILDREN, 253);
    assert_eq!(KEY_SIZE, 8);
    assert_eq!(PAGE_ID_SIZE, 8);
    assert_eq!(ENTRY_SIZE, 16);
}

#[test]
fn test_new_internal_page() {
    let page = IndexPage::new_internal(42);
    assert_eq!(page.page_id(), 42);
    assert_eq!(page.page_type(), IndexPageType::Internal);
    assert_eq!(page.count(), 0);
    assert!(!page.is_full_internal());
}

#[test]
fn test_new_leaf_page() {
    let page = IndexPage::new_leaf(99);
    assert_eq!(page.page_id(), 99);
    assert_eq!(page.page_type(), IndexPageType::Leaf);
    assert_eq!(page.count(), 0);
    assert!(!page.is_full_leaf());
}

#[test]
fn test_internal_page_round_trip() {
    let original = IndexPage::Internal {
        page_id: 1,
        keys: vec![100, 200, 300],
        children: vec![10, 11, 12, 13],
        checksum: 0,
        is_root: false,
    };

    let bytes = original.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();

    match restored {
        IndexPage::Internal {
            page_id,
            keys,
            children,
            ..
        } => {
            assert_eq!(page_id, 1);
            assert_eq!(keys, vec![100, 200, 300]);
            assert_eq!(children, vec![10, 11, 12, 13]);
        }
        _ => panic!("Expected Internal page"),
    }
}

#[test]
fn test_leaf_page_round_trip() {
    let original = IndexPage::Leaf {
        page_id: 2,
        entries: vec![(1, 10), (5, 11), (9, 12)],
        next_leaf: 3,
        checksum: 0,
        is_root: false,
    };

    let bytes = original.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();

    match restored {
        IndexPage::Leaf {
            page_id,
            entries,
            next_leaf,
            ..
        } => {
            assert_eq!(page_id, 2);
            assert_eq!(entries, vec![(1, 10), (5, 11), (9, 12)]);
            assert_eq!(next_leaf, 3);
        }
        _ => panic!("Expected Leaf page"),
    }
}

#[test]
fn test_full_internal_page_round_trip() {
    let mut keys = Vec::with_capacity(MAX_KEYS);
    let mut children = Vec::with_capacity(MAX_CHILDREN);

    for i in 0..MAX_KEYS {
        keys.push((i as u64) * 100 + 100);
    }
    for i in 0..(MAX_KEYS + 1) {
        children.push(i as u64);
    }

    let original = IndexPage::Internal {
        page_id: 5,
        keys,
        children,
        checksum: 0,
        is_root: false,
    };

    let bytes = original.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();

    assert_eq!(restored.count(), MAX_KEYS);
    match restored {
        IndexPage::Internal {
            keys: k,
            children: c,
            ..
        } => {
            assert_eq!(k.len(), MAX_KEYS);
            assert_eq!(c.len(), MAX_KEYS + 1);
        }
        _ => panic!("Expected internal page"),
    }
}

#[test]
fn test_full_leaf_page_round_trip() {
    let mut entries = Vec::with_capacity(MAX_ENTRIES);

    for i in 0..MAX_ENTRIES {
        entries.push((i as u64, (i as u64) * 100));
    }

    let original = IndexPage::Leaf {
        page_id: 6,
        entries,
        next_leaf: 0,
        checksum: 0,
        is_root: false,
    };

    let bytes = original.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();

    assert_eq!(restored.count(), MAX_ENTRIES);
    match restored {
        IndexPage::Leaf { entries: e, .. } => {
            assert_eq!(e.len(), MAX_ENTRIES);
        }
        _ => panic!("Expected leaf page"),
    }
}

#[test]
fn test_binary_search_leaf_found() {
    let entries = vec![(10, 1), (20, 2), (30, 3), (40, 4), (50, 5)];
    let result = IndexPage::binary_search_leaf(&entries, 30);
    assert_eq!(result, Ok(2));
}

#[test]
fn test_binary_search_leaf_not_found() {
    let entries = vec![(10, 1), (20, 2), (40, 4), (50, 5)];
    let result = IndexPage::binary_search_leaf(&entries, 30);
    assert_eq!(result, Err(2));
}

#[test]
fn test_find_child_index() {
    let keys = vec![100, 200, 300, 400];

    assert_eq!(IndexPage::find_child_index(&keys, 200), 2);
    assert_eq!(IndexPage::find_child_index(&keys, 150), 1);
    assert_eq!(IndexPage::find_child_index(&keys, 50), 0);
    assert_eq!(IndexPage::find_child_index(&keys, 500), 4);
}

#[test]
fn test_checksum_validation_internal() {
    let page = IndexPage::Internal {
        page_id: 1,
        keys: vec![100, 200],
        children: vec![10, 11, 12],
        checksum: 0,
        is_root: false,
    };

    let bytes = page.pack().unwrap();
    assert!(IndexPage::unpack(&bytes).is_ok());

    let mut corrupted = bytes;
    corrupted[constants::CHECKSUM_OFFSET] ^= 0xFF;
    assert!(IndexPage::unpack(&corrupted).is_err());
}

#[test]
fn test_checksum_validation_leaf() {
    let page = IndexPage::Leaf {
        page_id: 1,
        entries: vec![(1, 10), (2, 20)],
        next_leaf: 0,
        checksum: 0,
        is_root: false,
    };

    let bytes = page.pack().unwrap();
    assert!(IndexPage::unpack(&bytes).is_ok());

    let mut corrupted = bytes;
    corrupted[constants::CHECKSUM_OFFSET] ^= 0xFF;
    assert!(IndexPage::unpack(&corrupted).is_err());
}

#[test]
fn test_invalid_children_count() {
    let page = IndexPage::Internal {
        page_id: 1,
        keys: vec![100, 200],
        children: vec![10, 11],
        checksum: 0,
        is_root: false,
    };

    assert!(page.pack().is_err());
}

#[test]
fn test_empty_pages_round_trip() {
    let internal = IndexPage::new_internal(0);
    let bytes = internal.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();
    assert_eq!(restored.page_id(), 0);
    assert_eq!(restored.count(), 0);

    let leaf = IndexPage::new_leaf(0);
    let bytes = leaf.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();
    assert_eq!(restored.page_id(), 0);
    assert_eq!(restored.count(), 0);
}

#[test]
fn test_leaf_with_next_pointer() {
    let page = IndexPage::Leaf {
        page_id: 10,
        entries: vec![(1, 100), (2, 200)],
        next_leaf: 11,
        checksum: 0,
        is_root: false,
    };

    let bytes = page.pack().unwrap();
    let restored = IndexPage::unpack(&bytes).unwrap();

    match restored {
        IndexPage::Leaf { next_leaf, .. } => {
            assert_eq!(next_leaf, 11);
        }
        _ => panic!("Expected leaf page"),
    }
}
