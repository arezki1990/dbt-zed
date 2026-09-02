use super::{
    LSN_INVALID, MAX_RECORD_SIZE, V3_WAL_HEADER_SIZE, V3WALHeader, V3WALRecord, read_kv_checkpoint,
};
use crate::backend::native::v3::header::PersistentHeaderV3;
use crate::backend::native::{NativeBackendError, NativeResult};
use std::collections::HashMap;
use std::path::PathBuf;

/// WAL recovery statistics
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WALRecoveryStats {
    pub records_processed: usize,
    pub records_applied: usize,
    pub records_skipped: usize,
    pub page_allocations: usize,
    pub page_frees: usize,
    pub page_writes: usize,
    pub btree_splits: usize,
    pub checkpoints: usize,
}

impl WALRecoveryStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_activity(&self) -> bool {
        self.records_processed > 0
    }

    pub fn success_rate(&self) -> f64 {
        if self.records_processed == 0 {
            1.0
        } else {
            self.records_applied as f64 / self.records_processed as f64
        }
    }
}

/// WAL recovery engine.
#[derive(Debug)]
pub struct WALRecovery {
    wal_path: PathBuf,
    db_path: PathBuf,
    page_cache: HashMap<u64, Vec<u8>>,
    stats: WALRecoveryStats,
    checkpoint_header: Option<PersistentHeaderV3>,
    last_lsn: u64,
}

impl WALRecovery {
    pub fn new(wal_path: PathBuf) -> Self {
        let db_path = wal_path.with_extension("");
        Self {
            wal_path,
            db_path,
            page_cache: HashMap::new(),
            stats: WALRecoveryStats::new(),
            checkpoint_header: None,
            last_lsn: LSN_INVALID,
        }
    }

    pub fn stats(&self) -> &WALRecoveryStats {
        &self.stats
    }

    pub fn checkpoint_header(&self) -> Option<&PersistentHeaderV3> {
        self.checkpoint_header.as_ref()
    }

    pub fn last_lsn(&self) -> u64 {
        self.last_lsn
    }

    pub fn page_cache(&self) -> &HashMap<u64, Vec<u8>> {
        &self.page_cache
    }

