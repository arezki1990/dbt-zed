//! V3 Write-Ahead Logging (WAL) for B+Tree page operations.
//!
//! This module implements WAL for V3's page-based storage, providing:
//! - Atomic page operations (allocate, free, write)
//! - B+Tree split tracking
//! - Crash recovery from WAL
//! - Checkpoint integration with BTreeManager
//!
//! # Architecture
//!
//! V3 WAL is designed for page-level operations rather than node/edge operations:
//! - **Page Allocation**: Track new page assignments
//! - **Page Freeing**: Track deallocation for reuse
//! - **Page Writes**: Track data modifications with checksums
//! - **B+Tree Splits**: Track tree restructuring during growth
//! - **Checkpoints**: Persist root page and tree state
//!
//! # WAL File Format
//!
//! ```text
//! [V3WALHeader - 64 bytes]
//!   magic: [u8; 8]       // "V3WAL\0\0"
//!   version: u32            // WAL format version
//!   page_size: u32          // Page size (usually 4096)
//!   created_at: u64         // Creation timestamp
//!   current_lsn: u64        // Current log sequence number
//!   committed_lsn: u64       // Last committed LSN
//!   checkpointed_lsn: u64    // Last checkpointed LSN
//!   reserved: [u64; 3]     // Future use
//!
//! [V3WALRecord 1]
//! [V3WALRecord 2]
//! ...
//! ```
//!
//! # Recovery Process
//!
//! 1. Open WAL file and read header
//! 2. Sequential read and apply records:
//!    - PageAllocate: Allocate page via PageAllocator
//!    - PageFree: Free page via PageAllocator
//!    - PageWrite: Write data to page, verify checksum
//!    - BTreeSplit: Update B+Tree structure
//!    - Checkpoint: Persist header state (root page, height, etc.)
//! 3. Skip corrupt/invalid records
//! 4. After replay, truncate WAL at checkpoint point

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;
use crate::backend::native::v3::constants::checksum;
use crate::backend::native::v3::header::PersistentHeaderV3;
use bincode;
use serde::{Deserialize, Serialize};

mod checkpoint;
mod header_support;
mod record_support;
mod recovery;
#[cfg(test)]
mod tests;
mod writer;

pub use checkpoint::{read_kv_checkpoint, write_kv_checkpoint};
pub use recovery::{WALRecovery, WALRecoveryStats};
pub use writer::{V3WALPaths, WALWriter};

/// V3 WAL file magic bytes
pub const V3_WAL_MAGIC: [u8; 8] = [b'V', b'3', b'W', b'A', b'L', 0, 0, 0];

/// V3 WAL format version
pub const V3_WAL_VERSION: u32 = 1;

/// V3 WAL header size in bytes
pub const V3_WAL_HEADER_SIZE: usize = 64;

/// Maximum WAL record size (1MB - safety limit)
pub const MAX_RECORD_SIZE: usize = 1024 * 1024;

/// Log Sequence Number (LSN) representing beginning of WAL
pub const LSN_BEGIN: u64 = 1;

/// Log Sequence Number (LSN) representing invalid/uninitialized position
pub const LSN_INVALID: u64 = 0;

/// Check if an LSN is valid
#[inline]
pub fn lsn_is_valid(lsn: u64) -> bool {
    lsn >= LSN_BEGIN
}

/// Get the next LSN
#[inline]
pub fn lsn_next(lsn: u64) -> u64 {
    lsn.wrapping_add(1)
}

/// V3 WAL file header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct V3WALHeader {
    /// Magic bytes for identification
    pub magic: [u8; 8],
    /// WAL format version
    pub version: u32,
    /// Page size in bytes (usually 4096)
    pub page_size: u32,
    /// Creation timestamp (Unix epoch)
    pub created_at: u64,
    /// Current log sequence number
    pub current_lsn: u64,
    /// Last committed LSN
    pub committed_lsn: u64,
    /// Last checkpointed LSN
    pub checkpointed_lsn: u64,
    /// Reserved for future use
    pub reserved: [u64; 3],
}

/// V3 WAL record types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum V3WALRecordType {
    /// Page allocation - new page assigned from PageAllocator
    PageAllocate = 1,

    /// Page deallocation - page returned to free list
    PageFree = 2,

    /// Page write - data written to page with checksum
    PageWrite = 3,

    /// B+Tree split - page split during growth
    BTreeSplit = 4,

    /// Checkpoint - persist tree root and header state
    Checkpoint = 5,

    /// Transaction begin marker
    TransactionBegin = 6,

    /// Transaction commit marker
    TransactionCommit = 7,

    /// Transaction rollback marker
    TransactionRollback = 8,

    /// KV Set - store a key-value pair
    KvSet = 9,

    /// KV Delete - delete a key-value pair
    KvDelete = 10,

    /// KV Tombstone - mark key as deleted (for MVCC)
    KvTombstone = 11,

    /// Edge Insert - insert edge into edge cluster
    EdgeInsert = 12,
}

