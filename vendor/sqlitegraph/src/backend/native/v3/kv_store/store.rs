//! V3 Native KV Store - In-Memory Store with B+Tree Index Integration
//!
//! This module implements the core KV storage for V3, designed to integrate
//! with V3's page-based architecture.
//!
//! ## Architecture
//!
//! ```text
//! KV Store Layer:
//! ┌─────────────────────────────────────┐
//! │  KvStore                            │
//! │  ├─ entries: HashMap<key_hash, Vec<KvEntry>>  │
//! │  └─ btree: BTreeManager (key_hash → node_id)  │
//! └─────────────────────────────────────┘
//! │
//! ▼ WAL Integration
//! ┌─────────────────────────────────────┐
//! │  V3WALRecord::KvSet { ... }         │
//! │  V3WALRecord::KvDelete { ... }      │
//! └─────────────────────────────────────┘
//! │
//! ▼ Storage Layer  
//! ┌─────────────────────────────────────┐
//! │  NodeStore (KV nodes)               │
//! │  ├─ KV nodes stored as regular nodes│
//! │  └─ Kind = "_kv_" for identification│
//! └─────────────────────────────────────┘
//! ```

use crate::backend::native::v3::kv_store::types::{KvEntry, KvMetadata, KvValue, hash_key};
use crate::snapshot::SnapshotId;
use parking_lot::RwLock;
use std::collections::HashMap;

/// In-memory KV store with MVCC support
///
/// Each key maps to a version history (Vec<KvEntry> ordered by version).
/// This enables true MVCC snapshot isolation.
#[derive(Debug, Default)]
pub struct KvStore {
    /// Key hash → version history
    /// Each key retains all versions for snapshot isolation
    entries: RwLock<HashMap<u64, Vec<KvEntry>>>,
}

impl KvStore {
    /// Create a new empty KV store
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a value at a specific snapshot
    ///
    /// Uses binary search (O(log n)) to find the correct version.
    /// TTL is checked lazily: expired entries return None.
    pub fn get_at_snapshot(&self, key: &[u8], snapshot_id: SnapshotId) -> Option<KvValue> {
        let key_hash = hash_key(key);
        let entries = self.entries.read();

        let versions = entries.get(&key_hash)?;
        let snapshot_lsn = snapshot_id.as_lsn();

        // Snapshot at 0 means "see all data" - return latest version
        if snapshot_lsn == 0 {
            return versions
                .last()
                .filter(|e| !e.is_expired())
                .map(|e| e.value.clone());
        }

        // Binary search for the latest version with version <= snapshot_lsn
        let idx = versions.partition_point(|e| e.metadata.version <= snapshot_lsn);

        if idx == 0 {
            return None; // All versions are newer than snapshot
        }

        let entry = &versions[idx - 1];

        // Check TTL (lazy cleanup)
        if entry.is_expired() {
            return None;
        }

        // Check for tombstone (deleted entries have Null value)
        if matches!(entry.value, KvValue::Null) {
            return None;
        }

        Some(entry.value.clone())
    }