    pub fn recover(&mut self) -> NativeResult<()> {
        use std::io::Read;

        if !self.wal_path.exists() {
            return Ok(());
        }

        let mut file =
            std::fs::File::open(&self.wal_path).map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to open WAL file: {}", self.wal_path.display()),
                source: e,
            })?;

        let mut header_bytes = [0u8; V3_WAL_HEADER_SIZE];
        file.read_exact(&mut header_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to read WAL header".to_string(),
                source: e,
            })?;

        let header = V3WALHeader::from_bytes(&header_bytes)?;
        header.validate()?;

        let mut buffer = Vec::new();
        loop {
            let mut size_bytes = [0u8; 4];
            let n = file
                .read(&mut size_bytes)
                .map_err(|e| NativeBackendError::IoError {
                    context: "Failed to read record size".to_string(),
                    source: e,
                })?;

            if n == 0 {
                break;
            }

            if n < 4 {
                self.stats.records_skipped += 1;
                break;
            }

            let record_size = u32::from_le_bytes(size_bytes) as usize;
            if record_size == 0 || record_size > MAX_RECORD_SIZE {
                self.stats.records_skipped += 1;
                continue;
            }

            buffer.clear();
            buffer.resize(record_size, 0);
            if file.read_exact(&mut buffer).is_err() {
                self.stats.records_skipped += 1;
                break;
            }

            self.stats.records_processed += 1;
            match V3WALRecord::from_bytes(&buffer) {
                Ok(record) => {
                    if let Err(e) = self.apply_record(&record) {
                        eprintln!(
                            "WAL Recovery: Failed to apply record LSN {}: {:?}",
                            record.lsn(),
                            e
                        );
                        self.stats.records_skipped += 1;
                    } else {
                        self.stats.records_applied += 1;
                        self.last_lsn = record.lsn();
                    }
                }
                Err(e) => {
                    eprintln!("WAL Recovery: Failed to deserialize record: {:?}", e);
                    self.stats.records_skipped += 1;
                }
            }
        }

        Ok(())
    }

    pub(crate) fn apply_record(&mut self, record: &V3WALRecord) -> NativeResult<()> {
        match record {
            V3WALRecord::PageAllocate { page_id, lsn, .. } => {
                self.page_cache.insert(*page_id, vec![0; 4096]);
                self.stats.page_allocations += 1;
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::PageFree { page_id, lsn, .. } => {
                self.page_cache.remove(page_id);
                self.stats.page_frees += 1;
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::PageWrite {
                page_id,
                offset,
                data,
                lsn,
                ..
            } => {
                let page = self
                    .page_cache
                    .entry(*page_id)
                    .or_insert_with(|| vec![0; 4096]);
                let offset = *offset as usize;
                if offset + data.len() <= page.len() {
                    page[offset..offset + data.len()].copy_from_slice(data);
                }
                self.stats.page_writes += 1;
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::BTreeSplit {
                new_page_id, lsn, ..
            } => {
                self.page_cache.insert(*new_page_id, vec![0; 4096]);
                self.stats.btree_splits += 1;
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::Checkpoint {
                header_snapshot,
                lsn,
                ..
            } => {
                if !header_snapshot.is_empty() {
                    let restored =
                        PersistentHeaderV3::from_bytes(header_snapshot).map_err(|e| {
                            NativeBackendError::DeserializationError {
                                context: format!("Failed to restore checkpoint header: {:?}", e),
                            }
                        })?;
                    self.checkpoint_header = Some(restored);
                }
                self.stats.checkpoints += 1;
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::TransactionBegin { lsn, .. }
            | V3WALRecord::TransactionCommit { lsn, .. }
            | V3WALRecord::TransactionRollback { lsn, .. }
            | V3WALRecord::KvSet { lsn, .. }
            | V3WALRecord::KvDelete { lsn, .. }
            | V3WALRecord::KvTombstone { lsn, .. } => {
                self.last_lsn = *lsn;
                Ok(())
            }
            V3WALRecord::EdgeInsert { lsn, page_id, .. } => {
                self.page_cache
                    .entry(*page_id)
                    .or_insert_with(|| vec![0; 4096]);
                self.last_lsn = *lsn;
                Ok(())
            }
        }
    }

    pub fn get_header_state(&self) -> Option<&PersistentHeaderV3> {
        self.checkpoint_header.as_ref()
    }

    pub fn recover_kv(
        &mut self,
        kv_store: &mut super::super::kv_store::store::KvStore,
    ) -> NativeResult<usize> {
        use crate::backend::native::v3::kv_store::types::KvValue;
        use std::io::Read;

        if !self.wal_path.exists() {
            match read_kv_checkpoint(&self.db_path, kv_store) {
                Ok(found) => {
                    if found {
                        return Ok(1);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "WARNING: KV checkpoint read failed: {:?}. Continuing with empty KV store.",
                        e
                    );
                }
            }
            return Ok(0);
        }

        let mut file =
            std::fs::File::open(&self.wal_path).map_err(|e| NativeBackendError::IoError {
                context: format!(
                    "Failed to open WAL file for KV recovery: {}",
                    self.wal_path.display()
                ),
                source: e,
            })?;

        let mut header_bytes = [0u8; V3_WAL_HEADER_SIZE];
        file.read_exact(&mut header_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to read WAL header for KV recovery".to_string(),
                source: e,
            })?;

        let header = V3WALHeader::from_bytes(&header_bytes)?;
        header.validate()?;

        let mut buffer = Vec::new();
        let mut kv_records_applied = 0usize;

        loop {
            let mut size_bytes = [0u8; 4];
            let n = file
                .read(&mut size_bytes)
                .map_err(|e| NativeBackendError::IoError {
                    context: "Failed to read record size during KV recovery".to_string(),
                    source: e,
                })?;

            if n == 0 {
                break;
            }
            if n < 4 {
                break;
            }

            let record_size = u32::from_le_bytes(size_bytes) as usize;
            if record_size == 0 || record_size > MAX_RECORD_SIZE {
                continue;
            }

            buffer.clear();
            buffer.resize(record_size, 0);
            if file.read_exact(&mut buffer).is_err() {
                break;
            }

            if let Ok(record) = V3WALRecord::from_bytes(&buffer) {
                match record {
                    V3WALRecord::KvSet {
                        lsn,
                        key,
                        value_bytes,
                        value_type,
                        ttl_seconds,
                        ..
                    } => {
                        if let Some(value) = KvValue::from_bytes(&value_bytes, value_type) {
                            kv_store.set(key, value, ttl_seconds, lsn);
                            kv_records_applied += 1;
                        }
                    }
                    V3WALRecord::KvDelete { lsn, key, .. } => {
                        kv_store.delete(&key, lsn);
                        kv_records_applied += 1;
                    }
                    V3WALRecord::KvTombstone { lsn, key, .. } => {
                        kv_store.set(key, KvValue::Null, None, lsn);
                        kv_records_applied += 1;
                    }
                    _ => {}
                }
            }
        }

        Ok(kv_records_applied)
    }
}
