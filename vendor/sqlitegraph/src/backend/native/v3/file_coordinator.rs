//! File Coordinator for V3 Backend
//!
//! Coordinates all main DB file I/O through a single file handle to prevent
//! race conditions between NodeStore, BTreeManager, and V3EdgeStore.
//!
//! **Root Cause Fixed:** Multiple components opening independent file handles
//! caused inconsistent file size metadata and data corruption during concurrent
//! page writes.
//!
//! **Solution:** Single shared file handle with RwLock-protected I/O. Reads
//! take a shared lock (concurrent readers); writes take an exclusive lock.

use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
use crate::backend::native::{NativeBackendError, NativeResult};
use parking_lot::RwLock;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::Path;

/// Coordinated file handle for all V3 main DB file I/O
///
/// All writes to the main DB file MUST go through this coordinator to ensure
/// file size metadata consistency and prevent race conditions.
/// Reads use a shared lock, enabling concurrent readers across threads.
pub struct FileCoordinator {
    /// The underlying file handle (kept open for the lifetime of the coordinator)
    file: RwLock<CoordinatedFile>,
    /// Path to the database file (for reopen on error)
    db_path: std::path::PathBuf,
}

/// Inner file handle with coordination logic
struct CoordinatedFile {
    file: std::fs::File,
    /// Cached file size to avoid repeated metadata() calls
    cached_size: u64,
}

impl FileCoordinator {
    /// Create a new file coordinator for the given database path
    ///
    /// Opens the file in read-write mode. If the file doesn't exist, it will be
    /// created when the first write occurs.
    pub fn create(db_path: &std::path::Path) -> NativeResult<Self> {
        // Open file - create if doesn't exist, but don't truncate
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false) // Create if doesn't exist, no-op if exists
            .open(db_path)
            .map_err(|e| NativeBackendError::IoError {
                context: format!(
                    "Failed to open db file for coordination: {}",
                    db_path.display()
                ),
                source: e,
            })?;

        // Get initial file size
        let cached_size = file.metadata().map(|m| m.len()).unwrap_or(0);

        Ok(Self {
            file: RwLock::new(CoordinatedFile { file, cached_size }),
            db_path: db_path.to_path_buf(),
        })
    }

    /// Write a page of data to the file at the specified offset
    ///
    /// Uses positioned I/O (`write_at`) so the file offset is not changed,
    /// allowing concurrent writes at different offsets under the write lock.
    /// Syncs to ensure durability and updates the cached size.
    pub fn write_page(&self, page_id: u64, data: &[u8]) -> NativeResult<()> {
        let mut coord = self.file.write();

        let offset = Self::page_offset(page_id);

        write_all_at(&coord.file, data, offset).map_err(|e| NativeBackendError::IoError {
            context: format!(
                "Failed to write page {} data ({} bytes) at offset {}",
                page_id,
                data.len(),
                offset
            ),
            source: e,
        })?;

        coord
            .file
            .sync_all()
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to sync page {} write", page_id),
                source: e,
            })?;

        let actual_size = coord.file.metadata().map(|m| m.len()).unwrap_or(0);
        coord.cached_size = actual_size;

        Ok(())
    }

    /// Read a page of data from the file at the specified offset
    ///
    /// Uses positioned I/O (`read_at`) so the file offset is not changed,
    /// enabling truly concurrent reads from multiple threads under a shared
    /// read lock. Returns an error if the file is shorter than expected.
    pub fn read_page(&self, page_id: u64, buffer: &mut [u8]) -> NativeResult<()> {
        let coord = self.file.read();

        let offset = Self::page_offset(page_id);
        let required_len = offset + buffer.len() as u64;

        if coord.cached_size < required_len {
            return Err(NativeBackendError::IoError {
                context: format!(
                    "File too small to read page {}: cached_size={} < required_len={}",
                    page_id, coord.cached_size, required_len
                ),
                source: std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "file size {} < required {}",
                        coord.cached_size, required_len
                    ),
                ),
            });
        }

        read_all_at(&coord.file, buffer, offset).map_err(|e| NativeBackendError::IoError {
            context: format!(
                "Failed to read page {} from disk at offset {}",
                page_id, offset
            ),
            source: e,
        })?;

        Ok(())
    }

    /// Write raw data at a specific offset (for external node data)
    ///
    /// Uses positioned I/O. Extends the file if needed and writes the data
    /// atomically under the write lock.
    pub fn write_data_at_offset(&self, offset: u64, data: &[u8]) -> NativeResult<()> {
        let mut coord = self.file.write();

        let required_len = offset + data.len() as u64;

        write_all_at(&coord.file, data, offset).map_err(|e| NativeBackendError::IoError {
            context: format!("Failed to write external data at offset {}", offset),
            source: e,
        })?;

        coord
            .file
            .sync_all()
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to sync external data".to_string(),
                source: e,
            })?;

        if required_len > coord.cached_size {
            coord.cached_size = required_len;
        }

        Ok(())
    }

    /// Get the current file size
    pub fn file_size(&self) -> u64 {
        self.file.read().cached_size
    }

    /// Flush all pending writes to disk
    pub fn sync_all(&self) -> NativeResult<()> {
        self.file
            .write()
            .file
            .sync_all()
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to sync file".to_string(),
                source: e,
            })
    }

    /// Calculate page offset from page ID
    fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            0
        } else {
            V3_HEADER_SIZE + (page_id - 1) * DEFAULT_PAGE_SIZE
        }
    }

    /// Get the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

