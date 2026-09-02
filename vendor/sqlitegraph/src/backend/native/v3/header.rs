//! Persistent header for V3 native backend
//!
//! This module defines the stable, on-disk header format for V3 files.
//! V3 extends V2's 80-byte header to 112 bytes with B+Tree metadata fields.
//!
//! ## V3 Header Layout (112 bytes)
//!
//! **Preserved V2 fields (bytes 0-79):**
//! - magic[8] - Magic number with version byte
//! - version[4] - Format version
//! - flags[4] - Feature flags
//! - node_count[8] - Total node count
//! - edge_count[8] - Total edge count
//! - schema_version[4] - Schema version
//! - reserved[4] - Reserved field
//! - node_data_offset[8] - **V3: NodeStore's B+Tree root page ID** (was byte offset in V2)
//! - edge_data_offset[8] - **V3: EdgeStore's B+Tree root page ID** (was byte offset in V2)
//! - outgoing_cluster_offset[8] - Outgoing edge cluster offset (V2 compat, byte offset)
//! - incoming_cluster_offset[8] - Incoming edge cluster offset (V2 compat, byte offset)
//! - free_space_offset[8] - Free space management offset (V2 compat, byte offset)
//!
//! **New V3 fields (bytes 80-111):**
//! - root_index_page[8] - Root B+Tree index page ID (primary node index)
//! - free_page_list_head[8] - Head of free page list (0 if none)
//! - total_pages[8] - Total pages allocated
//! - page_size[4] - Page size in bytes (typically 4096)
//! - btree_height[4] - Current B+Tree height
//!
//! ## Important: B+Tree Root Page ID Storage
//!
//! In V3's page-based storage model:
//! - `node_data_offset` stores **NodeStore's B+Tree root page ID** (not a byte offset)
//! - `edge_data_offset` stores **EdgeStore's B+Tree root page ID** (not a byte offset)
//! - Value 0 means uninitialized/no data
//! - Value >= 1 is a valid page ID
//!
//! This reuses the V2 offset fields for compatibility but changes their semantics.
//! The validation in `validate()` reflects this new interpretation.

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::constants;
use crate::backend::native::v3::constants::{
    DEFAULT_PAGE_SIZE, DEFAULT_SCHEMA_VERSION, DEFAULT_V3_FEATURE_FLAGS, MAX_BTREE_HEIGHT,
    V2_MAGIC, V3_FORMAT_VERSION, V3_HEADER_SIZE, V3_MAGIC,
};

mod header_support;
#[cfg(test)]
mod tests;

/// V3 Persistent header that is written to disk
///
/// This header contains ONLY the fields that must be persisted across file closes.
/// Transaction state and rollback metadata are handled separately in runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentHeaderV3 {
    /// Magic number (should be V3_MAGIC)
    pub magic: [u8; 8],
    /// File format version (should be V3_FORMAT_VERSION)
    pub version: u32,
    /// Feature flags bitfield
    pub flags: u32,
    /// Total number of nodes in the file
    pub node_count: u64,
    /// Total number of edges in the file
    pub edge_count: u64,
    /// Schema version
    pub schema_version: u32,
    /// Reserved field (for future use)
    pub reserved: u32,
    /// **V3: NodeStore's B+Tree root page ID** (value 0 = uninitialized, >= 1 = valid page ID)
    /// Note: Field name "node_data_offset" is kept for V2 compatibility, but stores a page ID in V3
    pub node_data_offset: u64,
    /// **V3: EdgeStore's B+Tree root page ID** (value 0 = uninitialized, >= 1 = valid page ID)
    /// Note: Field name "edge_data_offset" is kept for V2 compatibility, but stores a page ID in V3
    pub edge_data_offset: u64,
    /// Offset where outgoing edge clusters begin (V2 compat, still byte offset)
    pub outgoing_cluster_offset: u64,
    /// Offset where incoming edge clusters begin (V2 compat, still byte offset)
    pub incoming_cluster_offset: u64,
    /// Offset where free space management begins (V2 compat, still byte offset)
    pub free_space_offset: u64,

    // --- V3-specific fields (bytes 80-111) ---
    /// Root B+Tree index page ID (0 if tree is empty)
    pub root_index_page: u64,
    /// Head of free page list for page reuse (0 if none)
    pub free_page_list_head: u64,
    /// Total number of pages allocated in the file
    pub total_pages: u64,
    /// Page size in bytes (typically 4096)
    pub page_size: u32,
    /// Current B+Tree height (0 if empty)
    pub btree_height: u32,
}

