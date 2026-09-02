use super::*;

#[test]
fn test_encode_edge_cluster_pages_roundtrip_header_chain() {
    let cluster_bytes = (0..80u8).collect::<Vec<_>>();
    let page_ids = [10_u64, 11_u64, 12_u64];
    let pages = encode_edge_cluster_pages(&cluster_bytes, 48, &page_ids).unwrap();

    assert_eq!(pages.len(), page_ids.len());

    let mut rebuilt = Vec::new();
    for (idx, page) in pages.iter().enumerate() {
        let (payload_len, next_page_id) = decode_edge_cluster_page_header(page).unwrap();
        let expected_next = page_ids.get(idx + 1).copied().unwrap_or(0);
        assert_eq!(next_page_id, expected_next);
        rebuilt.extend_from_slice(
            &page[EDGE_CLUSTER_PAGE_HEADER_SIZE..EDGE_CLUSTER_PAGE_HEADER_SIZE + payload_len],
        );
    }

    assert_eq!(rebuilt, cluster_bytes);
}

#[test]
fn test_encode_edge_cluster_pages_rejects_wrong_page_count() {
    let cluster_bytes = (0..17u8).collect::<Vec<_>>();
    let err = encode_edge_cluster_pages(&cluster_bytes, 24, &[7]).unwrap_err();
    assert!(
        err.to_string().contains("page count mismatch"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_packed_edge_page_roundtrip_selects_matching_slot() {
    let entries = vec![
        ((41, Direction::Outgoing), vec![1_u8, 2, 3]),
        ((41, Direction::Incoming), vec![9_u8, 8, 7, 6]),
    ];
    let page = encode_packed_edge_page(128, &entries).unwrap();

    assert_eq!(
        decode_packed_edge_page(&page, 41, Direction::Outgoing).unwrap(),
        Some(vec![1_u8, 2, 3])
    );
    assert_eq!(
        decode_packed_edge_page(&page, 41, Direction::Incoming).unwrap(),
        Some(vec![9_u8, 8, 7, 6])
    );
    assert_eq!(
        decode_packed_edge_page(&page, 99, Direction::Outgoing).unwrap(),
        None
    );
}

#[test]
fn test_decode_packed_edge_page_rejects_out_of_bounds_payload() {
    let entries = vec![((5, Direction::Outgoing), vec![1_u8, 2, 3, 4])];
    let mut page = encode_packed_edge_page(64, &entries).unwrap();
    page[18..20].copy_from_slice(&60_u16.to_be_bytes());
    page[20..22].copy_from_slice(&10_u16.to_be_bytes());

    let err = decode_packed_edge_page(&page, 5, Direction::Outgoing).unwrap_err();
    assert!(
        err.to_string().contains("out of bounds"),
        "unexpected error: {err}"
    );
}
