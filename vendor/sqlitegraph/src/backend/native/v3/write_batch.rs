//! Write Batch Implementation for V3
//!
//! Provides transaction-like batching for B+Tree page writes.
//! Multiple page mutations are buffered in memory and flushed
//! with a single fsync at commit time.
//!
//! This is the minimal fix for V3's write performance issue.
//! Before: 138× slower than SQLite (per-insert fsync)
//! After: Target 2-3× slower than SQLite (batched fsync)

use std::collections::HashMap;
use std::path::Path;

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::index::IndexPage;

/// A write batch that buffers page mutations in memory
///
/// Use this to amortize fsync costs across multiple operations:
/// ```ignore
/// use sqlitegraph::backend::native::v3::write_batch::WriteBatch;
/// use sqlitegraph::backend::native::v3::index::IndexPage;
///
/// let mut batch = WriteBatch::new();
/// batch.stage_page(IndexPage::new_leaf(1))?;
/// batch.stage_page(IndexPage::new_leaf(2))?;
/// batch.stage_page(IndexPage::new_leaf(3))?;
/// batch.commit(std::path::Path::new("/tmp/db.graph"))?; // Single fsync for all pages
/// ```
#[derive(Debug)]
pub struct WriteBatch {
    /// Pages staged for writing (page_id -> page)
    dirty_pages: HashMap<u64, IndexPage>,
    /// Whether this batch has been committed
    committed: bool,
}

impl WriteBatch {
    /// Create a new empty write batch
    pub fn new() -> Self {
        Self {
            dirty_pages: HashMap::new(),
            committed: false,
        }
    }

    /// Stage a page for writing (in-memory only)
    ///
    /// The page is added to the batch but not written to disk yet.
    /// Multiple writes to the same page_id will overwrite.
    ///
    /// # Arguments
    ///
    /// * `page` - The IndexPage to stage
    ///
    /// # Errors
    ///
    /// Returns error if batch is already committed
    pub fn stage_page(&mut self, page: IndexPage) -> NativeResult<()> {
        if self.committed {
            return Err(NativeBackendError::InvalidOperation {
                context: "Cannot stage page to already-committed batch".to_string(),
            });
        }

        let page_id = page.page_id();
        self.dirty_pages.insert(page_id, page);
        Ok(())
    }

    /// Get the number of pages in this batch
    pub fn len(&self) -> usize {
        self.dirty_pages.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.dirty_pages.is_empty()
    }

    /// Check if this batch has been committed
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Commit all staged pages to disk in a single operation
    ///
    /// This writes all pages and performs exactly ONE fsync.
    /// On failure, partial writes may have occurred but pages
    /// remain in dirty state (can be retried).
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the database file
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Batch is empty (nothing to commit)
    /// - Batch already committed
    /// - I/O error during write
    pub fn commit(mut self, db_path: &Path) -> NativeResult<()> {
        if self.committed {
            return Err(NativeBackendError::InvalidOperation {
                context: "Batch already committed".to_string(),
            });
        }

        if self.dirty_pages.is_empty() {
            return Err(NativeBackendError::InvalidOperation {
                context: "Cannot commit empty batch".to_string(),
            });
        }

        // Write all pages
        self.write_pages_to_disk(db_path)?;

        self.committed = true;
        Ok(())
    }

    /// Write all pages to disk with single fsync
    fn write_pages_to_disk(&self, db_path: &Path) -> NativeResult<()> {
        use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
        use std::fs::OpenOptions;
        use std::io::{Seek, SeekFrom, Write};

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(db_path)
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to open db for batch write: {}", db_path.display()),
                source: e,
            })?;

        // Write all pages
        for (page_id, page) in &self.dirty_pages {
            // Skip page 0 (header)
            if *page_id == 0 {
                continue;
            }

            let offset = V3_HEADER_SIZE + (page_id - 1) * DEFAULT_PAGE_SIZE;
            let page_bytes = page.pack()?;

            file.seek(SeekFrom::Start(offset))
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to seek to page {}", page_id),
                    source: e,
                })?;

            file.write_all(&page_bytes)
                .map_err(|e| NativeBackendError::IoError {
                    context: format!("Failed to write page {}", page_id),
                    source: e,
                })?;
        }

        // Single fsync for entire batch
        file.sync_data().map_err(|e| NativeBackendError::IoError {
            context: "Failed to sync batch write".to_string(),
            source: e,
        })?;

        Ok(())
    }

    /// Get a reference to a staged page (for testing)
    #[cfg(test)]
    pub fn get_page(&self, page_id: u64) -> Option<&IndexPage> {
        self.dirty_pages.get(&page_id)
    }
}

impl Default for WriteBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::native::v3::index::IndexPage;
    use tempfile::TempDir;

    fn create_test_db() -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.graph");

        // Create minimal V3 database file
        use crate::backend::native::v3::header::PersistentHeaderV3;
        use std::fs::File;
        use std::io::Write;

        let header = PersistentHeaderV3::new_v3();
        let header_bytes = header.to_bytes();

        let mut file = File::create(&db_path).unwrap();
        file.write_all(&header_bytes).unwrap();
        // Pre-allocate space for a few pages
        file.set_len(4096 * 10).unwrap();

        (temp, db_path)
    }

    #[test]
    fn test_write_batch_new_is_empty() {
        let batch = WriteBatch::new();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        assert!(!batch.is_committed());
    }

    #[test]
    fn test_stage_page_increases_count() {
        let mut batch = WriteBatch::new();
        let page = IndexPage::new_leaf(1);

        batch.stage_page(page).unwrap();

        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_stage_same_page_twice_overwrites() {
        let mut batch = WriteBatch::new();
        let page1 = IndexPage::new_leaf(1);
        let page2 = IndexPage::new_leaf(1); // Same page_id

        batch.stage_page(page1).unwrap();
        batch.stage_page(page2.clone()).unwrap();

        // Should still be 1 page (overwritten)
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.get_page(1).unwrap().page_id(), 1);
    }

    #[test]
    fn test_cannot_commit_empty_batch() {
        let (_temp, db_path) = create_test_db();
        let batch = WriteBatch::new();

        let result = batch.commit(&db_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_commit_multiple_pages() {
        let (_temp, db_path) = create_test_db();
        let mut batch = WriteBatch::new();

        // Stage 5 pages
        for i in 1..=5 {
            let page = IndexPage::new_leaf(i);
            batch.stage_page(page).unwrap();
        }

        assert_eq!(batch.len(), 5);

        // Commit should succeed
        batch.commit(&db_path).unwrap();
    }

    #[test]
    fn test_commit_skips_page_zero() {
        let (_temp, db_path) = create_test_db();
        let mut batch = WriteBatch::new();

        // Try to stage page 0 (should be skipped on commit)
        let page0 = IndexPage::new_leaf(0);
        let page1 = IndexPage::new_leaf(1);

        batch.stage_page(page0).unwrap();
        batch.stage_page(page1).unwrap();

        // Commit should succeed (page 0 skipped)
        batch.commit(&db_path).unwrap();
    }
}
