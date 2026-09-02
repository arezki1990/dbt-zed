use super::{
    LSN_INVALID, MAX_RECORD_SIZE, V3_WAL_HEADER_SIZE, V3_WAL_MAGIC, V3_WAL_VERSION, V3WALHeader,
    V3WALRecord, lsn_next,
};
use crate::backend::native::v3::header::PersistentHeaderV3;
use crate::backend::native::{NativeBackendError, NativeResult};
use std::path::PathBuf;

/// WAL writer for appending records to WAL file
///
/// Handles writing WAL records to disk with proper synchronization
/// for crash recovery. Records are written in format:
///
/// ```text
/// [4 bytes: record size (little-endian u32)]
/// [N bytes: serialized record]
/// ```
///
/// # Durability
///
/// Each record is followed by an optional fsync for durability.
/// For performance, multiple records can be batched before syncing.
#[derive(Debug)]
pub struct WALWriter {
    /// WAL file path
    wal_path: PathBuf,
    /// Current LSN
    current_lsn: u64,
    /// Committed LSN
    committed_lsn: u64,
    /// Buffered records before fsync
    buffer: Vec<u8>,
    /// Buffer size threshold for auto-flush
    pub(crate) flush_threshold: usize,
}

impl WALWriter {
    /// Create a new WAL writer
    pub fn new(wal_path: PathBuf, start_lsn: u64) -> NativeResult<Self> {
        let mut writer = Self {
            wal_path,
            current_lsn: start_lsn,
            committed_lsn: LSN_INVALID,
            buffer: Vec::new(),
            flush_threshold: 64 * 1024,
        };

        if writer.wal_path.exists() {
            writer.read_header()?;
        }

        Ok(writer)
    }

    pub fn current_lsn(&self) -> u64 {
        self.current_lsn
    }

    pub fn committed_lsn(&self) -> u64 {
        self.committed_lsn
    }

    fn read_header(&mut self) -> NativeResult<()> {
        use std::io::Read;

        let mut file =
            std::fs::File::open(&self.wal_path).map_err(|e| NativeBackendError::IoError {
                context: "Failed to open WAL for reading".to_string(),
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

        self.current_lsn = header.current_lsn;
        self.committed_lsn = header.committed_lsn;

        Ok(())
    }

    pub fn write_header(&self) -> NativeResult<()> {
        use std::io::Write;

        let header = V3WALHeader {
            magic: V3_WAL_MAGIC,
            version: V3_WAL_VERSION,
            page_size: 4096,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            current_lsn: self.current_lsn,
            committed_lsn: self.committed_lsn,
            checkpointed_lsn: LSN_INVALID,
            reserved: [0; 3],
        };

        let header_bytes = header.to_bytes();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.wal_path)
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to create WAL file: {}", self.wal_path.display()),
                source: e,
            })?;

