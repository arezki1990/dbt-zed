//! Snapshot/version metadata helpers for native-v3.

use super::V3Backend;
use crate::SqliteGraphError;
use crate::snapshot::SnapshotId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

impl V3Backend {
    pub(super) fn current_graph_version(&self) -> u64 {
        *self.current_snapshot_version.read()
    }

    pub(super) fn next_graph_version(&self) -> u64 {
        self.current_graph_version().saturating_add(1)
    }

    pub(super) fn set_graph_version(&self, version: u64) {
        *self.current_snapshot_version.write() = version;
    }

    pub(super) fn set_snapshot_registry(&self, registry: HashMap<String, u64>) {
        *self.snapshot_registry.write() = registry;
    }

    pub(super) fn graph_transaction_active(&self) -> bool {
        !self.graph_tx_state.lock().frames.is_empty()
    }

    pub(super) fn ensure_hnsw_not_in_graph_transaction(
        &self,
        operation: &str,
    ) -> Result<(), SqliteGraphError> {
        if self.graph_transaction_active() {
            return Err(SqliteGraphError::unsupported(format!(
                "{} is not supported while a native-v3 transaction/savepoint is active",
                operation
            )));
        }
        Ok(())
    }

    pub(super) fn transaction_timestamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }

    pub(super) fn index_sidecar_path(db_path: &Path) -> PathBuf {
        db_path.with_extension("v3index")
    }

    pub(super) fn edge_metadata_path(db_path: &Path) -> PathBuf {
        db_path.with_extension("v3edgemeta")
    }

    pub(super) fn validate_base_path(db_path: &Path) -> Result<(), SqliteGraphError> {
        let uses_reserved_sqlite_extension = db_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("sqlite"))
            .unwrap_or(false);
        if uses_reserved_sqlite_extension {
            return Err(SqliteGraphError::connection(
                "Base path must not use .sqlite extension; that suffix is reserved for the internal SQLite property store"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn sqlite_sidecar_path(db_path: &Path) -> Result<PathBuf, SqliteGraphError> {
        Self::validate_base_path(db_path)?;
        Ok(db_path.with_extension("sqlite"))
    }

    /// Check if snapshot is valid for V3 backend.
    ///
    /// V3 now supports MVCC snapshots:
    /// - SnapshotId(0) = current state (sees all committed data)
    /// - SnapshotId(lsn) = historical state at specific LSN (time travel)
    /// - SnapshotId(u64::MAX) = invalid snapshot
    pub(super) fn require_current_snapshot(
        snapshot_id: SnapshotId,
    ) -> Result<(), SqliteGraphError> {
        if snapshot_id.0 == 0 {
            return Ok(());
        }

        if snapshot_id.0 == u64::MAX {
            return Err(SqliteGraphError::query(
                "Invalid snapshot ID (u64::MAX sentinel)".to_string(),
            ));
        }

        if snapshot_id.0 > 0 {
            return Ok(());
        }

        Err(SqliteGraphError::query(format!(
            "Invalid snapshot ID: {}",
            snapshot_id.0
        )))
    }

    /// Create a named snapshot with current LSN as version
    pub fn create_snapshot(&self, name: &str) -> Result<u64, SqliteGraphError> {
        let current_lsn = if let Some(ref wal) = self.wal {
            wal.read().committed_lsn()
        } else {
            self.current_graph_version()
        };

        let mut registry = self.snapshot_registry.write();
        registry.insert(name.to_string(), current_lsn);

        Ok(current_lsn)
    }

    /// Get the current snapshot version (LSN)
    pub fn current_snapshot_version(&self) -> u64 {
        if let Some(ref wal) = self.wal {
            wal.read().committed_lsn()
        } else {
            self.current_graph_version()
        }
    }

    /// Increment snapshot version (called after commits)
    pub(super) fn advance_snapshot_version(&self) -> u64 {
        let version = self.next_graph_version();
        self.set_graph_version(version);
        version
    }

    /// Get snapshot LSN by name
    pub fn get_snapshot_lsn(&self, name: &str) -> Option<u64> {
        let registry = self.snapshot_registry.read();
        registry.get(name).copied()
    }

    /// List all snapshots
    pub fn list_snapshots(&self) -> Vec<(String, u64)> {
        let registry = self.snapshot_registry.read();
        registry.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    /// Delete a snapshot by name
    pub fn delete_snapshot(&self, name: &str) -> Result<(), SqliteGraphError> {
        let mut registry = self.snapshot_registry.write();
        if registry.remove(name).is_none() {
            return Err(SqliteGraphError::query(format!(
                "Snapshot not found: {}",
                name
            )));
        }
        Ok(())
    }
}
