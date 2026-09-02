//! B+Tree index page structure
//!
//! Defines the IndexPage enum with Internal and Leaf variants for B+Tree node index.
//! Uses big-endian serialization for cross-platform compatibility.

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::constants as v3_constants;

mod serde_support;
#[cfg(test)]
mod tests;

/// Page header offset and size constants
pub mod constants {
    /// Page header size in bytes
    pub const PAGE_HEADER_SIZE: usize = 32;

    /// Page ID offset within header
    pub const PAGE_ID_OFFSET: usize = 0;

    /// Is leaf flag offset (1 byte, 1 = leaf, 0 = internal)
    pub const IS_LEAF_OFFSET: usize = 8;

    /// Is root flag offset (1 byte, 1 = root, 0 = non-root)
    pub const IS_ROOT_OFFSET: usize = 9;

    /// Count offset (number of keys or entries, u16)
    pub const COUNT_OFFSET: usize = 10;

    /// Checksum offset (u32)
    pub const CHECKSUM_OFFSET: usize = 12;

    /// Reserved/padding offset
    pub const PADDING_OFFSET: usize = 16;

    /// Key/data start offset after header
    pub const DATA_START_OFFSET: usize = PAGE_HEADER_SIZE;
}

/// Maximum keys per internal page (252 allows 125/126 splits after separator)
/// Page layout: 32 header + 252*8 keys + 253*8 children = 32 + 2016 + 2024 = 4072 bytes < 4096
pub const MAX_KEYS: usize = 252;

/// Minimum keys for non-root internal nodes (floor(MAX_KEYS / 2))
/// When splitting 252 keys at index 125: old node gets 125, sibling gets 126
pub const MIN_KEYS: usize = 125;

/// Maximum entries per leaf page (same as MAX_KEYS for consistency)
/// Page layout: 32 header + 252*16 entries + 8 next_leaf = 32 + 4032 + 8 = 4072 bytes < 4096
pub const MAX_ENTRIES: usize = 252;

/// Minimum entries for non-root leaf nodes (floor(MAX_ENTRIES / 2))
/// When splitting 252 entries at index 126: old node gets 126, sibling gets 126
pub const MIN_ENTRIES: usize = 126;

/// Maximum children per internal page
pub const MAX_CHILDREN: usize = MAX_KEYS + 1;

/// Key size in bytes (u64 node_id)
pub const KEY_SIZE: usize = 8;

/// Page ID size in bytes (u64)
pub const PAGE_ID_SIZE: usize = 8;

/// Entry size in bytes (node_id + page_id)
pub const ENTRY_SIZE: usize = KEY_SIZE + PAGE_ID_SIZE;

/// B+Tree index page type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexPageType {
    /// Internal node with keys and child pointers
    Internal,
    /// Leaf node with (node_id, page_id) entries
    Leaf,
}

/// B+Tree index page
///
/// Internal pages contain split keys and child page pointers for tree navigation.
/// Leaf pages contain the actual (node_id, page_id) mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexPage {
    /// Internal index page with keys and child pointers
    Internal {
        /// Page ID for this page
        page_id: u64,
        /// Split keys (max 252) - keys[i] is the minimum key in child i+1
        keys: Vec<u64>,
        /// Child page IDs (max 253, one more than keys)
        children: Vec<u64>,
        /// Page checksum for validation
        checksum: u32,
        /// Is this the root page (exempt from MIN_KEYS constraint)
        is_root: bool,
    },
    /// Leaf index page with node ID to page ID mappings
    Leaf {
        /// Page ID for this page
        page_id: u64,
        /// (node_id, page_id) entries (max 252)
        entries: Vec<(u64, u64)>,
        /// Next leaf page ID (0 if none)
        next_leaf: u64,
        /// Page checksum for validation
        checksum: u32,
        /// Is this the root page (exempt from MIN_ENTRIES constraint)
        is_root: bool,
    },
}

