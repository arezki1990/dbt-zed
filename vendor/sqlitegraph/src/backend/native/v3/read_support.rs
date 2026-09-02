use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::v3::NodeRecordV3;
use crate::snapshot::SnapshotId;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};

impl V3Backend {
    pub(super) fn validate_node_snapshot_visibility(
        &self,
        snapshot_id: SnapshotId,
        node_id: i64,
    ) -> Result<Option<u64>, SqliteGraphError> {
        let mut deleted_before_present = None;

        if snapshot_id.0 == 0 {
            return Ok(deleted_before_present);
        }

        let conn = self.sqlite_conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT created_version, updated_version, deleted_version FROM node_properties WHERE node_id = ?1",
            )
            .map_err(|e| SqliteGraphError::connection(format!("Failed to prepare query: {}", e)))?;

        let mut rows = stmt
            .query([node_id])
            .map_err(|e| SqliteGraphError::connection(format!("Failed to query: {}", e)))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to fetch row: {}", e)))?
        {
            let created_version: u64 = row.get(0).map_err(|e| {
                SqliteGraphError::connection(format!("Failed to get created_version: {}", e))
            })?;
            let updated_version: Option<u64> = row.get(1).map_err(|e| {
                SqliteGraphError::connection(format!("Failed to get updated_version: {}", e))
            })?;
            let deleted_version: Option<u64> = row.get(2).map_err(|e| {
                SqliteGraphError::connection(format!("Failed to get deleted_version: {}", e))
            })?;

            if created_version > snapshot_id.0 {
                return Err(SqliteGraphError::query(format!(
                    "Node {} not found in snapshot",
                    node_id
                )));
            }

            if let Some(version) = updated_version
                && snapshot_id.0 < version
            {
                return Err(SqliteGraphError::query(format!(
                    "Historical version for node {} is unavailable after update",
                    node_id
                )));
            }

            if let Some(version) = deleted_version {
                if snapshot_id.0 >= version {
                    return Err(SqliteGraphError::query(format!(
                        "Node {} not found in snapshot",
                        node_id
                    )));
                }
                deleted_before_present = Some(version);
            }
        } else {
            return Err(SqliteGraphError::query(format!(
                "Node {} not found",
                node_id
            )));
        }

        Ok(deleted_before_present)
    }

    pub(super) fn load_node_record_bytes(
        &self,
        record: &NodeRecordV3,
    ) -> Result<Vec<u8>, SqliteGraphError> {
        if let Some(inline) = record.data_inline.clone() {
            return Ok(inline);
        }

        if let Some(offset) = record.data_external_offset {
            let actual_data_len =
                record.data_len & crate::backend::native::v3::node::record::constants::MAX_DATA_LEN;
            let mut file = OpenOptions::new()
                .read(true)
                .open(&self.db_path)
                .map_err(|e| SqliteGraphError::connection(format!("Failed to open file: {}", e)))?;

            let mut buffer = vec![0u8; actual_data_len as usize];
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| SqliteGraphError::connection(format!("Failed to seek: {}", e)))?;
            file.read_exact(&mut buffer)
                .map_err(|e| SqliteGraphError::connection(format!("Failed to read: {}", e)))?;
            return Ok(buffer);
        }

        Ok(Vec::new())
    }
}