/// Write all bytes at the given offset using positioned I/O.
///
/// Loops until all bytes are written, since `write_at` may return fewer
/// bytes than requested. Works on `&File` without changing the file offset.
fn write_all_at(file: &std::fs::File, mut data: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !data.is_empty() {
        let written = positioned_write(file, data, offset)?;
        if written == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "write_at returned 0 bytes",
            ));
        }
        data = &data[written..];
        offset += written as u64;
    }
    Ok(())
}

/// Read exactly `buffer.len()` bytes at the given offset using positioned I/O.
///
/// Loops until the buffer is full, since `read_at` may return fewer bytes
/// than requested. Works on `&File` without changing the file offset.
fn read_all_at(file: &std::fs::File, buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = positioned_read(file, &mut buffer[filled..], offset)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read_at returned 0 bytes before buffer was full",
            ));
        }
        filled += read;
        offset += read as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn positioned_write(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    file.write_at(data, offset)
}

#[cfg(windows)]
fn positioned_write(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    file.seek_write(data, offset)
}

#[cfg(unix)]
fn positioned_read(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn positioned_read(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    file.seek_read(buffer, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_coordinator_create() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.graph");

        let coordinator = FileCoordinator::create(&db_path).unwrap();
        assert_eq!(coordinator.file_size(), 0);
        assert_eq!(coordinator.db_path(), db_path);
    }

    #[test]
    fn test_write_and_read_page() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.graph");

        let coordinator = FileCoordinator::create(&db_path).unwrap();

        // Write page 1
        let data1 = vec![1u8; 4096];
        coordinator.write_page(1, &data1).unwrap();
        assert_eq!(coordinator.file_size(), V3_HEADER_SIZE + 4096);

        // Write page 2
        let data2 = vec![2u8; 4096];
        coordinator.write_page(2, &data2).unwrap();
        assert_eq!(coordinator.file_size(), V3_HEADER_SIZE + 8192);

        // Read back
        let mut buffer = vec![0u8; 4096];
        coordinator.read_page(1, &mut buffer).unwrap();
        assert_eq!(buffer, data1);

        coordinator.read_page(2, &mut buffer).unwrap();
        assert_eq!(buffer, data2);
    }

    #[test]
    fn test_write_extends_file() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.graph");

        let coordinator = FileCoordinator::create(&db_path).unwrap();

        // Write page 100 (should extend file)
        let data = vec![42u8; 4096];
        coordinator.write_page(100, &data).unwrap();

        let expected_size = V3_HEADER_SIZE + (99 * DEFAULT_PAGE_SIZE) + 4096;
        assert_eq!(coordinator.file_size(), expected_size);

        // Read it back
        let mut buffer = vec![0u8; 4096];
        coordinator.read_page(100, &mut buffer).unwrap();
        assert_eq!(buffer, data);
    }
}