impl IndexPage {
    /// Get the page ID for this page
    pub fn page_id(&self) -> u64 {
        match self {
            IndexPage::Internal { page_id, .. } => *page_id,
            IndexPage::Leaf { page_id, .. } => *page_id,
        }
    }

    /// Get the page type
    pub fn page_type(&self) -> IndexPageType {
        match self {
            IndexPage::Internal { .. } => IndexPageType::Internal,
            IndexPage::Leaf { .. } => IndexPageType::Leaf,
        }
    }

    /// Get the number of keys (internal) or entries (leaf)
    pub fn count(&self) -> usize {
        match self {
            IndexPage::Internal { keys, .. } => keys.len(),
            IndexPage::Leaf { entries, .. } => entries.len(),
        }
    }

    /// Create a new empty internal page (non-root by default)
    pub fn new_internal(page_id: u64) -> Self {
        IndexPage::Internal {
            page_id,
            keys: Vec::new(),
            children: Vec::new(),
            checksum: 0,
            is_root: false,
        }
    }

    /// Create a new empty internal page as root
    pub fn new_internal_root(page_id: u64) -> Self {
        IndexPage::Internal {
            page_id,
            keys: Vec::new(),
            children: Vec::new(),
            checksum: 0,
            is_root: true,
        }
    }

    /// Create a new empty leaf page (non-root by default)
    pub fn new_leaf(page_id: u64) -> Self {
        IndexPage::Leaf {
            page_id,
            entries: Vec::new(),
            next_leaf: 0,
            checksum: 0,
            is_root: false,
        }
    }

    /// Create a new empty leaf page as root
    pub fn new_leaf_root(page_id: u64) -> Self {
        IndexPage::Leaf {
            page_id,
            entries: Vec::new(),
            next_leaf: 0,
            checksum: 0,
            is_root: true,
        }
    }

    /// Check if this page is the root
    pub fn is_root(&self) -> bool {
        match self {
            IndexPage::Internal { is_root, .. } => *is_root,
            IndexPage::Leaf { is_root, .. } => *is_root,
        }
    }

    /// Set the root flag
    pub fn set_root(&mut self, root: bool) {
        match self {
            IndexPage::Internal { is_root, .. } => *is_root = root,
            IndexPage::Leaf { is_root, .. } => *is_root = root,
        }
    }

    /// Check if internal page is full (at max capacity)
    pub fn is_full_internal(&self) -> bool {
        match self {
            IndexPage::Internal { keys, .. } => keys.len() >= MAX_KEYS,
            _ => false,
        }
    }

    /// Check if leaf page is full (at max capacity)
    pub fn is_full_leaf(&self) -> bool {
        match self {
            IndexPage::Leaf { entries, .. } => entries.len() >= MAX_ENTRIES,
            _ => false,
        }
    }

    /// Check if internal page needs splitting (has exceeded capacity)
    pub fn needs_split_internal(&self) -> bool {
        match self {
            IndexPage::Internal { keys, .. } => keys.len() >= MAX_KEYS,
            _ => false,
        }
    }

    /// Check if leaf page needs splitting (has exceeded capacity)
    pub fn needs_split_leaf(&self) -> bool {
        match self {
            IndexPage::Leaf { entries, .. } => entries.len() >= MAX_ENTRIES,
            _ => false,
        }
    }

    /// Search for a node_id in a leaf page using binary search
    ///
    /// Returns the index where the node_id is found, or Err(idx) with the insertion point.
    pub fn binary_search_leaf(entries: &[(u64, u64)], target: u64) -> Result<usize, usize> {
        entries.binary_search_by_key(&target, |&(node_id, _)| node_id)
    }

    /// Find the appropriate child index for a key in an internal page
    ///
    /// Returns the index of the child that should contain the target key.
    pub fn find_child_index(keys: &[u64], target: u64) -> usize {
        match keys.binary_search(&target) {
            Ok(idx) => idx + 1,
            Err(idx) => idx,
        }
    }
}