    /// Set a value with optional TTL
    ///
    /// Appends a new version to the key's version history.
    /// The version is set by the caller (from WAL LSN).
    pub fn set(&self, key: Vec<u8>, value: KvValue, ttl_seconds: Option<u64>, version: u64) {
        let key_hash = hash_key(&key);
        let mut entries = self.entries.write();

        // Get created_at from existing versions (if any)
        let created_at = entries
            .get(&key_hash)
            .and_then(|versions| versions.last().map(|e| e.metadata.created_at))
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            });

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let entry = KvEntry {
            key: key.clone(),
            value,
            metadata: KvMetadata {
                created_at,
                updated_at: now,
                ttl_seconds,
                version,
            },
        };

        entries.entry(key_hash).or_default().push(entry);
    }

    /// Delete a key (tombstone)
    ///
    /// Adds a tombstone entry with Null value.
    pub fn delete(&self, key: &[u8], version: u64) {
        let key_hash = hash_key(key);
        let mut entries = self.entries.write();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let entry = KvEntry {
            key: key.to_vec(),
            value: KvValue::Null,
            metadata: KvMetadata {
                created_at: now,
                updated_at: now,
                ttl_seconds: None,
                version,
            },
        };

        entries.entry(key_hash).or_default().push(entry);
    }

    /// Scan all keys matching a prefix
    ///
    /// Returns keys in lexicographic order along with their values.
    /// Only returns the latest version for each key.
    pub fn prefix_scan(&self, prefix: &[u8], snapshot_id: SnapshotId) -> Vec<(Vec<u8>, KvValue)> {
        let entries = self.entries.read();
        let mut results = Vec::new();

        for versions in entries.values() {
            if let Some(entry) = versions.last() {
                // Check prefix match
                if entry.key.starts_with(prefix) {
                    // Check snapshot visibility
                    if entry.metadata.version <= snapshot_id.as_lsn() {
                        // Check TTL
                        if !entry.is_expired() && !matches!(entry.value, KvValue::Null) {
                            results.push((entry.key.clone(), entry.value.clone()));
                        }
                    }
                }
            }
        }

        // Sort by key for deterministic output
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    /// Clean up expired entries
    ///
    /// This removes old versions that are both:
    /// 1. Expired by TTL
    /// 2. Older than the minimum active snapshot
    ///
    /// For now, this is a no-op (lazy cleanup on read).
    pub fn cleanup_expired(&self, _min_active_snapshot: u64) {
        // Background cleanup not implemented; lazy cleanup on read is sufficient
        // for current workloads. Full compaction would iterate all keys.
    }

    /// Serialize the KV store state to bytes for checkpoint persistence
    ///
    /// This captures the latest version of each key for durability across
    /// WAL truncation. Old versions are not included since checkpoint
    /// represents a consistent point-in-time snapshot.
    ///
    /// Format: count + (key_len, key, value_bytes, value_type, ttl, version) * N
    pub fn to_bytes(&self) -> Vec<u8> {
        let entries = self.entries.read();
        let mut result = Vec::new();

        // Write entry count as u32
        let count: u32 = entries.len().try_into().unwrap_or(u32::MAX);
        result.extend_from_slice(&count.to_le_bytes());

        // Write each entry (latest version only)
        for versions in entries.values() {
            if let Some(entry) = versions.last() {
                // Skip tombstones (deleted keys) - they don't need to persist in checkpoint
                if matches!(entry.value, KvValue::Null) {
                    continue;
                }

                // Skip expired entries
                if entry.is_expired() {
                    continue;
                }

                // Key length as u16
                let key_len: u16 = entry.key.len().try_into().unwrap_or(u16::MAX);
                result.extend_from_slice(&key_len.to_le_bytes());

                // Key bytes
                result.extend_from_slice(&entry.key);

                // Value serialization (u32 length in v3 format)
                let value_bytes = entry.value.to_bytes();
                let value_type = entry.value.type_tag();
                let value_len: u32 = value_bytes.len().try_into().unwrap_or(u32::MAX);
                result.extend_from_slice(&value_len.to_le_bytes());
                result.extend_from_slice(&value_bytes);
                result.push(value_type);

                // TTL (u64, 0 = None)
                let ttl = entry.metadata.ttl_seconds.unwrap_or(0);
                result.extend_from_slice(&ttl.to_le_bytes());

                // Version (u64)
                result.extend_from_slice(&entry.metadata.version.to_le_bytes());
            }
        }

        result
    }

    /// Deserialize KV store state from checkpoint bytes
    ///
    /// Replaces the current store contents with the checkpoint data.
    pub fn from_bytes(&mut self, bytes: &[u8], version: u8) -> Result<(), String> {
        use std::io::Read;

        if bytes.len() < 4 {
            return Err("Checkpoint data too short".to_string());
        }

        let mut cursor = std::io::Cursor::new(bytes);

        // Read entry count
        let mut count_bytes = [0u8; 4];
        cursor
            .read_exact(&mut count_bytes)
            .map_err(|e| e.to_string())?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        let mut new_entries = HashMap::new();

        for _ in 0..count {
            // Read key length
            let mut key_len_bytes = [0u8; 2];
            if cursor.read_exact(&mut key_len_bytes).is_err() {
                break; // End of data
            }
            let key_len = u16::from_le_bytes(key_len_bytes) as usize;

            // Read key
            let mut key = vec![0u8; key_len];
            cursor.read_exact(&mut key).map_err(|e| e.to_string())?;

            // Read value length (u32 in version >= 3, u16 in version < 3)
            let value_len = if version >= 3 {
                let mut value_len_bytes = [0u8; 4];
                cursor
                    .read_exact(&mut value_len_bytes)
                    .map_err(|e| e.to_string())?;
                u32::from_le_bytes(value_len_bytes) as usize
            } else {
                let mut value_len_bytes = [0u8; 2];
                cursor
                    .read_exact(&mut value_len_bytes)
                    .map_err(|e| e.to_string())?;
                u16::from_le_bytes(value_len_bytes) as usize
            };

            // Read value bytes
            let mut value_bytes = vec![0u8; value_len];
            cursor
                .read_exact(&mut value_bytes)
                .map_err(|e| e.to_string())?;

            // Read value type
            let mut value_type_byte = [0u8; 1];
            cursor
                .read_exact(&mut value_type_byte)
                .map_err(|e| e.to_string())?;
            let value_type = value_type_byte[0];

            // Deserialize value
            let value = KvValue::from_bytes(&value_bytes, value_type)
                .ok_or_else(|| "Failed to deserialize value".to_string())?;

            // Read TTL
            let mut ttl_bytes = [0u8; 8];
            cursor
                .read_exact(&mut ttl_bytes)
                .map_err(|e| e.to_string())?;
            let ttl = u64::from_le_bytes(ttl_bytes);
            let ttl_seconds = if ttl > 0 { Some(ttl) } else { None };

            // Read version
            let mut version_bytes = [0u8; 8];
            cursor
                .read_exact(&mut version_bytes)
                .map_err(|e| e.to_string())?;
            let version = u64::from_le_bytes(version_bytes);

            // Create entry
            let now = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let entry = KvEntry {
                key: key.clone(),
                value,
                metadata: KvMetadata {
                    created_at: now,
                    updated_at: now,
                    ttl_seconds,
                    version,
                },
            };

            let key_hash = hash_key(&key);
            new_entries.insert(key_hash, vec![entry]);
        }

        // Replace entries
        *self.entries.write() = new_entries;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kv_store_basic_operations() {
        let store = KvStore::new();
        let snapshot = SnapshotId::current();

        // Set a value
        store.set(b"key1".to_vec(), KvValue::Integer(42), None, 1);

        // Get the value
        let value = store.get_at_snapshot(b"key1", snapshot);
        assert_eq!(value, Some(KvValue::Integer(42)));

        // Get non-existent key
        let value = store.get_at_snapshot(b"nonexistent", snapshot);
        assert_eq!(value, None);
    }

    #[test]
    fn test_kv_store_snapshot_isolation() {
        let store = KvStore::new();

        // Set values at different versions
        store.set(b"key".to_vec(), KvValue::Integer(1), None, 10);
        store.set(b"key".to_vec(), KvValue::Integer(2), None, 20);
        store.set(b"key".to_vec(), KvValue::Integer(3), None, 30);

        // Create snapshots at different LSNs
        let snapshot_15 = SnapshotId::from_lsn(15);
        let snapshot_25 = SnapshotId::from_lsn(25);
        let snapshot_35 = SnapshotId::from_lsn(35);

        // Check visibility
        assert_eq!(
            store.get_at_snapshot(b"key", snapshot_15),
            Some(KvValue::Integer(1))
        );
        assert_eq!(
            store.get_at_snapshot(b"key", snapshot_25),
            Some(KvValue::Integer(2))
        );
        assert_eq!(
            store.get_at_snapshot(b"key", snapshot_35),
            Some(KvValue::Integer(3))
        );
    }

    #[test]
    fn test_kv_store_delete() {
        let store = KvStore::new();

        // Set then delete
        store.set(b"key".to_vec(), KvValue::Integer(42), None, 10);
        store.delete(b"key", 20);

        let snapshot_before = SnapshotId::from_lsn(15);
        let snapshot_after = SnapshotId::from_lsn(25);

        assert_eq!(
            store.get_at_snapshot(b"key", snapshot_before),
            Some(KvValue::Integer(42))
        );
        assert_eq!(store.get_at_snapshot(b"key", snapshot_after), None);
    }

    #[test]
    fn test_kv_store_prefix_scan() {
        let store = KvStore::new();
        let snapshot = SnapshotId::from_lsn(100);

        // Insert multiple keys
        store.set(
            b"user:1".to_vec(),
            KvValue::String("Alice".to_string()),
            None,
            10,
        );
        store.set(
            b"user:2".to_vec(),
            KvValue::String("Bob".to_string()),
            None,
            10,
        );
        store.set(
            b"user:3".to_vec(),
            KvValue::String("Charlie".to_string()),
            None,
            10,
        );
        store.set(
            b"other".to_vec(),
            KvValue::String("Other".to_string()),
            None,
            10,
        );

        // Scan with prefix
        let results = store.prefix_scan(b"user:", snapshot);
        assert_eq!(results.len(), 3);

        // Verify sorted order
        assert_eq!(results[0].0, b"user:1".to_vec());
        assert_eq!(results[1].0, b"user:2".to_vec());
        assert_eq!(results[2].0, b"user:3".to_vec());
    }

    #[test]
    fn test_kv_store_ttl_expiration() {
        let store = KvStore::new();

        // Set with 0 TTL (expired immediately for testing)
        store.set(b"expired".to_vec(), KvValue::Integer(1), Some(0), 10);
        store.set(b"valid".to_vec(), KvValue::Integer(2), None, 10);

        let snapshot = SnapshotId::from_lsn(100);

        // Expired key should return None
        assert_eq!(store.get_at_snapshot(b"expired", snapshot), None);

        // Valid key should return value
        assert_eq!(
            store.get_at_snapshot(b"valid", snapshot),
            Some(KvValue::Integer(2))
        );
    }
}