        file.write_all(&header_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to write WAL header".to_string(),
                source: e,
            })?;

        file.sync_all().map_err(|e| NativeBackendError::IoError {
            context: "Failed to sync WAL file".to_string(),
            source: e,
        })?;

        Ok(())
    }

    pub fn append(&mut self, record: &V3WALRecord) -> NativeResult<()> {
        let bytes = record.to_bytes()?;
        let size = bytes.len() as u32;

        if bytes.len() > MAX_RECORD_SIZE {
            return Err(NativeBackendError::SerializationError {
                context: format!(
                    "Record size {} exceeds maximum {}",
                    bytes.len(),
                    MAX_RECORD_SIZE
                ),
            });
        }

        self.buffer.extend_from_slice(&size.to_le_bytes());
        self.buffer.extend_from_slice(&bytes);

        if self.buffer.len() >= self.flush_threshold {
            self.flush()?;
        }

        self.current_lsn = lsn_next(self.current_lsn);
        Ok(())
    }

    pub fn flush(&mut self) -> NativeResult<()> {
        use std::io::Write;

        if self.buffer.is_empty() {
            return Ok(());
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.wal_path)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to open WAL for writing".to_string(),
                source: e,
            })?;

        file.write_all(&self.buffer)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to write WAL records".to_string(),
                source: e,
            })?;

        file.sync_all().map_err(|e| NativeBackendError::IoError {
            context: "Failed to sync WAL file".to_string(),
            source: e,
        })?;

        self.buffer.clear();
        Ok(())
    }

    pub fn commit(&mut self) -> NativeResult<()> {
        self.committed_lsn = self.current_lsn;
        self.update_header()
    }

    fn update_header(&self) -> NativeResult<()> {
        use std::io::{Read, Seek, SeekFrom, Write};

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.wal_path)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to open WAL for header update".to_string(),
                source: e,
            })?;

        file.seek(SeekFrom::Start(0))
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to seek in WAL file".to_string(),
                source: e,
            })?;

        let mut header_bytes = [0u8; V3_WAL_HEADER_SIZE];
        file.read_exact(&mut header_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to read WAL header".to_string(),
                source: e,
            })?;

        let mut header = V3WALHeader::from_bytes(&header_bytes)?;
        header.current_lsn = self.current_lsn;
        header.committed_lsn = self.committed_lsn;

        let updated_bytes = header.to_bytes();
        file.seek(SeekFrom::Start(0))
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to seek to WAL header".to_string(),
                source: e,
            })?;

        file.write_all(&updated_bytes)
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to write updated WAL header".to_string(),
                source: e,
            })?;

        file.sync_all().map_err(|e| NativeBackendError::IoError {
            context: "Failed to sync WAL header".to_string(),
            source: e,
        })?;

        Ok(())
    }

    pub fn truncate(&self) -> NativeResult<()> {
        if !self.wal_path.exists() {
            return Ok(());
        }

        std::fs::remove_file(&self.wal_path).map_err(|e| NativeBackendError::IoError {
            context: "Failed to truncate WAL file".to_string(),
            source: e,
        })?;

        Ok(())
    }

    pub fn page_allocate(&mut self, page_id: u64) -> NativeResult<u64> {
        let record = V3WALRecord::page_allocate(page_id, self.current_lsn);
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn page_free(&mut self, page_id: u64, checksum: u32) -> NativeResult<u64> {
        let record = V3WALRecord::page_free(page_id, checksum, self.current_lsn);
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn page_write(&mut self, page_id: u64, offset: u32, data: Vec<u8>) -> NativeResult<u64> {
        let record = V3WALRecord::page_write(page_id, offset, data, self.current_lsn);
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn btree_split(
        &mut self,
        original_page_id: u64,
        new_page_id: u64,
        split_key: u64,
        page_type: bool,
    ) -> NativeResult<u64> {
        let record = V3WALRecord::btree_split(
            original_page_id,
            new_page_id,
            split_key,
            page_type,
            self.current_lsn,
        );
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn checkpoint(
        &mut self,
        root_page_id: u64,
        total_pages: u64,
        btree_height: u32,
        free_page_list_head: u64,
        header: &PersistentHeaderV3,
    ) -> NativeResult<u64> {
        let record = V3WALRecord::checkpoint(
            root_page_id,
            total_pages,
            btree_height,
            free_page_list_head,
            header,
            self.current_lsn,
        );
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn transaction_begin(&mut self, tx_id: u64) -> NativeResult<u64> {
        let record = V3WALRecord::TransactionBegin {
            tx_id,
            lsn: self.current_lsn,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn transaction_commit(&mut self, tx_id: u64) -> NativeResult<u64> {
        let record = V3WALRecord::TransactionCommit {
            tx_id,
            lsn: self.current_lsn,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn transaction_rollback(&mut self, tx_id: u64) -> NativeResult<u64> {
        let record = V3WALRecord::TransactionRollback {
            tx_id,
            lsn: self.current_lsn,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn edge_insert(
        &mut self,
        src: i64,
        dst: i64,
        direction: u8,
        page_id: u64,
    ) -> NativeResult<u64> {
        let record = V3WALRecord::edge_insert(src, dst, direction, page_id, self.current_lsn);
        let lsn = record.lsn();
        self.append(&record)?;
        Ok(lsn)
    }

    pub fn set_flush_threshold(&mut self, threshold: usize) {
        self.flush_threshold = threshold;
    }
}

/// WAL file path utilities
pub struct V3WALPaths;

impl V3WALPaths {
    pub fn wal_file(db_path: &std::path::Path) -> PathBuf {
        db_path.with_extension("v3wal")
    }

    pub fn checkpoint_file(db_path: &std::path::Path) -> PathBuf {
        db_path.with_extension("v3checkpoint")
    }

    pub fn temp_checkpoint_file(db_path: &std::path::Path) -> PathBuf {
        let random: u64 = {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0) as u64
        };
        db_path.with_extension(format!("v3checkpoint.tmp.{}", random))
    }
}
