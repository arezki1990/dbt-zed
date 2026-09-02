use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::v3::wal::V3WALPaths;
use crate::backend::{BackupResult, ImportMetadata, SnapshotMetadata};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "snapshot_io_support/import_support.rs"]
mod import_support;

impl V3Backend {
    pub(super) fn checkpoint_support(&self) -> Result<(), SqliteGraphError> {
        if let Some(ref wal) = self.wal {
            let header = self.header.read();
            let btree = self.btree.read();
            let allocator = self.allocator.read();

            wal.write()
                .checkpoint(
                    btree.root_page_id(),
                    allocator.total_pages(),
                    btree.tree_height(),
                    allocator.free_list_head(),
                    &header,
                )
                .map_err(|e| SqliteGraphError::connection(format!("Checkpoint failed: {:?}", e)))?;
        }

        Ok(())
    }

    pub(super) fn backup_support(
        &self,
        backup_dir: &Path,
    ) -> Result<BackupResult, SqliteGraphError> {
        std::fs::create_dir_all(backup_dir).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create backup dir: {}", e))
        })?;

        let timestamp = Self::snapshot_unix_timestamp_secs();
        let backup_filename = format!("v3_backup_{}.graph", timestamp);
        let backup_path = backup_dir.join(&backup_filename);

        std::fs::copy(&self.db_path, &backup_path)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to copy database: {}", e)))?;

        let wal_path = V3WALPaths::wal_file(&self.db_path);
        if wal_path.exists() {
            let backup_wal_path = V3WALPaths::wal_file(&backup_path);
            std::fs::copy(&wal_path, &backup_wal_path)
                .map_err(|e| SqliteGraphError::connection(format!("Failed to copy WAL: {}", e)))?;
        }

        let metadata = std::fs::metadata(&backup_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to get backup metadata: {}", e))
        })?;

        Ok(BackupResult {
            snapshot_path: backup_path,
            manifest_path: backup_dir.join(format!("v3_backup_{}.manifest", timestamp)),
            size_bytes: metadata.len(),
            checksum: 0,
            record_count: self.header.read().node_count,
            duration_secs: 0.0,
            timestamp,
            checkpoint_performed: self.wal.is_some(),
        })
    }

    pub(super) fn snapshot_export_support(
        &self,
        export_dir: &Path,
    ) -> Result<SnapshotMetadata, SqliteGraphError> {
        std::fs::create_dir_all(export_dir).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create export dir: {}", e))
        })?;

        let timestamp = Self::snapshot_unix_timestamp_secs();
        let snapshot_filename = format!("v3_snapshot_{}", timestamp);
        let snapshot_path = export_dir.join(&snapshot_filename);

        self.checkpoint_support()?;

        std::fs::copy(&self.db_path, &snapshot_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to export snapshot: {}", e))
        })?;

        let metadata = std::fs::metadata(&snapshot_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to get snapshot metadata: {}", e))
        })?;

        let header = self.header.read();

        Ok(SnapshotMetadata {
            snapshot_path,
            size_bytes: metadata.len(),
            entity_count: header.node_count,
            edge_count: header.edge_count,
        })
    }

    pub(super) fn snapshot_import_support(
        &self,
        import_dir: &Path,
    ) -> Result<ImportMetadata, SqliteGraphError> {
        import_support::snapshot_import_support(self, import_dir)
    }

    fn snapshot_unix_timestamp_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}