/// Byte offset specifications for PersistentHeaderV3
pub mod offset {
    // V2 preserved fields (bytes 0-79)
    pub const MAGIC: usize = 0; // bytes 0-7
    pub const VERSION: usize = 8; // bytes 8-11
    pub const FLAGS: usize = 12; // bytes 12-15
    pub const NODE_COUNT: usize = 16; // bytes 16-23
    pub const EDGE_COUNT: usize = 24; // bytes 24-31
    pub const SCHEMA_VERSION: usize = 32; // bytes 32-35
    pub const RESERVED: usize = 36; // bytes 36-39
    pub const NODE_DATA_OFFSET: usize = 40; // bytes 40-47
    pub const EDGE_DATA_OFFSET: usize = 48; // bytes 48-55
    pub const OUTGOING_CLUSTER_OFFSET: usize = 56; // bytes 56-63
    pub const INCOMING_CLUSTER_OFFSET: usize = 64; // bytes 64-71
    pub const FREE_SPACE_OFFSET: usize = 72; // bytes 72-79

    // V3 new fields (bytes 80-111)
    pub const ROOT_INDEX_PAGE: usize = 80; // bytes 80-87
    pub const FREE_PAGE_LIST_HEAD: usize = 88; // bytes 88-95
    pub const TOTAL_PAGES: usize = 96; // bytes 96-103
    pub const PAGE_SIZE: usize = 104; // bytes 104-107
    pub const BTREE_HEIGHT: usize = 108; // bytes 108-111
}

/// Size specifications for PersistentHeaderV3
pub mod size {
    // V2 preserved field sizes
    pub const MAGIC: usize = 8;
    pub const VERSION: usize = 4;
    pub const FLAGS: usize = 4;
    pub const NODE_COUNT: usize = 8;
    pub const EDGE_COUNT: usize = 8;
    pub const SCHEMA_VERSION: usize = 4;
    pub const RESERVED: usize = 4;
    pub const NODE_DATA_OFFSET: usize = 8;
    pub const EDGE_DATA_OFFSET: usize = 8;
    pub const OUTGOING_CLUSTER_OFFSET: usize = 8;
    pub const INCOMING_CLUSTER_OFFSET: usize = 8;
    pub const FREE_SPACE_OFFSET: usize = 8;

    // V3 new field sizes
    pub const ROOT_INDEX_PAGE: usize = 8;
    pub const FREE_PAGE_LIST_HEAD: usize = 8;
    pub const TOTAL_PAGES: usize = 8;
    pub const PAGE_SIZE: usize = 4;
    pub const BTREE_HEIGHT: usize = 4;
}

impl PersistentHeaderV3 {
    /// Create a new persistent header with default V3 values
    pub fn new_v3() -> Self {
        Self {
            magic: V3_MAGIC,
            version: V3_FORMAT_VERSION,
            flags: DEFAULT_V3_FEATURE_FLAGS,
            node_count: 0,
            edge_count: 0,
            schema_version: DEFAULT_SCHEMA_VERSION,
            reserved: 0,
            node_data_offset: 0,
            edge_data_offset: 0,
            outgoing_cluster_offset: 0,
            incoming_cluster_offset: 0,
            free_space_offset: 0,
            root_index_page: 0,
            free_page_list_head: 0,
            total_pages: 0,
            page_size: DEFAULT_PAGE_SIZE as u32,
            btree_height: 0,
        }
    }

