use super::codec_support;
use super::*;

impl NodePage {
    /// Find a node by ID using lazy decoding - only decodes O(log n) node IDs
    /// then fully decodes only the target node.
    ///
    /// This performs binary search directly on the encoded data, decoding node IDs
    /// on-the-fly. Once the target node is found, it fully decodes that node's record.
    ///
    /// # Parameters
    /// - `page_data`: Raw page bytes (must be MAX_PAGE_SIZE)
    ///
    /// Find a node by ID using lazy linear scan.
    ///
    /// For ~100-node pages, linear scan is simpler and nearly as fast as binary search.
    /// This only decodes node IDs during scan, then fully decodes only the target node.
    ///
    /// # Arguments
    /// - `page_data`: Raw page bytes
    /// - `node_id`: The node ID to find
    ///
    /// # Returns
    /// - `Some(NodeRecordV3)` if found
    /// - `None` if not found in this page
    pub fn find_node_lazy(page_data: &[u8], node_id: i64) -> NativeResult<Option<NodeRecordV3>> {
        use crate::backend::native::v3::compression::delta::decode_id_delta;
        use crate::backend::native::v3::compression::varint::decode_varint;

        let node_count = u16::from_be_bytes(
            page_data[constants::NODE_COUNT_OFFSET..constants::NODE_COUNT_OFFSET + 2]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.node_count".to_string(),
                    reason: "invalid node_count bytes".to_string(),
                })?,
        ) as usize;

        let base_id = i64::from_be_bytes(
            page_data[constants::BASE_ID_OFFSET..constants::BASE_ID_OFFSET + 8]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.base_id".to_string(),
                    reason: "invalid base_id bytes".to_string(),
                })?,
        );

        let used_bytes = u16::from_be_bytes(
            page_data[constants::USED_BYTES_OFFSET..constants::USED_BYTES_OFFSET + 2]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.used_bytes".to_string(),
                    reason: "invalid used_bytes bytes".to_string(),
                })?,
        ) as usize;

        let data_start = PAGE_HEADER_SIZE;
        let data_end = data_start + used_bytes;
        let data = &page_data[data_start..data_end];

        let mut offset = 0;
        for _ in 0..node_count {
            let node_start_offset = offset;

            let (delta, id_bytes) =
                decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node.id_delta".to_string(),
                    reason: "invalid varint encoding".to_string(),
                })?;
            offset += id_bytes;

            let id = decode_id_delta(delta as u32, base_id).map_err(|_| {
                NativeBackendError::InvalidHeader {
                    field: "node.id".to_string(),
                    reason: format!(
                        "failed to reconstruct ID from delta {} and base_id {}",
                        delta, base_id
                    ),
                }
            })?;

            if id == node_id {
                return codec_support::decode_node_at_offset(&data[node_start_offset..], base_id);
            }

            offset = codec_support::skip_remaining_fields(data, offset)?;
        }

        Ok(None)
    }

    fn calculate_checksum(&self, data: &[u8]) -> u32 {
        v3_constants::checksum::xor_checksum(data) as u32
    }

    pub(super) fn pack_nodes(&self) -> NativeResult<Vec<u8>> {
        codec_support::pack_nodes(&self.nodes, self.base_id, self.used_bytes)
    }

    fn calculate_checksum_with_nodes(&self) -> u32 {
        let mut data = Vec::with_capacity(PAGE_HEADER_SIZE + self.used_size());
        data.extend_from_slice(&self.page_id.to_be_bytes());
        data.extend_from_slice(&self.next_page_id.to_be_bytes());
        data.extend_from_slice(&(self.nodes.len() as u16).to_be_bytes());
        data.extend_from_slice(&self.used_bytes.to_be_bytes());
        data.extend_from_slice(&self.base_id.to_be_bytes());
        data.extend_from_slice(&[0u8; 4]);

        if let Ok(node_data) = self.pack_nodes() {
            data.extend_from_slice(&node_data);
        }

        v3_constants::checksum::xor_checksum(&data) as u32
    }

    pub fn pack(&self) -> NativeResult<[u8; MAX_PAGE_SIZE]> {
        let mut bytes = [0u8; MAX_PAGE_SIZE];
        let node_data = self.pack_nodes()?;
        let actual_used_bytes = node_data.len() as u16;

        bytes[constants::PAGE_ID_OFFSET..constants::PAGE_ID_OFFSET + 8]
            .copy_from_slice(&self.page_id.to_be_bytes());
        bytes[constants::NEXT_PAGE_ID_OFFSET..constants::NEXT_PAGE_ID_OFFSET + 8]
            .copy_from_slice(&self.next_page_id.to_be_bytes());
        bytes[constants::NODE_COUNT_OFFSET..constants::NODE_COUNT_OFFSET + 2]
            .copy_from_slice(&(self.nodes.len() as u16).to_be_bytes());
        bytes[constants::USED_BYTES_OFFSET..constants::USED_BYTES_OFFSET + 2]
            .copy_from_slice(&actual_used_bytes.to_be_bytes());
        bytes[constants::BASE_ID_OFFSET..constants::BASE_ID_OFFSET + 8]
            .copy_from_slice(&self.base_id.to_be_bytes());

        let checksum_offset = constants::CHECKSUM_OFFSET;
        if PAGE_HEADER_SIZE + node_data.len() > MAX_PAGE_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "page overflow: header {} + data {} > {}",
                    PAGE_HEADER_SIZE,
                    node_data.len(),
                    MAX_PAGE_SIZE
                ),
            });
        }

        let data_offset = PAGE_HEADER_SIZE;
        bytes[data_offset..data_offset + node_data.len()].copy_from_slice(&node_data);

        let checksum_end = data_offset + node_data.len();
        let checksum = self.calculate_checksum(&bytes[..checksum_end]);
        bytes[checksum_offset..checksum_offset + 4].copy_from_slice(&checksum.to_be_bytes());

        Ok(bytes)
    }

    pub fn unpack(bytes: &[u8]) -> NativeResult<Self> {
        #[cfg(feature = "v3-forensics")]
        {
            use crate::backend::native::v3::forensics::FORENSIC_COUNTERS;
            FORENSIC_COUNTERS
                .node_page_unpack_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        if bytes.len() < MAX_PAGE_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "insufficient bytes: expected {}, found {}",
                    MAX_PAGE_SIZE,
                    bytes.len()
                ),
            });
        }

        let page_id = u64::from_be_bytes(
            bytes[constants::PAGE_ID_OFFSET..constants::PAGE_ID_OFFSET + 8]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.page_id".to_string(),
                    reason: "invalid page_id bytes".to_string(),
                })?,
        );

        let next_page_id = u64::from_be_bytes(
            bytes[constants::NEXT_PAGE_ID_OFFSET..constants::NEXT_PAGE_ID_OFFSET + 8]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.next_page_id".to_string(),
                    reason: "invalid next_page_id bytes".to_string(),
                })?,
        );

        let node_count = u16::from_be_bytes(
            bytes[constants::NODE_COUNT_OFFSET..constants::NODE_COUNT_OFFSET + 2]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.node_count".to_string(),
                    reason: "invalid node_count bytes".to_string(),
                })?,
        ) as usize;

        let used_bytes = u16::from_be_bytes(
            bytes[constants::USED_BYTES_OFFSET..constants::USED_BYTES_OFFSET + 2]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.used_bytes".to_string(),
                    reason: "invalid used_bytes bytes".to_string(),
                })?,
        );

        let base_id = i64::from_be_bytes(
            bytes[constants::BASE_ID_OFFSET..constants::BASE_ID_OFFSET + 8]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.base_id".to_string(),
                    reason: "invalid base_id bytes".to_string(),
                })?,
        );

        let checksum = u32::from_be_bytes(
            bytes[constants::CHECKSUM_OFFSET..constants::CHECKSUM_OFFSET + 4]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_page.checksum".to_string(),
                    reason: "invalid checksum bytes".to_string(),
                })?,
        );

        let data_start = PAGE_HEADER_SIZE;
        let data_end = data_start + used_bytes as usize;
        if data_end > MAX_PAGE_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "used_bytes exceeds page boundary: {} + {} > {}",
                    data_start, used_bytes, MAX_PAGE_SIZE
                ),
            });
        }

        let (nodes, actual_bytes_used) =
            codec_support::unpack_nodes(&bytes[data_start..data_end], base_id, node_count)?;

        if actual_bytes_used != used_bytes as usize {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page".to_string(),
                reason: format!(
                    "node data size mismatch: expected {} bytes, actually used {}",
                    used_bytes, actual_bytes_used
                ),
            });
        }

        let page = NodePage {
            page_id,
            next_page_id,
            nodes,
            used_bytes,
            base_id,
            checksum,
            block_id: node_id_to_block(base_id),
        };

        let calculated_checksum = page.calculate_checksum_with_nodes();
        if calculated_checksum != checksum {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_page_checksum".to_string(),
                reason: format!(
                    "checksum mismatch: expected {}, found {}",
                    calculated_checksum, checksum
                ),
            });
        }

        Ok(page)
    }
}
