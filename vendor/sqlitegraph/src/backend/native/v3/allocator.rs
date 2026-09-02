//! Page allocator for V3 native backend
//!
//! This module implements dynamic page allocation with free list management
//! for unlimited page capacity in V3 database files.
//!
//! ## Architecture
//!
//! 1. **Free List**: Singly-linked list of free pages stored in page headers
//! 2. **Bitmap**: In-memory tracking of allocated pages for O(1) lookup
//! 3. **Double-Free Prevention**: Page state enum tracks allocation status
//! 4. **Checksums**: All pages have XOR checksums for integrity
//!
//! ## Allocation Strategy
//!
//! - **Allocation**: Check free list first, then append to file
//! - **Deallocation**: Add page to free list, mark in bitmap
//! - **Persistence**: Free list head stored in header (PersistentHeaderV3::free_page_list_head)

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::constants::{DEFAULT_PAGE_SIZE, V3_HEADER_SIZE};
#[cfg(feature = "v3-forensics")]
use crate::backend::native::v3::forensics::FORENSIC_COUNTERS;
use crate::backend::native::v3::header::PersistentHeaderV3;

mod free_page_support;
#[cfg(test)]
mod tests;

pub use free_page_support::FreePageHeader;

/// Page size in bytes (4KB default)
pub const PAGE_SIZE: u64 = DEFAULT_PAGE_SIZE;

/// Maximum pages before bitmap expansion (can grow dynamically)
const INITIAL_BITMAP_PAGES: usize = 1024;

/// Page allocation state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// Page is free and on free list
    Free,
    /// Page is allocated and in use
    Allocated,
    /// Page is pinned (cannot be freed, used during WAL operations)
    Pinned,
}

/// Page allocator for dynamic page allocation
///
/// Manages free list and bitmap tracking for efficient page reuse.
#[derive(Clone)]
pub struct PageAllocator {
    /// Bitmap tracking page allocation state
    /// Grows dynamically as pages are allocated
    bitmap: Vec<bool>,
    /// Free list: stack of deallocated page IDs for reuse (LIFO order)
    free_list: Vec<u64>,
    /// Total pages allocated (including free)
    total_pages: u64,
}

impl PageAllocator {
    /// Create a new page allocator
    ///
    /// # Arguments
    ///
    /// * `header` - V3 persistent header with free_page_list_head and total_pages
    ///
    /// # Returns
    ///
    /// Initialized PageAllocator with sparse bitmap for O(1) initialization
    ///
    /// ## Optimization
    ///
    /// The bitmap is SPARSE: only pages 0 and 1 (reserved) are pre-initialized.
    /// Pages 2+ are "implicitly free" until actually allocated.
    /// This eliminates the O(N) bitmap initialization on open.
    ///
    /// See `get_page_state()` for how pages beyond bitmap.len() are handled.
    pub fn new(header: &PersistentHeaderV3) -> Self {
        // OPTIMIZATION: Sparse bitmap initialization
        // Only allocate bitmap entries for pages 0 and 1 (reserved pages).
        // Pages 2+ are implicitly free until actually allocated (handled by get_page_state).
        // This eliminates O(N) startup cost for large databases.
        let mut bitmap = Vec::with_capacity(INITIAL_BITMAP_PAGES);

        // Page 0: Header page (always allocated/reserved)
        bitmap.push(true);

        // Page 1: First data page (allocated during database creation)
        bitmap.push(true);

        // Pages 2+ are NOT pre-initialized.
        // They are implicitly free until actually allocated.
        // The allocate() method will extend to bitmap as needed.

        Self {
            bitmap,
            free_list: Vec::new(),
            total_pages: header.total_pages,
        }
    }

    /// Allocate a new page
    ///
    /// # Strategy
    ///
    /// 1. Check free list for reusable page
    /// 2. If none, append new page to file
    /// 3. Mark page as allocated in bitmap
    ///
    /// # Returns
    ///
    /// Allocated page_id
    pub fn allocate(&mut self) -> NativeResult<u64> {
        let result = self.allocate_inner()?;
        Ok(result)
    }

    fn allocate_inner(&mut self) -> NativeResult<u64> {
        #[cfg(feature = "v3-forensics")]
        FORENSIC_COUNTERS
            .page_allocate_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Strategy 1: Try to reuse from free list (LIFO stack)
        if let Some(page_id) = self.free_list.pop() {
            // Mark as allocated in bitmap
            let page_idx = page_id as usize;
            if page_idx < self.bitmap.len() {
                self.bitmap[page_idx] = true;
            }

            return Ok(page_id);
        }

        // Strategy 2: Allocate new page at end of file
        // Pages 0 and 1 are reserved (header and first data page)
        // Ensure we start allocating from page 2 if this is a fresh database
        let new_page_id = if self.total_pages < 2 {
            // First allocation - skip reserved pages 0 and 1
            2
        } else {
            self.total_pages
        };

        // Ensure bitmap has capacity
        if new_page_id as usize >= self.bitmap.len() {
            self.bitmap.resize((new_page_id as usize) + 1024, false);
        }

        // Mark as allocated
        self.bitmap[new_page_id as usize] = true;
        self.total_pages = new_page_id + 1;

        Ok(new_page_id)
    }