    /// Serialize the header to a byte array
    pub fn to_bytes(&self) -> [u8; V3_HEADER_SIZE as usize] {
        let mut bytes = [0u8; V3_HEADER_SIZE as usize];

        // V2 preserved fields
        bytes[offset::MAGIC..offset::MAGIC + size::MAGIC].copy_from_slice(&self.magic);
        bytes[offset::VERSION..offset::VERSION + size::VERSION]
            .copy_from_slice(&self.version.to_be_bytes());
        bytes[offset::FLAGS..offset::FLAGS + size::FLAGS]
            .copy_from_slice(&self.flags.to_be_bytes());
        bytes[offset::NODE_COUNT..offset::NODE_COUNT + size::NODE_COUNT]
            .copy_from_slice(&self.node_count.to_be_bytes());
        bytes[offset::EDGE_COUNT..offset::EDGE_COUNT + size::EDGE_COUNT]
            .copy_from_slice(&self.edge_count.to_be_bytes());
        bytes[offset::SCHEMA_VERSION..offset::SCHEMA_VERSION + size::SCHEMA_VERSION]
            .copy_from_slice(&self.schema_version.to_be_bytes());
        bytes[offset::RESERVED..offset::RESERVED + size::RESERVED]
            .copy_from_slice(&self.reserved.to_be_bytes());
        bytes[offset::NODE_DATA_OFFSET..offset::NODE_DATA_OFFSET + size::NODE_DATA_OFFSET]
            .copy_from_slice(&self.node_data_offset.to_be_bytes());
        bytes[offset::EDGE_DATA_OFFSET..offset::EDGE_DATA_OFFSET + size::EDGE_DATA_OFFSET]
            .copy_from_slice(&self.edge_data_offset.to_be_bytes());
        bytes[offset::OUTGOING_CLUSTER_OFFSET
            ..offset::OUTGOING_CLUSTER_OFFSET + size::OUTGOING_CLUSTER_OFFSET]
            .copy_from_slice(&self.outgoing_cluster_offset.to_be_bytes());
        bytes[offset::INCOMING_CLUSTER_OFFSET
            ..offset::INCOMING_CLUSTER_OFFSET + size::INCOMING_CLUSTER_OFFSET]
            .copy_from_slice(&self.incoming_cluster_offset.to_be_bytes());
        bytes[offset::FREE_SPACE_OFFSET..offset::FREE_SPACE_OFFSET + size::FREE_SPACE_OFFSET]
            .copy_from_slice(&self.free_space_offset.to_be_bytes());

        // V3 new fields
        bytes[offset::ROOT_INDEX_PAGE..offset::ROOT_INDEX_PAGE + size::ROOT_INDEX_PAGE]
            .copy_from_slice(&self.root_index_page.to_be_bytes());
        bytes[offset::FREE_PAGE_LIST_HEAD..offset::FREE_PAGE_LIST_HEAD + size::FREE_PAGE_LIST_HEAD]
            .copy_from_slice(&self.free_page_list_head.to_be_bytes());
        bytes[offset::TOTAL_PAGES..offset::TOTAL_PAGES + size::TOTAL_PAGES]
            .copy_from_slice(&self.total_pages.to_be_bytes());
        bytes[offset::PAGE_SIZE..offset::PAGE_SIZE + size::PAGE_SIZE]
            .copy_from_slice(&self.page_size.to_be_bytes());
        bytes[offset::BTREE_HEIGHT..offset::BTREE_HEIGHT + size::BTREE_HEIGHT]
            .copy_from_slice(&self.btree_height.to_be_bytes());

        bytes
    }

