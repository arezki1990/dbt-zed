use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::edge_compat::Direction as EdgeDirection;
use crate::backend::{BackendDirection, NeighborQuery};
use crate::snapshot::SnapshotId;
use std::collections::HashMap;
use std::sync::Arc;

#[path = "csr_support/runtime_view_support.rs"]
mod runtime_view_support;

const CSR_SHARD_OUTGOING: i64 = 0;
const CSR_SHARD_INCOMING: i64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CsrEdgeTypeLookup {
    MissingTable,
    MissingType,
    Found(u32),
}

pub(super) fn encode_csr_adjacency(entries: &[(u32, f32, u32)]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 12);
    for &(dst, weight, flags) in entries {
        buf.extend_from_slice(&dst.to_le_bytes());
        buf.extend_from_slice(&weight.to_le_bytes());
        buf.extend_from_slice(&flags.to_le_bytes());
    }
    buf
}

impl V3Backend {
    pub(super) fn csr_shard_id(direction: BackendDirection) -> i64 {
        match direction {
            BackendDirection::Outgoing => CSR_SHARD_OUTGOING,
            BackendDirection::Incoming => CSR_SHARD_INCOMING,
        }
    }

    pub(super) fn lookup_csr_edge_type_id(
        &self,
        edge_type: &str,
    ) -> Result<CsrEdgeTypeLookup, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        let mut stmt = match conn.prepare("SELECT type_id FROM csr_edge_types WHERE edge_type = ?1")
        {
            Ok(stmt) => stmt,
            Err(e) if e.to_string().contains("no such table: csr_edge_types") => {
                return Ok(CsrEdgeTypeLookup::MissingTable);
            }
            Err(e) => {
                return Err(SqliteGraphError::query(format!(
                    "Failed to prepare CSR edge type lookup: {}",
                    e
                )));
            }
        };

        let mut rows = stmt.query([edge_type]).map_err(|e| {
            SqliteGraphError::query(format!("Failed to execute CSR edge type lookup: {}", e))
        })?;

        let Some(row) = rows.next().map_err(|e| {
            SqliteGraphError::query(format!("Failed to read CSR edge type row: {}", e))
        })?
        else {
            return Ok(CsrEdgeTypeLookup::MissingType);
        };

        let type_id: i64 = row.get(0).map_err(|e| {
            SqliteGraphError::query(format!("Failed to load CSR edge type id: {}", e))
        })?;
        Ok(CsrEdgeTypeLookup::Found(type_id as u32))
    }

    pub(super) fn load_csr_adjacency(
        &self,
        node_id: i64,
        visible_version: u64,
        direction: BackendDirection,
        edge_type: Option<&str>,
    ) -> Result<Option<Vec<(i64, f32)>>, SqliteGraphError> {
        let expected_type_id = match edge_type {
            Some(edge_type) => match self.lookup_csr_edge_type_id(edge_type)? {
                CsrEdgeTypeLookup::MissingTable => return Ok(None),
                CsrEdgeTypeLookup::MissingType => return Ok(Some(Vec::new())),
                CsrEdgeTypeLookup::Found(type_id) => Some(type_id),
            },
            None => None,
        };

        let conn = self.sqlite_conn.lock();
        let mut stmt = match conn.prepare(
            "SELECT shard_data
             FROM csr_shards
             WHERE shard_id = ?1 AND node_id = ?2 AND visible_at <= ?3
             ORDER BY version DESC
             LIMIT 1",
        ) {
            Ok(stmt) => stmt,
            Err(e) if e.to_string().contains("no such table: csr_shards") => return Ok(None),
            Err(e) => {
                return Err(SqliteGraphError::query(format!(
                    "Failed to prepare CSR lookup: {}",
                    e
                )));
            }
        };

        let mut rows = stmt
            .query([
                Self::csr_shard_id(direction),
                node_id,
                visible_version as i64,
            ])
            .map_err(|e| SqliteGraphError::query(format!("Failed to execute CSR lookup: {}", e)))?;

        let Some(row) = rows
            .next()
            .map_err(|e| SqliteGraphError::query(format!("Failed to read CSR row: {}", e)))?
        else {
            return Ok(None);
        };

        let blob: Option<Vec<u8>> = row
            .get(0)
            .map_err(|e| SqliteGraphError::query(format!("Failed to load CSR blob: {}", e)))?;

        let Some(blob) = blob else {
            return Ok(Some(Vec::new()));
        };

        if blob.len() % 12 != 0 {
            return Err(SqliteGraphError::GraphCorruption(format!(
                "CSR shard_data for node {} has invalid length {} (expected 12-byte records)",
                node_id,
                blob.len()
            )));
        }

        let mut neighbors = Vec::with_capacity(blob.len() / 12);
        for chunk in blob.chunks_exact(12) {
            let dst = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as i64;
            let weight = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            let flags = u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
            if expected_type_id.is_none_or(|type_id| flags == type_id) {
                neighbors.push((dst, weight));
            }
        }

        Ok(Some(neighbors))
    }

    pub(super) fn csr_neighbors_shared(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: &NeighborQuery,
    ) -> Result<Option<Arc<[i64]>>, SqliteGraphError> {
        let visible_version = if snapshot_id == SnapshotId::current() {
            *self.current_snapshot_version.read()
        } else {
            snapshot_id.as_u64()
        };

        let Some(adjacency) = self.load_csr_adjacency(
            node,
            visible_version,
            query.direction,
            query.edge_type.as_deref(),
        )?
        else {
            return Ok(None);
        };

        let ids: Vec<i64> = adjacency.into_iter().map(|(dst, _)| dst).collect();
        Ok(Some(Arc::from(ids.into_boxed_slice())))
    }

    pub(super) fn csr_neighbors_weighted_shared(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: &NeighborQuery,
    ) -> Result<Option<Arc<[(i64, f32)]>>, SqliteGraphError> {
        let visible_version = if snapshot_id == SnapshotId::current() {
            *self.current_snapshot_version.read()
        } else {
            snapshot_id.as_u64()
        };

        let Some(adjacency) = self.load_csr_adjacency(
            node,
            visible_version,
            query.direction,
            query.edge_type.as_deref(),
        )?
        else {
            return Ok(None);
        };

        Ok(Some(Arc::from(adjacency.into_boxed_slice())))
    }
}
