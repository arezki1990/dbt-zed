//! V3 Native Key-Value Store
//!
//! This module provides a native V3 implementation of key-value storage,
//! integrated with V3's page-based architecture and WAL.
//!
//! ## Architecture
//!
//! The KV store uses V3's existing infrastructure:
//! - **B+Tree index**: Maps key_hash → node_id for O(log n) lookups
//! - **Node storage**: KV entries stored as regular nodes with kind="_kv_"
//! - **WAL integration**: KV operations logged as V3WALRecord variants
//! - **MVCC**: Version history per key using LSN from WAL
//!
//! ## Design Decisions
//!
//! 1. **In-memory HashMap with WAL backing**: Fast reads, durable writes
//! 2. **Lazy TTL cleanup**: Expired entries filtered on read
//! 3. **Snapshot isolation**: Binary search for correct version at snapshot
//! 4. **Key hashing**: Uses std::hash for B+Tree compatibility

pub mod store;
pub mod types;

// Re-export public API
pub use store::KvStore;
pub use types::{KvEntry, KvMetadata, KvValue, hash_key};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify key types are exported
        let _store = KvStore::new();
        let _value = KvValue::Integer(42);
        let _entry = KvEntry::new(b"key".to_vec(), KvValue::Null, 1, None);
    }
}