    /// Validate persistent header for consistency
    pub fn validate(&self) -> NativeResult<()> {
        // Check magic number - must be V3_MAGIC
        if self.magic != V3_MAGIC {
            // Check if it's a V2 header for better error message
            if self.magic == V2_MAGIC {
                return Err(NativeBackendError::UnsupportedVersion {
                    version: 2,
                    supported_version: 3,
                });
            }
            return Err(NativeBackendError::InvalidMagic {
                expected: u64::from_be_bytes(V3_MAGIC),
                found: u64::from_be_bytes(self.magic),
            });
        }

        // Check version - must be V3_FORMAT_VERSION
        if self.version != V3_FORMAT_VERSION {
            return Err(NativeBackendError::UnsupportedVersion {
                version: self.version,
                supported_version: V3_FORMAT_VERSION,
            });
        }

        // Check V2/V3 common flags
        let required_flags = constants::FLAG_V2_FRAMED_RECORDS | constants::FLAG_V2_ATOMIC_COMMIT;
        if (self.flags & required_flags) != required_flags {
            return Err(NativeBackendError::InvalidHeader {
                field: "flags".to_string(),
                reason: format!(
                    "missing required flags: expected {:x}, found {:x}",
                    required_flags, self.flags
                ),
            });
        }

        // Check V3-specific flags
        let v3_required = crate::backend::native::v3::constants::v3_flags::FLAG_V3_BTREE_INDEX;
        if (self.flags & v3_required) != v3_required {
            return Err(NativeBackendError::InvalidHeader {
                field: "flags".to_string(),
                reason: "V3 files must have B+Tree index flag set".to_string(),
            });
        }

        // V3 B+TREE INDEXING MODEL:
        // In V3's page-based storage, node_data_offset and edge_data_offset
        // store B+Tree root page IDs (not byte offsets like the field names suggest).
        // - Value 0 means uninitialized/no data
        // - Value >= 1 is a valid page ID
        //
        // The original monolithic file layout with byte offsets has been replaced
        // by page-based B+Tree indexing, but we reuse these fields for compatibility.

        // Validate node_data_offset as page ID (0 or >= 1)
        if self.node_data_offset != 0 && self.node_data_offset < 1 {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_data_offset".to_string(),
                reason: "must be 0 (uninitialized) or >= 1 (valid page ID)".to_string(),
            });
        }

        // Validate edge_data_offset as page ID (0 or >= 1)
        if self.edge_data_offset != 0 && self.edge_data_offset < 1 {
            return Err(NativeBackendError::InvalidHeader {
                field: "edge_data_offset".to_string(),
                reason: "must be 0 (uninitialized) or >= 1 (valid page ID)".to_string(),
            });
        }

        // Validate cluster offset ordering (these are still byte offsets for V2 compat)
        // The cluster offsets are compared against V3_HEADER_SIZE as byte offsets
        if self.outgoing_cluster_offset > 0 && self.outgoing_cluster_offset < V3_HEADER_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "outgoing_cluster_offset".to_string(),
                reason: format!("must be 0 or >= header_size ({})", V3_HEADER_SIZE),
            });
        }

        if self.incoming_cluster_offset > 0
            && self.incoming_cluster_offset < self.outgoing_cluster_offset
        {
            return Err(NativeBackendError::InvalidHeader {
                field: "incoming_cluster_offset".to_string(),
                reason: "must be >= outgoing_cluster_offset".to_string(),
            });
        }

        if self.free_space_offset > 0 && self.free_space_offset < self.incoming_cluster_offset {
            return Err(NativeBackendError::InvalidHeader {
                field: "free_space_offset".to_string(),
                reason: "must be >= incoming_cluster_offset".to_string(),
            });
        }

        // Validate V3-specific fields
        if self.page_size != 4096 && self.page_size != 8192 && self.page_size != 16384 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_size".to_string(),
                reason: "must be 4096, 8192, or 16384".to_string(),
            });
        }

        if self.btree_height > MAX_BTREE_HEIGHT {
            return Err(NativeBackendError::InvalidHeader {
                field: "btree_height".to_string(),
                reason: format!("must be <= {}", MAX_BTREE_HEIGHT),
            });
        }

        Ok(())
    }

    /// Parse a header from a byte slice
    pub fn from_bytes(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < V3_HEADER_SIZE as usize {
            return Err(NativeBackendError::InvalidHeader {
                field: "bytes".to_string(),
                reason: format!(
                    "insufficient bytes: expected {}, found {}",
                    V3_HEADER_SIZE,
                    bytes.len()
                ),
            });
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[offset::MAGIC..offset::MAGIC + size::MAGIC]);

        let version =
            header_support::read_u32_be(bytes, offset::VERSION, size::VERSION, "version")?;
        let flags = header_support::read_u32_be(bytes, offset::FLAGS, size::FLAGS, "flags")?;
        let node_count =
            header_support::read_u64_be(bytes, offset::NODE_COUNT, size::NODE_COUNT, "node_count")?;
        let edge_count =
            header_support::read_u64_be(bytes, offset::EDGE_COUNT, size::EDGE_COUNT, "edge_count")?;
        let schema_version = header_support::read_u32_be(
            bytes,
            offset::SCHEMA_VERSION,
            size::SCHEMA_VERSION,
            "schema_version",
        )?;
        let reserved =
            header_support::read_u32_be(bytes, offset::RESERVED, size::RESERVED, "reserved")?;
        let node_data_offset = header_support::read_u64_be(
            bytes,
            offset::NODE_DATA_OFFSET,
            size::NODE_DATA_OFFSET,
            "node_data_offset",
        )?;
        let edge_data_offset = header_support::read_u64_be(
            bytes,
            offset::EDGE_DATA_OFFSET,
            size::EDGE_DATA_OFFSET,
            "edge_data_offset",
        )?;
        let outgoing_cluster_offset = header_support::read_u64_be(
            bytes,
            offset::OUTGOING_CLUSTER_OFFSET,
            size::OUTGOING_CLUSTER_OFFSET,
            "outgoing_cluster_offset",
        )?;
        let incoming_cluster_offset = header_support::read_u64_be(
            bytes,
            offset::INCOMING_CLUSTER_OFFSET,
            size::INCOMING_CLUSTER_OFFSET,
            "incoming_cluster_offset",
        )?;
        let free_space_offset = header_support::read_u64_be(
            bytes,
            offset::FREE_SPACE_OFFSET,
            size::FREE_SPACE_OFFSET,
            "free_space_offset",
        )?;

        // V3 fields
        let root_index_page = header_support::read_u64_be(
            bytes,
            offset::ROOT_INDEX_PAGE,
            size::ROOT_INDEX_PAGE,
            "root_index_page",
        )?;
        let free_page_list_head = header_support::read_u64_be(
            bytes,
            offset::FREE_PAGE_LIST_HEAD,
            size::FREE_PAGE_LIST_HEAD,
            "free_page_list_head",
        )?;
        let total_pages = header_support::read_u64_be(
            bytes,
            offset::TOTAL_PAGES,
            size::TOTAL_PAGES,
            "total_pages",
        )?;
        let page_size =
            header_support::read_u32_be(bytes, offset::PAGE_SIZE, size::PAGE_SIZE, "page_size")?;
        let btree_height = header_support::read_u32_be(
            bytes,
            offset::BTREE_HEIGHT,
            size::BTREE_HEIGHT,
            "btree_height",
        )?;

        Ok(Self {
            magic,
            version,
            flags,
            node_count,
            edge_count,
            schema_version,
            reserved,
            node_data_offset,
            edge_data_offset,
            outgoing_cluster_offset,
            incoming_cluster_offset,
            free_space_offset,
            root_index_page,
            free_page_list_head,
            total_pages,
            page_size,
            btree_height,
        })
    }

    /// Detect the version of a header from raw bytes
    pub fn detect_version(bytes: &[u8]) -> NativeResult<u32> {
        if bytes.len() < 8 {
            return Err(NativeBackendError::InvalidHeader {
                field: "bytes".to_string(),
                reason: "insufficient bytes for magic detection".to_string(),
            });
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);

        if magic == V3_MAGIC {
            Ok(3)
        } else if magic == V2_MAGIC || magic == constants::MAGIC_BYTES {
            Ok(2)
        } else {
            Err(NativeBackendError::InvalidMagic {
                expected: u64::from_be_bytes(V3_MAGIC),
                found: u64::from_be_bytes(magic),
            })
        }
    }
}

/// Size of PersistentHeaderV3 - calculated from field sizes
pub const PERSISTENT_HEADER_V3_SIZE: usize = size::MAGIC
    + size::VERSION
    + size::FLAGS
    + size::NODE_COUNT
    + size::EDGE_COUNT
    + size::SCHEMA_VERSION
    + size::RESERVED
    + size::NODE_DATA_OFFSET
    + size::EDGE_DATA_OFFSET
    + size::OUTGOING_CLUSTER_OFFSET
    + size::INCOMING_CLUSTER_OFFSET
    + size::FREE_SPACE_OFFSET
    + size::ROOT_INDEX_PAGE
    + size::FREE_PAGE_LIST_HEAD
    + size::TOTAL_PAGES
    + size::PAGE_SIZE
    + size::BTREE_HEIGHT;

// Compile-time assertion - ensure persistent header matches V3_HEADER_SIZE
const _: [(); 112] = [(); PERSISTENT_HEADER_V3_SIZE];

// Compile-time assertion - ensure mem::size_of matches calculated size
const _: [(); 112] = [(); std::mem::size_of::<PersistentHeaderV3>()];
