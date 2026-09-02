use super::*;

impl Default for V3WALHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl V3WALHeader {
    /// Create a new WAL header with defaults
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            magic: V3_WAL_MAGIC,
            version: V3_WAL_VERSION,
            page_size: 4096,
            created_at: now,
            current_lsn: LSN_BEGIN,
            committed_lsn: LSN_INVALID,
            checkpointed_lsn: LSN_INVALID,
            reserved: [0; 3],
        }
    }

    pub fn validate(&self) -> NativeResult<()> {
        if self.magic != V3_WAL_MAGIC {
            return Err(NativeBackendError::InvalidHeader {
                field: "magic".to_string(),
                reason: format!("expected {:?}, found {:?}", V3_WAL_MAGIC, self.magic),
            });
        }

        if self.version != V3_WAL_VERSION {
            return Err(NativeBackendError::UnsupportedVersion {
                version: self.version,
                supported_version: V3_WAL_VERSION,
            });
        }

        if self.page_size != 4096 && self.page_size != 8192 && self.page_size != 16384 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_size".to_string(),
                reason: "must be 4096, 8192, or 16384".to_string(),
            });
        }

        if !lsn_is_valid(self.current_lsn) {
            return Err(NativeBackendError::InvalidHeader {
                field: "current_lsn".to_string(),
                reason: "must be >= LSN_BEGIN".to_string(),
            });
        }

        if self.committed_lsn > self.current_lsn {
            return Err(NativeBackendError::InvalidHeader {
                field: "committed_lsn".to_string(),
                reason: "cannot be greater than current_lsn".to_string(),
            });
        }

        if self.checkpointed_lsn > self.committed_lsn {
            return Err(NativeBackendError::InvalidHeader {
                field: "checkpointed_lsn".to_string(),
                reason: "cannot be greater than committed_lsn".to_string(),
            });
        }

        Ok(())
    }

    pub fn to_bytes(&self) -> [u8; V3_WAL_HEADER_SIZE] {
        let mut bytes = [0u8; V3_WAL_HEADER_SIZE];

        bytes[0..8].copy_from_slice(&self.magic);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.page_size.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.created_at.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.current_lsn.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.committed_lsn.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.checkpointed_lsn.to_le_bytes());
        bytes[48..56].copy_from_slice(&self.reserved[0].to_le_bytes());
        bytes[56..64].copy_from_slice(&self.reserved[1].to_le_bytes());

        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < V3_WAL_HEADER_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "bytes".to_string(),
                reason: format!(
                    "expected {} bytes, found {}",
                    V3_WAL_HEADER_SIZE,
                    bytes.len()
                ),
            });
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);

        let extract_u32 = |offset: usize| -> NativeResult<u32> {
            let slice =
                bytes
                    .get(offset..offset + 4)
                    .ok_or_else(|| NativeBackendError::InvalidHeader {
                        field: format!("offset_{}", offset),
                        reason: format!("expected 4 bytes at offset {}", offset),
                    })?;
            let arr: [u8; 4] = slice
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: format!("offset_{}", offset),
                    reason: "failed to convert to u32 array".to_string(),
                })?;
            Ok(u32::from_le_bytes(arr))
        };

        let extract_u64 = |offset: usize| -> NativeResult<u64> {
            let slice =
                bytes
                    .get(offset..offset + 8)
                    .ok_or_else(|| NativeBackendError::InvalidHeader {
                        field: format!("offset_{}", offset),
                        reason: format!("expected 8 bytes at offset {}", offset),
                    })?;
            let arr: [u8; 8] = slice
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: format!("offset_{}", offset),
                    reason: "failed to convert to u64 array".to_string(),
                })?;
            Ok(u64::from_le_bytes(arr))
        };

        let version = extract_u32(8)?;
        let page_size = extract_u32(12)?;
        let created_at = extract_u64(16)?;
        let current_lsn = extract_u64(24)?;
        let committed_lsn = extract_u64(32)?;
        let checkpointed_lsn = extract_u64(40)?;
        let reserved0 = extract_u64(48)?;
        let reserved1 = extract_u64(56)?;

        Ok(Self {
            magic,
            version,
            page_size,
            created_at,
            current_lsn,
            committed_lsn,
            checkpointed_lsn,
            reserved: [reserved0, reserved1, 0],
        })
    }
}
