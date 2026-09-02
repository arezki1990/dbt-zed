use super::{
    Direction, EDGE_CLUSTER_PAGE_HEADER_SIZE, EDGE_CLUSTER_PAGE_MAGIC,
    PACKED_EDGE_PAGE_HEADER_SIZE, PACKED_EDGE_PAGE_MAGIC, PACKED_EDGE_PAGE_SLOT_SIZE,
};
use crate::backend::native::types::{NativeBackendError, NativeResult};

pub(crate) fn encode_edge_cluster_pages(
    cluster_bytes: &[u8],
    page_size: usize,
    page_ids: &[u64],
) -> NativeResult<Vec<Vec<u8>>> {
    if page_size <= EDGE_CLUSTER_PAGE_HEADER_SIZE {
        return Err(NativeBackendError::SerializationError {
            context: format!(
                "edge page size {} too small for header {}",
                page_size, EDGE_CLUSTER_PAGE_HEADER_SIZE
            ),
        });
    }
    if page_ids.is_empty() {
        return Err(NativeBackendError::SerializationError {
            context: "missing page ids for edge cluster encode".to_string(),
        });
    }

    let payload_capacity = page_size - EDGE_CLUSTER_PAGE_HEADER_SIZE;
    let expected_pages = cluster_bytes.len().max(1).div_ceil(payload_capacity);
    if expected_pages != page_ids.len() {
        return Err(NativeBackendError::SerializationError {
            context: format!(
                "edge cluster page count mismatch: expected {}, got {}",
                expected_pages,
                page_ids.len()
            ),
        });
    }

    let mut pages = Vec::with_capacity(page_ids.len());
    for (index, chunk) in cluster_bytes.chunks(payload_capacity).enumerate() {
        let mut page = vec![0u8; page_size];
        page[0..4].copy_from_slice(&EDGE_CLUSTER_PAGE_MAGIC);
        page[4..8].copy_from_slice(&(chunk.len() as u32).to_be_bytes());
        let next_page_id = page_ids.get(index + 1).copied().unwrap_or(0);
        page[8..16].copy_from_slice(&next_page_id.to_be_bytes());
        let payload_end = EDGE_CLUSTER_PAGE_HEADER_SIZE + chunk.len();
        page[EDGE_CLUSTER_PAGE_HEADER_SIZE..payload_end].copy_from_slice(chunk);
        pages.push(page);
    }

    if cluster_bytes.is_empty() {
        let mut page = vec![0u8; page_size];
        page[0..4].copy_from_slice(&EDGE_CLUSTER_PAGE_MAGIC);
        page[4..8].copy_from_slice(&0u32.to_be_bytes());
        page[8..16].copy_from_slice(&0u64.to_be_bytes());
        pages.push(page);
    }

    Ok(pages)
}

pub(crate) fn decode_edge_cluster_page_header(page: &[u8]) -> Option<(usize, u64)> {
    if page.len() < EDGE_CLUSTER_PAGE_HEADER_SIZE || page[0..4] != EDGE_CLUSTER_PAGE_MAGIC {
        return None;
    }

    let payload_len = u32::from_be_bytes([page[4], page[5], page[6], page[7]]) as usize;
    let max_payload = page.len().saturating_sub(EDGE_CLUSTER_PAGE_HEADER_SIZE);
    if payload_len > max_payload {
        return None;
    }

    let next_page_id = u64::from_be_bytes([
        page[8], page[9], page[10], page[11], page[12], page[13], page[14], page[15],
    ]);
    Some((payload_len, next_page_id))
}

