//! B+Tree index page structure for V3 native backend
//!
//! This module defines the B+Tree index page structure used for node lookup.
//! B+Tree provides O(log n) lookup with unlimited node capacity.
//!
//! ## Page Structure
//!
//! - **Internal pages**: Contain split keys + child page pointers
//! - **Leaf pages**: Contain (node_id, page_id) entries + next_leaf pointer
//! - Both page types: 4KB total with 32-byte header + 4064 bytes usable
//!
//! ## Capacity
//!
//! - Max keys per internal page: 254 (fanout 255)
//! - Max entries per leaf page: 254 (node_id, page_id) pairs
//! - Tree height: max 4 levels for 4B nodes (128-ary branching)

pub mod page;

// Re-export index page types
pub use page::{IndexPage, IndexPageType};

/// Index page constants
pub mod constants {

    /// Page header size in bytes (page_id, is_leaf, count, checksum, padding)
    pub const PAGE_HEADER_SIZE: usize = 32;

    /// Usable page size after header
    pub const USABLE_SIZE: usize = 4096 - PAGE_HEADER_SIZE;

    /// Maximum keys per internal page
    pub const MAX_KEYS: usize = 253;

    /// Maximum entries per leaf page
    pub const MAX_ENTRIES: usize = 254;

    /// Maximum children per internal page (keys + 1)
    pub const MAX_CHILDREN: usize = MAX_KEYS + 1;

    /// Key size in bytes (u64 node_id)
    pub const KEY_SIZE: usize = 8;

    /// Page ID size in bytes (u64)
    pub const PAGE_ID_SIZE: usize = 8;

    /// Entry size in bytes (node_id + page_id)
    pub const ENTRY_SIZE: usize = KEY_SIZE + PAGE_ID_SIZE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_constants() {
        assert_eq!(constants::PAGE_HEADER_SIZE, 32);
        assert_eq!(constants::USABLE_SIZE, 4064);
        assert_eq!(constants::MAX_KEYS, 253);
        assert_eq!(constants::MAX_ENTRIES, 254);
        assert_eq!(constants::MAX_CHILDREN, 254);
    }
}