/// V3 WAL record for page-level operations
///
/// Each record represents a single operation that modifies the database state.
/// Records are written sequentially to the WAL file and can be replayed
/// during recovery to restore database state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum V3WALRecord {
    /// Page allocation - assign new page from PageAllocator
    PageAllocate {
        /// Log sequence number
        lsn: u64,
        /// Newly allocated page ID
        page_id: u64,
        /// Timestamp of allocation
        timestamp: u64,
    },

    /// Page deallocation - return page to free list
    PageFree {
        /// Log sequence number
        lsn: u64,
        /// Page ID being freed
        page_id: u64,
        /// Checksum of page before free (for validation)
        checksum: u32,
        /// Timestamp of deallocation
        timestamp: u64,
    },

    /// Page write - write data to page
    PageWrite {
        /// Log sequence number
        lsn: u64,
        /// Target page ID
        page_id: u64,
        /// Offset within page (0-4095)
        offset: u32,
        /// Data being written
        data: Vec<u8>,
        /// Checksum of data
        checksum: u32,
        /// Timestamp of write
        timestamp: u64,
    },

    /// B+Tree page split
    BTreeSplit {
        /// Log sequence number
        lsn: u64,
        /// Original page ID being split
        original_page_id: u64,
        /// New page ID created from split
        new_page_id: u64,
        /// Split key (first key in new page)
        split_key: u64,
        /// Page type being split (internal or leaf)
        page_type: u8, // 0 = internal, 1 = leaf
        /// Timestamp of split
        timestamp: u64,
    },

    /// Checkpoint - persist database state
    Checkpoint {
        /// Log sequence number
        lsn: u64,
        /// Root B+Tree page ID
        root_page_id: u64,
        /// Total pages in database
        total_pages: u64,
        /// B+Tree height
        btree_height: u32,
        /// Free page list head
        free_page_list_head: u64,
        /// Full header snapshot for recovery
        header_snapshot: Vec<u8>, // Serialized PersistentHeaderV3
        /// Timestamp of checkpoint
        timestamp: u64,
    },

    /// Transaction begin marker
    TransactionBegin {
        /// Transaction ID
        tx_id: u64,
        /// Log sequence number
        lsn: u64,
        /// Timestamp
        timestamp: u64,
    },

    /// Transaction commit marker
    TransactionCommit {
        /// Transaction ID
        tx_id: u64,
        /// Log sequence number
        lsn: u64,
        /// Timestamp
        timestamp: u64,
    },

    /// Transaction rollback marker
    TransactionRollback {
        /// Transaction ID
        tx_id: u64,
        /// Log sequence number
        lsn: u64,
        /// Timestamp
        timestamp: u64,
    },

    /// KV Set operation
    KvSet {
        /// Log sequence number
        lsn: u64,
        /// Key bytes
        key: Vec<u8>,
        /// Value bytes (serialized)
        value_bytes: Vec<u8>,
        /// Value type tag
        value_type: u8,
        /// Optional TTL in seconds
        ttl_seconds: Option<u64>,
        /// Timestamp
        timestamp: u64,
    },

    /// KV Delete operation
    KvDelete {
        /// Log sequence number
        lsn: u64,
        /// Key bytes
        key: Vec<u8>,
        /// Timestamp
        timestamp: u64,
    },

    /// KV Tombstone for MVCC
    KvTombstone {
        /// Log sequence number
        lsn: u64,
        /// Key bytes
        key: Vec<u8>,
        /// Previous value (for rollback)
        old_value_bytes: Option<Vec<u8>>,
        /// Previous value type
        old_value_type: u8,
        /// Timestamp
        timestamp: u64,
    },

    /// Edge Insert - insert edge into edge cluster
    EdgeInsert {
        /// Log sequence number
        lsn: u64,
        /// Source node ID
        src: i64,
        /// Destination node ID
        dst: i64,
        /// Edge direction (0 = Outgoing, 1 = Incoming)
        direction: u8,
        /// Page ID where edge cluster will be stored
        page_id: u64,
        /// Timestamp
        timestamp: u64,
    },
}