pub(crate) fn encode_packed_edge_page(
    page_size: usize,
    entries: &[((i64, Direction), Vec<u8>)],
) -> NativeResult<Vec<u8>> {
    let slot_bytes = entries.len() * PACKED_EDGE_PAGE_SLOT_SIZE;
    if PACKED_EDGE_PAGE_HEADER_SIZE + slot_bytes > page_size {
        return Err(NativeBackendError::SerializationError {
            context: format!(
                "packed edge page header {} + slots {} exceed page size {}",
                PACKED_EDGE_PAGE_HEADER_SIZE, slot_bytes, page_size
            ),
        });
    }

    let mut payload_cursor = PACKED_EDGE_PAGE_HEADER_SIZE + slot_bytes;
    let total_payload: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    if payload_cursor + total_payload > page_size {
        return Err(NativeBackendError::SerializationError {
            context: format!(
                "packed edge page payload {} exceeds page size {}",
                payload_cursor + total_payload,
                page_size
            ),
        });
    }

    let mut page = vec![0u8; page_size];
    page[0..4].copy_from_slice(&PACKED_EDGE_PAGE_MAGIC);
    page[4..6].copy_from_slice(&(entries.len() as u16).to_be_bytes());

    for (idx, ((src, dir), cluster_bytes)) in entries.iter().enumerate() {
        let slot_offset = PACKED_EDGE_PAGE_HEADER_SIZE + idx * PACKED_EDGE_PAGE_SLOT_SIZE;
        page[slot_offset..slot_offset + 8].copy_from_slice(&src.to_be_bytes());
        page[slot_offset + 8] = match dir {
            Direction::Outgoing => 0,
            Direction::Incoming => 1,
        };
        page[slot_offset + 9] = 0;
        page[slot_offset + 10..slot_offset + 12]
            .copy_from_slice(&(payload_cursor as u16).to_be_bytes());
        page[slot_offset + 12..slot_offset + 14]
            .copy_from_slice(&(cluster_bytes.len() as u16).to_be_bytes());
        page[slot_offset + 14..slot_offset + 16].copy_from_slice(&0u16.to_be_bytes());

        let payload_end = payload_cursor + cluster_bytes.len();
        page[payload_cursor..payload_end].copy_from_slice(cluster_bytes);
        payload_cursor = payload_end;
    }

    Ok(page)
}

pub(crate) fn decode_packed_edge_page(
    page: &[u8],
    src: i64,
    dir: Direction,
) -> NativeResult<Option<Vec<u8>>> {
    if page.len() < PACKED_EDGE_PAGE_HEADER_SIZE || page[0..4] != PACKED_EDGE_PAGE_MAGIC {
        return Ok(None);
    }

    let slot_count = u16::from_be_bytes([page[4], page[5]]) as usize;
    let slot_region_end = PACKED_EDGE_PAGE_HEADER_SIZE + slot_count * PACKED_EDGE_PAGE_SLOT_SIZE;
    if slot_region_end > page.len() {
        return Err(NativeBackendError::DeserializationError {
            context: "Packed edge page slot directory exceeds page length".to_string(),
        });
    }

    for idx in 0..slot_count {
        let slot_offset = PACKED_EDGE_PAGE_HEADER_SIZE + idx * PACKED_EDGE_PAGE_SLOT_SIZE;
        let slot_src = i64::from_be_bytes(
            page[slot_offset..slot_offset + 8]
                .try_into()
                .expect("slot src bounds checked"),
        );
        let slot_dir = if page[slot_offset + 8] == 1 {
            Direction::Incoming
        } else {
            Direction::Outgoing
        };
        if slot_src != src || slot_dir != dir {
            continue;
        }

        let payload_offset =
            u16::from_be_bytes([page[slot_offset + 10], page[slot_offset + 11]]) as usize;
        let payload_len =
            u16::from_be_bytes([page[slot_offset + 12], page[slot_offset + 13]]) as usize;
        let payload_end = payload_offset + payload_len;
        if payload_offset < slot_region_end || payload_end > page.len() {
            return Err(NativeBackendError::DeserializationError {
                context: format!(
                    "Packed edge page payload out of bounds: offset {} len {} page {}",
                    payload_offset,
                    payload_len,
                    page.len()
                ),
            });
        }

        return Ok(Some(page[payload_offset..payload_end].to_vec()));
    }

    Ok(None)
}
