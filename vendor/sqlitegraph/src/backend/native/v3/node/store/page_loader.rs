use super::{DEFAULT_PAGE_SIZE, File, NativeBackendError, NativeResult, NodePage, V3_HEADER_SIZE};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

/// PageLoader for loading NodePage structures from disk.
pub struct PageLoader {
    file: Arc<File>,
    page_size: usize,
    header_size: u64,
}

impl PageLoader {
    pub fn new(file: Arc<File>, page_size: usize) -> Self {
        PageLoader {
            file,
            page_size,
            header_size: V3_HEADER_SIZE,
        }
    }

    pub fn with_default_page_size(file: Arc<File>) -> Self {
        Self::new(file, DEFAULT_PAGE_SIZE as usize)
    }

    pub fn load_page(&self, page_id: u64) -> NativeResult<NodePage> {
        if page_id == 0 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_id".to_string(),
                reason: "Cannot load header page (page 0) as NodePage".to_string(),
            });
        }

        let page_bytes = self.load_page_bytes(page_id)?;
        NodePage::unpack(&page_bytes)
    }

    pub fn load_page_bytes(&self, page_id: u64) -> NativeResult<Vec<u8>> {
        if page_id == 0 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_id".to_string(),
                reason: "Cannot load header page (page 0) bytes".to_string(),
            });
        }

        let offset = Self::page_offset(page_id);
        let mut file = self
            .file
            .try_clone()
            .map_err(|_| NativeBackendError::IoError {
                context: "Failed to clone file handle for page read".to_string(),
                source: std::io::Error::other("Arc clone failed"),
            })?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to seek to page {} offset {}", page_id, offset),
                source: e,
            })?;

        let mut buffer = vec![0u8; self.page_size];
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to read page {} at offset {}", page_id, offset),
                source: e,
            })?;

        if bytes_read != self.page_size {
            return Err(NativeBackendError::IoError {
                context: format!(
                    "Incomplete page read: expected {} bytes, got {}",
                    self.page_size, bytes_read
                ),
                source: std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Page truncated"),
            });
        }

        Ok(buffer)
    }

    pub fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            return 0;
        }
        let data_page_index = page_id.saturating_sub(1);
        V3_HEADER_SIZE + data_page_index * DEFAULT_PAGE_SIZE
    }

    pub fn validate_page_checksum(&self, page_bytes: &[u8]) -> NativeResult<()> {
        use crate::backend::native::v3::node::page::constants;

        if page_bytes.len() < constants::PAGE_HEADER_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_checksum".to_string(),
                reason: format!(
                    "Insufficient bytes for checksum: need at least {}, got {}",
                    constants::PAGE_HEADER_SIZE,
                    page_bytes.len()
                ),
            });
        }

        let checksum_offset = constants::CHECKSUM_OFFSET;
        let stored_checksum = u32::from_be_bytes(
            page_bytes[checksum_offset..checksum_offset + 4]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "checksum".to_string(),
                    reason: "Failed to read checksum bytes".to_string(),
                })?,
        );

        let calculated_checksum =
            crate::backend::native::v3::constants::checksum::xor_checksum(page_bytes) as u32;

        if calculated_checksum != stored_checksum {
            return Err(NativeBackendError::InvalidChecksum {
                expected: stored_checksum as u64,
                found: calculated_checksum as u64,
            });
        }

        Ok(())
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn header_size(&self) -> u64 {
        self.header_size
    }

    pub fn load_pages<'a, I>(&self, page_ids: I) -> Vec<NativeResult<NodePage>>
    where
        I: IntoIterator<Item = &'a u64>,
    {
        page_ids
            .into_iter()
            .map(|&page_id| self.load_page(page_id))
            .collect()
    }
}