    /// Deallocate a page (add to free list)
    ///
    /// # Arguments
    ///
    /// * `page_id` - Page ID to free
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Page is already free (double-free detection)
    /// - Page ID 0 (header page cannot be freed)
    pub fn deallocate(&mut self, page_id: u64) -> NativeResult<()> {
        // Validate page_id
        if page_id == 0 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_id".to_string(),
                reason: "Cannot free header page (page 0)".to_string(),
            });
        }

        let page_idx = page_id as usize;

        // Ensure bitmap covers this page for accurate tracking
        if page_idx >= self.bitmap.len() {
            self.bitmap.resize(page_idx + 1, false);
        }

        // Double-free detection: if bitmap already shows free, this is a double-free
        if !self.bitmap[page_idx] {
            return Err(NativeBackendError::CorruptionDetected {
                context: format!("Double-free detected for page {}", page_id),
                source: None,
            });
        }

        // Mark as free in bitmap
        self.bitmap[page_idx] = false;

        // Push onto free list stack
        self.free_list.push(page_id);

        Ok(())
    }

    /// Get page state
    ///
    /// # Arguments
    ///
    /// * `page_id` - Page ID to query
    ///
    /// # Returns
    ///
    /// PageState (Free, Allocated, or Pinned)
    pub fn get_page_state(&self, page_id: u64) -> NativeResult<PageState> {
        if page_id == 0 {
            // Header page is always allocated
            return Ok(PageState::Allocated);
        }

        let page_idx = page_id as usize;

        if page_idx >= self.total_pages as usize {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_id".to_string(),
                reason: format!("Page {} exceeds max pages {}", page_id, self.total_pages),
            });
        }

        if page_idx >= self.bitmap.len() {
            // Page beyond current bitmap is implicitly free (not yet allocated)
            return Ok(PageState::Free);
        }

        let state = if self.bitmap[page_idx] {
            PageState::Allocated
        } else {
            PageState::Free
        };

        Ok(state)
    }

    /// Pin a page (prevent deallocation during WAL operations)
    ///
    /// # Arguments
    ///
    /// * `page_id` - Page ID to pin
    ///
    /// # Note
    ///
    /// Full implementation would track pinned pages separately.
    /// For Phase 64, this is a stub that validates the page exists.
    pub fn pin_page(&mut self, page_id: u64) -> NativeResult<()> {
        let state = self.get_page_state(page_id)?;

        if state == PageState::Free {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_state".to_string(),
                reason: format!("Cannot pin free page {}", page_id),
            });
        }

        // Full implementation would track pinned pages in a HashSet
        // For Phase 64, this validates state only
        Ok(())
    }

    /// Unpin a page (allow deallocation)
    ///
    /// # Arguments
    ///
    /// * `page_id` - Page ID to unpin
    ///
    /// # Note
    ///
    /// Full implementation would remove from pinned set.
    /// For Phase 64, this is a stub that validates the page exists.
    pub fn unpin_page(&mut self, page_id: u64) -> NativeResult<()> {
        let _state = self.get_page_state(page_id)?;

        // Full implementation would remove from pinned set
        // For Phase 64, this validates state only
        Ok(())
    }

    /// Get current allocation statistics
    ///
    /// # Returns
    ///
    /// Tuple of (allocated_pages, free_pages, total_pages)
    pub fn stats(&self) -> (u64, u64, u64) {
        // Count actually allocated pages from bitmap
        let allocated = self.bitmap.iter().filter(|&&x| x).count() as u64;
        let total = self.total_pages;
        // Free pages = pages deallocated and on free list + pages never allocated (beyond total)
        let on_free_list = self.free_list.len() as u64;
        // Pages that have been touched but are free = on_free_list
        // Pages never touched = total_pages..∞ are implicitly free but don't count
        let free = on_free_list;
        (allocated, free, total)
    }

    /// Get free list head (for persistence)
    ///
    /// # Returns
    ///
    /// Top of the free list stack page ID (0 if empty)
    pub fn free_list_head(&self) -> u64 {
        self.free_list.last().copied().unwrap_or(0)
    }

    /// Get total pages (for persistence)
    ///
    /// # Returns
    ///
    /// Total pages allocated
    pub fn total_pages(&self) -> u64 {
        self.total_pages
    }

    /// Calculate page offset in file
    ///
    /// # Arguments
    ///
    /// * `page_id` - Page ID
    ///
    /// # Returns
    ///
    /// Byte offset of page in file
    ///
    /// # Formula
    ///
    /// offset = V3_HEADER_SIZE + (page_id - 1) * PAGE_SIZE
    ///
    /// Note: page_id 0 is the header (not a data page)
    /// Data pages start at page_id = 1
    pub fn page_offset(page_id: u64) -> NativeResult<u64> {
        if page_id == 0 {
            return Ok(0); // Header page
        }

        // Data page: header + (page_id - 1) * page_size
        let offset = V3_HEADER_SIZE + (page_id - 1) * PAGE_SIZE;
        Ok(offset)
    }

    /// Validate page checksum
    ///
    /// # Arguments
    ///
    /// * `page_data` - Raw page bytes
    /// * `stored_checksum` - Checksum from page header
    ///
    /// # Returns
    ///
    /// Ok(()) if checksum valid, Err otherwise
    pub fn validate_checksum(page_data: &[u8], stored_checksum: u64) -> NativeResult<()> {
        // Calculate XOR checksum over page data (excluding checksum field)
        let calculated = free_page_support::xor_checksum(page_data);

        if calculated != stored_checksum {
            return Err(NativeBackendError::InvalidChecksum {
                expected: stored_checksum,
                found: calculated,
            });
        }

        Ok(())
    }
}
