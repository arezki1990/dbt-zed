use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::types::KvValue as V2KvValue;
use crate::backend::native::v3::kv_store::types::hash_key;
use crate::backend::native::v3::wal::V3WALRecord;
use crate::backend::native::v3::{KvStore, KvValue};
use crate::snapshot::SnapshotId;

impl V3Backend {
    pub(crate) fn kv_get_v2(
        &self,
        snapshot_id: SnapshotId,
        key: &[u8],
    ) -> Result<Option<V2KvValue>, SqliteGraphError> {
        let kv_guard = self.kv_store.read();
        let v3_value = kv_guard
            .as_ref()
            .and_then(|kv| kv.get_at_snapshot(key, snapshot_id));

        let v2_value = v3_value.and_then(|v| match v {
            KvValue::Null => None,
            KvValue::Integer(i) => Some(V2KvValue::Integer(i)),
            KvValue::Float(f) => Some(V2KvValue::Float(f)),
            KvValue::String(s) => Some(V2KvValue::String(s)),
            KvValue::Boolean(b) => Some(V2KvValue::Boolean(b)),
            KvValue::Bytes(b) => Some(V2KvValue::Bytes(b)),
            KvValue::Json(j) => Some(V2KvValue::Json(j)),
        });

        Ok(v2_value)
    }

    pub(crate) fn kv_set_v2(
        &self,
        key: Vec<u8>,
        value: V2KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), SqliteGraphError> {
        let v3_value = match &value {
            V2KvValue::Null => KvValue::Null,
            V2KvValue::Integer(i) => KvValue::Integer(*i),
            V2KvValue::Float(f) => KvValue::Float(*f),
            V2KvValue::String(s) => KvValue::String(s.clone()),
            V2KvValue::Boolean(b) => KvValue::Boolean(*b),
            V2KvValue::Bytes(b) => KvValue::Bytes(b.clone()),
            V2KvValue::Json(j) => KvValue::Json(j.clone()),
        };

        let version = self.current_kv_version();
        let key_hash = hash_key(&key);

        {
            let mut kv_guard = self.kv_store.write();
            kv_guard.get_or_insert_with(KvStore::new).set(
                key.clone(),
                v3_value,
                ttl_seconds,
                version,
            );
        }

        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            let value_bytes = match &value {
                V2KvValue::Null => vec![],
                V2KvValue::Integer(i) => i.to_le_bytes().to_vec(),
                V2KvValue::Float(f) => f.to_le_bytes().to_vec(),
                V2KvValue::String(s) => s.clone().into_bytes(),
                V2KvValue::Boolean(b) => vec![if *b { 1 } else { 0 }],
                V2KvValue::Bytes(b) => b.clone(),
                V2KvValue::Json(j) => serde_json::to_vec(j).unwrap_or_default(),
            };
            let value_type = match &value {
                V2KvValue::Null => 0,
                V2KvValue::Integer(_) => 1,
                V2KvValue::Float(_) => 2,
                V2KvValue::String(_) => 3,
                V2KvValue::Boolean(_) => 4,
                V2KvValue::Bytes(_) => 5,
                V2KvValue::Json(_) => 6,
            };

            let record = V3WALRecord::KvSet {
                lsn: version,
                key,
                value_bytes,
                value_type,
                ttl_seconds,
                timestamp: Self::unix_timestamp_secs(),
            };
            wal_guard
                .append(&record)
                .map_err(|e| SqliteGraphError::connection(format!("WAL write failed: {:?}", e)))?;
        }

        self.emit_kv_changed(key_hash, version);
        Ok(())
    }

    pub(crate) fn kv_delete_v2(&self, key: &[u8]) -> Result<(), SqliteGraphError> {
        let version = self.current_kv_version();

        {
            let mut kv_guard = self.kv_store.write();
            kv_guard
                .get_or_insert_with(KvStore::new)
                .delete(key, version);
        }

        if let Some(ref wal) = self.wal {
            let mut wal_guard = wal.write();
            let record = V3WALRecord::KvDelete {
                lsn: version,
                key: key.to_vec(),
                timestamp: Self::unix_timestamp_secs(),
            };
            wal_guard
                .append(&record)
                .map_err(|e| SqliteGraphError::connection(format!("WAL write failed: {:?}", e)))?;
        }

        self.emit_kv_changed(hash_key(key), version);
        Ok(())
    }

    pub(crate) fn kv_prefix_scan_v2(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, V2KvValue)>, SqliteGraphError> {
        let v3_results = self.kv_prefix_scan_v3(snapshot_id, prefix);
        let v2_results = v3_results
            .into_iter()
            .filter_map(|(k, v)| {
                let v2_value = match v {
                    KvValue::Null => return None,
                    KvValue::Integer(i) => V2KvValue::Integer(i),
                    KvValue::Float(f) => V2KvValue::Float(f),
                    KvValue::String(s) => V2KvValue::String(s),
                    KvValue::Boolean(b) => V2KvValue::Boolean(b),
                    KvValue::Bytes(b) => V2KvValue::Bytes(b),
                    KvValue::Json(j) => V2KvValue::Json(j),
                };
                Some((k, v2_value))
            })
            .collect();
        Ok(v2_results)
    }
}
