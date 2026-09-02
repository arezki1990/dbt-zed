use super::*;

impl TryFrom<u8> for V3WALRecordType {
    type Error = NativeBackendError;

    fn try_from(value: u8) -> NativeResult<Self> {
        match value {
            1 => Ok(Self::PageAllocate),
            2 => Ok(Self::PageFree),
            3 => Ok(Self::PageWrite),
            4 => Ok(Self::BTreeSplit),
            5 => Ok(Self::Checkpoint),
            6 => Ok(Self::TransactionBegin),
            7 => Ok(Self::TransactionCommit),
            8 => Ok(Self::TransactionRollback),
            9 => Ok(Self::KvSet),
            10 => Ok(Self::KvDelete),
            11 => Ok(Self::KvTombstone),
            12 => Ok(Self::EdgeInsert),
            _ => Err(NativeBackendError::InvalidHeader {
                field: "record_type".to_string(),
                reason: format!("unknown record type: {}", value),
            }),
        }
    }
}

impl V3WALRecord {
    pub fn record_type(&self) -> V3WALRecordType {
        match self {
            Self::PageAllocate { .. } => V3WALRecordType::PageAllocate,
            Self::PageFree { .. } => V3WALRecordType::PageFree,
            Self::PageWrite { .. } => V3WALRecordType::PageWrite,
            Self::BTreeSplit { .. } => V3WALRecordType::BTreeSplit,
            Self::Checkpoint { .. } => V3WALRecordType::Checkpoint,
            Self::TransactionBegin { .. } => V3WALRecordType::TransactionBegin,
            Self::TransactionCommit { .. } => V3WALRecordType::TransactionCommit,
            Self::TransactionRollback { .. } => V3WALRecordType::TransactionRollback,
            Self::KvSet { .. } => V3WALRecordType::KvSet,
            Self::KvDelete { .. } => V3WALRecordType::KvDelete,
            Self::KvTombstone { .. } => V3WALRecordType::KvTombstone,
            Self::EdgeInsert { .. } => V3WALRecordType::EdgeInsert,
        }
    }

    pub fn lsn(&self) -> u64 {
        match self {
            Self::PageAllocate { lsn, .. } => *lsn,
            Self::PageFree { lsn, .. } => *lsn,
            Self::PageWrite { lsn, .. } => *lsn,
            Self::BTreeSplit { lsn, .. } => *lsn,
            Self::Checkpoint { lsn, .. } => *lsn,
            Self::TransactionBegin { lsn, .. } => *lsn,
            Self::TransactionCommit { lsn, .. } => *lsn,
            Self::TransactionRollback { lsn, .. } => *lsn,
            Self::KvSet { lsn, .. } => *lsn,
            Self::KvDelete { lsn, .. } => *lsn,
            Self::KvTombstone { lsn, .. } => *lsn,
            Self::EdgeInsert { lsn, .. } => *lsn,
        }
    }

    pub fn is_data_modifying(&self) -> bool {
        matches!(
            self,
            Self::PageAllocate { .. }
                | Self::PageFree { .. }
                | Self::PageWrite { .. }
                | Self::BTreeSplit { .. }
        )
    }

    pub fn is_transaction_control(&self) -> bool {
        matches!(
            self,
            Self::TransactionBegin { .. }
                | Self::TransactionCommit { .. }
                | Self::TransactionRollback { .. }
        )
    }

    pub fn is_checkpoint(&self) -> bool {
        matches!(self, Self::Checkpoint { .. })
    }

    pub fn to_bytes(&self) -> NativeResult<Vec<u8>> {
        let bytes: Result<Vec<u8>, _> = bincode::serialize(self);
        bytes
            .map_err(NativeBackendError::BincodeError)
            .and_then(|bytes: Vec<u8>| {
                if bytes.len() > MAX_RECORD_SIZE {
                    Err(NativeBackendError::RecordTooLarge {
                        size: bytes.len() as u32,
                        max_size: MAX_RECORD_SIZE as u32,
                    })
                } else {
                    Ok(bytes)
                }
            })
    }

    pub fn from_bytes(bytes: &[u8]) -> NativeResult<Self> {
        bincode::deserialize(bytes).map_err(NativeBackendError::BincodeError)
    }

    pub fn calculate_checksum(&self) -> u64 {
        let bytes = match self.to_bytes() {
            Ok(b) => b,
            Err(_) => return 0,
        };
        checksum::xor_checksum(&bytes)
    }

    pub fn page_allocate(page_id: u64, lsn: u64) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::PageAllocate {
            lsn,
            page_id,
            timestamp,
        }
    }

    pub fn page_free(page_id: u64, checksum: u32, lsn: u64) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::PageFree {
            lsn,
            page_id,
            checksum,
            timestamp,
        }
    }

    pub fn page_write(page_id: u64, offset: u32, data: Vec<u8>, lsn: u64) -> Self {
        let checksum = checksum::xor_checksum(&data) as u32;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::PageWrite {
            lsn,
            page_id,
            offset,
            data,
            checksum,
            timestamp,
        }
    }

    pub fn btree_split(
        original_page_id: u64,
        new_page_id: u64,
        split_key: u64,
        is_leaf: bool,
        lsn: u64,
    ) -> Self {
        let page_type = if is_leaf { 1 } else { 0 };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::BTreeSplit {
            lsn,
            original_page_id,
            new_page_id,
            split_key,
            page_type,
            timestamp,
        }
    }

    pub fn checkpoint(
        root_page_id: u64,
        total_pages: u64,
        btree_height: u32,
        free_page_list_head: u64,
        header: &PersistentHeaderV3,
        lsn: u64,
    ) -> Self {
        let header_snapshot = header.to_bytes().to_vec();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::Checkpoint {
            lsn,
            root_page_id,
            total_pages,
            btree_height,
            free_page_list_head,
            header_snapshot,
            timestamp,
        }
    }

    pub fn edge_insert(src: i64, dst: i64, direction: u8, page_id: u64, lsn: u64) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self::EdgeInsert {
            lsn,
            src,
            dst,
            direction,
            page_id,
            timestamp,
        }
    }
}
