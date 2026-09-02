use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
use crate::errors::SqliteGraphError;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct AsyncFileCoordinator {
    file: std::fs::File,
    db_path: PathBuf,
    cached_size: AtomicU64,
    #[cfg(target_os = "linux")]
    ring: Option<rio::Rio>,
}

impl AsyncFileCoordinator {
    /// Create a new async file coordinator
    pub fn create(db_path: &Path) -> Result<Self, SqliteGraphError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(db_path)
            .map_err(|_e| {
                SqliteGraphError::connection(format!(
                    "Failed to open db file for async coordination: {}",
                    db_path.display()
                ))
            })?;

        let cached_size = file.metadata().map(|m| m.len()).unwrap_or(0);

        #[cfg(target_os = "linux")]
        let ring = rio::new().ok();

        Ok(Self {
            file,
            db_path: db_path.to_path_buf(),
            cached_size: AtomicU64::new(cached_size),
            #[cfg(target_os = "linux")]
            ring,
        })
    }

    /// Calculate page offset from page ID
    pub fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            0
        } else {
            V3_HEADER_SIZE + (page_id - 1) * DEFAULT_PAGE_SIZE
        }
    }

    /// Read a page asynchronously (without locks)
    pub async fn read_page(
        &self,
        page_id: u64,
        buf: Vec<u8>,
    ) -> Result<(Vec<u8>, usize), SqliteGraphError> {
        let offset = Self::page_offset(page_id);
        let required_len = offset + buf.len() as u64;

        let current_size = self.cached_size.load(Ordering::Acquire);
        if current_size < required_len {
            return Err(SqliteGraphError::connection(format!(
                "File too small to read page {}: size={} < required_len={}",
                page_id, current_size, required_len
            )));
        }

        #[cfg(target_os = "linux")]
        if let Some(ref ring) = self.ring {
            let completion = ring.read_at(&self.file, &buf, offset);
            let bytes_read = completion.await.map_err(|e| {
                SqliteGraphError::connection(format!(
                    "io_uring read failed on page {}: {}",
                    page_id, e
                ))
            })?;
            return Ok((buf, bytes_read));
        }

        let file = self.file.try_clone().map_err(|e| {
            SqliteGraphError::connection(format!("Failed to clone file descriptor: {}", e))
        })?;

        tokio::task::spawn_blocking(move || {
            let mut local_buf = buf;
            let n = Self::file_read_at(&file, &mut local_buf, offset)
                .map_err(|e| SqliteGraphError::connection(format!("pread failed: {}", e)))?;
            Ok((local_buf, n))
        })
        .await
        .map_err(|e| SqliteGraphError::connection(format!("blocking join failed: {}", e)))?
    }

    /// Read data from an arbitrary file offset asynchronously
    pub async fn read_at_offset(
        &self,
        offset: u64,
        buf: Vec<u8>,
    ) -> Result<(Vec<u8>, usize), SqliteGraphError> {
        let required_len = offset + buf.len() as u64;

        let current_size = self.cached_size.load(Ordering::Acquire);
        if current_size < required_len {
            return Err(SqliteGraphError::connection(format!(
                "File too small to read at offset {}: size={} < required_len={}",
                offset, current_size, required_len
            )));
        }

        #[cfg(target_os = "linux")]
        if let Some(ref ring) = self.ring {
            let completion = ring.read_at(&self.file, &buf, offset);
            let bytes_read = completion.await.map_err(|e| {
                SqliteGraphError::connection(format!(
                    "io_uring read failed at offset {}: {}",
                    offset, e
                ))
            })?;
            return Ok((buf, bytes_read));
        }

        let file = self.file.try_clone().map_err(|e| {
            SqliteGraphError::connection(format!("Failed to clone file descriptor: {}", e))
        })?;

        tokio::task::spawn_blocking(move || {
            let mut local_buf = buf;
            let n = Self::file_read_at(&file, &mut local_buf, offset)
                .map_err(|e| SqliteGraphError::connection(format!("pread failed: {}", e)))?;
            Ok((local_buf, n))
        })
        .await
        .map_err(|e| SqliteGraphError::connection(format!("blocking join failed: {}", e)))?
    }

    /// Write a page asynchronously
    pub async fn write_page(
        &self,
        page_id: u64,
        data: Vec<u8>,
    ) -> Result<Vec<u8>, SqliteGraphError> {
        let offset = Self::page_offset(page_id);
        let required_len = offset + data.len() as u64;

        #[cfg(target_os = "linux")]
        if let Some(ref ring) = self.ring {
            let completion = ring.write_at(&self.file, &data, offset);
            completion.await.map_err(|e| {
                SqliteGraphError::connection(format!(
                    "io_uring write failed on page {}: {}",
                    page_id, e
                ))
            })?;
            self.file.sync_all().map_err(|e| {
                SqliteGraphError::connection(format!("Failed to sync page {}: {}", page_id, e))
            })?;
            let current_size = self.cached_size.load(Ordering::Acquire);
            if required_len > current_size {
                self.cached_size.store(required_len, Ordering::Release);
            }
            return Ok(data);
        }

        let file = self.file.try_clone().map_err(|e| {
            SqliteGraphError::connection(format!(
                "Failed to clone file descriptor for write: {}",
                e
            ))
        })?;

        let returned_data = tokio::task::spawn_blocking(move || {
            let mut written = 0;
            while written < data.len() {
                let n = Self::file_write_at(&file, &data[written..], offset + written as u64)
                    .map_err(|e| SqliteGraphError::connection(format!("pwrite failed: {}", e)))?;
                if n == 0 {
                    return Err(SqliteGraphError::connection(
                        "pwrite wrote 0 bytes".to_string(),
                    ));
                }
                written += n;
            }
            file.sync_all()
                .map_err(|e| SqliteGraphError::connection(format!("Failed to sync page: {}", e)))?;
            Ok(data)
        })
        .await
        .map_err(|e| {
            SqliteGraphError::connection(format!("blocking join failed on write: {}", e))
        })??;

        let current_size = self.cached_size.load(Ordering::Acquire);
        if required_len > current_size {
            self.cached_size.store(required_len, Ordering::Release);
        }

        Ok(returned_data)
    }

    /// Return the current database file size
    pub fn file_size(&self) -> u64 {
        self.cached_size.load(Ordering::Acquire)
    }

    /// Get the database file path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn file_read_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            file.read_at(buf, offset)
        }
        #[cfg(windows)]
        {
            file.seek_read(buf, offset)
        }
    }

    fn file_write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            file.write_at(buf, offset)
        }
        #[cfg(windows)]
        {
            file.seek_write(buf, offset)
        }
    }
}
