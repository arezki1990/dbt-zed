use super::V3Backend;
use crate::backend::native::v3::{KvStore, KvValue, Publisher};
use crate::snapshot::SnapshotId;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "kv_pubsub_support/pubsub_bridge_support.rs"]
mod pubsub_bridge_support;
#[path = "kv_pubsub_support/v2_bridge_support.rs"]
mod v2_bridge_support;

impl V3Backend {
    /// Check if KV store has been initialized
    pub fn is_kv_initialized(&self) -> bool {
        self.kv_store.read().is_some()
    }

    /// Check if Publisher has been initialized
    pub fn is_pubsub_initialized(&self) -> bool {
        self.publisher.read().is_some()
    }

    /// Get a value from the KV store
    ///
    /// Returns None if the key doesn't exist or has been deleted (tombstone).
    pub fn kv_get_v3(&self, snapshot_id: SnapshotId, key: &[u8]) -> Option<KvValue> {
        let kv_guard = self.kv_store.read();
        kv_guard.as_ref().and_then(|kv| {
            kv.get_at_snapshot(key, snapshot_id)
                .filter(|v| !matches!(v, KvValue::Null))
        })
    }

    /// Set a value in the KV store
    pub fn kv_set_v3(&self, key: Vec<u8>, value: KvValue, ttl_seconds: Option<u64>) {
        let version = self.current_kv_version();
        let mut kv_guard = self.kv_store.write();
        kv_guard
            .get_or_insert_with(KvStore::new)
            .set(key, value, ttl_seconds, version);
    }

    /// Delete a key from the KV store
    pub fn kv_delete_v3(&self, key: &[u8]) {
        let version = self.current_kv_version();
        let mut kv_guard = self.kv_store.write();
        kv_guard
            .get_or_insert_with(KvStore::new)
            .delete(key, version);
    }

    /// Prefix scan for keys in the KV store using V3 types
    pub fn kv_prefix_scan_v3(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Vec<(Vec<u8>, KvValue)> {
        let kv_guard = self.kv_store.read();
        kv_guard
            .as_ref()
            .map(|kv| kv.prefix_scan(prefix, snapshot_id))
            .unwrap_or_default()
    }
    fn current_kv_version(&self) -> u64 {
        if let Some(ref wal) = self.wal {
            wal.read().committed_lsn()
        } else {
            1
        }
    }

    fn emit_kv_changed(&self, key_hash: u64, snapshot_id: u64) {
        let mut pub_guard = self.publisher.write();
        pub_guard.get_or_insert_with(Publisher::new).emit(
            crate::backend::native::v3::pubsub::types::PubSubEvent::KvChanged {
                key_hash,
                snapshot_id,
            },
        );
    }

    fn unix_timestamp_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}
