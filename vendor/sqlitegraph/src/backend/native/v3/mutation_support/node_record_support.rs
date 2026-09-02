use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::NodeSpec;
use crate::backend::native::types::NodeFlags;
use crate::backend::native::v3::NodeRecordV3;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

const MAX_INLINE_DATA: usize = 64;

impl V3Backend {
    pub(crate) fn build_insert_node_record(
        &self,
        node: &NodeSpec,
        requested_id: Option<i64>,
    ) -> Result<NodeRecordV3, SqliteGraphError> {
        let payload = Self::serialize_node_payload(node);

        if payload.len() <= MAX_INLINE_DATA {
            return Ok(NodeRecordV3::new_inline(
                requested_id.unwrap_or(0),
                NodeFlags::empty(),
                0,
                0,
                payload,
                0,
                0,
                0,
                0,
            ));
        }

        self.build_external_node_record(requested_id, payload)
    }

    fn serialize_node_payload(node: &NodeSpec) -> Vec<u8> {
        let kind_bytes = node.kind.as_bytes();
        let name_bytes = node.name.as_bytes();
        let data_bytes = serde_json::to_vec(&node.data).unwrap_or_default();
        let total_len = 2 + kind_bytes.len() + name_bytes.len() + data_bytes.len();

        let mut payload = Vec::with_capacity(total_len);
        payload.push(kind_bytes.len() as u8);
        payload.extend_from_slice(kind_bytes);
        payload.push(name_bytes.len() as u8);
        payload.extend_from_slice(name_bytes);
        payload.extend_from_slice(&data_bytes);
        payload
    }

    fn build_external_node_record(
        &self,
        requested_id: Option<i64>,
        payload: Vec<u8>,
    ) -> Result<NodeRecordV3, SqliteGraphError> {
        let data_len = payload.len();
        let page_size = crate::backend::native::v3::constants::DEFAULT_PAGE_SIZE as usize;
        let pages_needed = data_len.div_ceil(page_size);

        let mut allocator = self.allocator.write();
        let start_page_id = allocator
            .allocate()
            .map_err(SqliteGraphError::NativeError)?;

        for _ in 1..pages_needed {
            allocator
                .allocate()
                .map_err(SqliteGraphError::NativeError)?;
        }

        let offset = Self::page_offset(start_page_id);
        drop(allocator);

        Self::write_external_node_payload(&self.db_path, offset, &payload)?;

        Ok(NodeRecordV3::new_external(
            requested_id.unwrap_or(0),
            NodeFlags::empty(),
            0,
            0,
            offset,
            data_len as u16,
            0,
            0,
            0,
            0,
        ))
    }

    fn write_external_node_payload(
        db_path: &std::path::Path,
        offset: u64,
        payload: &[u8],
    ) -> Result<(), SqliteGraphError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(db_path)
            .map_err(|e| {
                SqliteGraphError::ConnectionError(format!("Failed to open file: {}", e))
            })?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| SqliteGraphError::ConnectionError(format!("Failed to seek: {}", e)))?;
        file.write_all(payload)
            .map_err(|e| SqliteGraphError::ConnectionError(format!("Failed to write: {}", e)))?;
        file.sync_all().map_err(|e| {
            SqliteGraphError::ConnectionError(format!("Failed to sync external data: {}", e))
        })?;
        Ok(())
    }

    /// Calculate file offset for a given page ID
    ///
    /// Page 0 is the header at offset 0.
    /// Data pages start at page 1, which maps to offset V3_HEADER_SIZE.
    pub(crate) fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            return 0;
        }
        let data_page_index = page_id.saturating_sub(1);
        crate::backend::native::v3::constants::V3_HEADER_SIZE
            + data_page_index * crate::backend::native::v3::constants::DEFAULT_PAGE_SIZE
    }
}
