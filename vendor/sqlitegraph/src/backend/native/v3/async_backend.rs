//! Async helper and trait implementation for V3 backend.

use super::{EdgeDirection, V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::{AsyncFileCoordinator, NodeRecordV3};
use crate::backend::{BackendDirection, GraphEntity, NeighborQuery};
use crate::snapshot::SnapshotId;
use std::sync::Arc;

impl V3Backend {
    /// Get or create the async file coordinator
    pub fn get_async_coordinator(&self) -> Result<Arc<AsyncFileCoordinator>, SqliteGraphError> {
        if let Some(coord) = self.async_coordinator.get() {
            return Ok(Arc::clone(coord));
        }
        let coord = AsyncFileCoordinator::create(&self.db_path)?;
        let arc = Arc::new(coord);
        if self.async_coordinator.set(Arc::clone(&arc)).is_ok() {
            return Ok(arc);
        }
        self.async_coordinator.get().cloned().ok_or_else(|| {
            SqliteGraphError::connection(
                "async coordinator initialization raced without publishing coordinator".to_string(),
            )
        })
    }

    async fn get_node_internal_async(
        &self,
        node_id: i64,
        async_coordinator: &AsyncFileCoordinator,
    ) -> Result<Option<NodeRecordV3>, SqliteGraphError> {
        if let Some(record) = self.node_cache.get(node_id) {
            return Ok(Some(record));
        }

        let (btree_manager, page_cache, unpacked_page_cache) = {
            let store = self.node_store.read();
            (
                store.btree_manager().clone(),
                Arc::clone(store.page_cache_ref()),
                Arc::clone(store.unpacked_page_cache_ref()),
            )
        };

        if let Some(record) = crate::backend::native::v3::node::store::lookup_node_async(
            &btree_manager,
            &page_cache,
            &unpacked_page_cache,
            node_id,
            async_coordinator,
        )
        .await?
        {
            self.node_cache.insert(node_id, record.clone());
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }
}

#[allow(
    clippy::manual_async_fn,
    reason = "impl Future return is required by the AsyncGraphBackend trait definition"
)]
impl crate::backend::AsyncGraphBackend for V3Backend {
    fn get_node(
        &self,
        snapshot_id: SnapshotId,
        id: i64,
    ) -> impl std::future::Future<Output = Result<GraphEntity, SqliteGraphError>> + Send {
        async move {
            let mut deleted_before_present: Option<u64> = None;

            if snapshot_id.0 != 0 {
                let conn = self.sqlite_conn.lock();
                let mut stmt = conn.prepare(
                    "SELECT created_version, updated_version, deleted_version FROM node_properties WHERE node_id = ?1"
                )
                    .map_err(|e| SqliteGraphError::connection(format!("Failed to prepare query: {}", e)))?;

                let mut rows = stmt
                    .query([id])
                    .map_err(|e| SqliteGraphError::connection(format!("Failed to query: {}", e)))?;

                if let Some(row) = rows.next().map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to fetch row: {}", e))
                })? {
                    let created_version: u64 = row.get(0).map_err(|e| {
                        SqliteGraphError::connection(format!(
                            "Failed to get created_version: {}",
                            e
                        ))
                    })?;
                    let updated_version: Option<u64> = row.get(1).map_err(|e| {
                        SqliteGraphError::connection(format!(
                            "Failed to get updated_version: {}",
                            e
                        ))
                    })?;
                    let deleted_version: Option<u64> = row.get(2).map_err(|e| {
                        SqliteGraphError::connection(format!(
                            "Failed to get deleted_version: {}",
                            e
                        ))
                    })?;

                    if created_version > snapshot_id.0 {
                        return Err(SqliteGraphError::query(format!(
                            "Node {} not found in snapshot",
                            id
                        )));
                    }

                    if let Some(version) = updated_version
                        && snapshot_id.0 < version
                    {
                        return Err(SqliteGraphError::query(format!(
                            "Historical version for node {} is unavailable after update",
                            id
                        )));
                    }

                    if let Some(version) = deleted_version {
                        if snapshot_id.0 >= version {
                            return Err(SqliteGraphError::query(format!(
                                "Node {} not found in snapshot",
                                id
                            )));
                        }
                        deleted_before_present = Some(version);
                    }
                } else {
                    return Err(SqliteGraphError::query(format!("Node {} not found", id)));
                }
            }

            let async_coordinator = self.get_async_coordinator()?;

            match self.get_node_internal_async(id, &async_coordinator).await? {
                Some(record) => {
                    let data_bytes = if let Some(inline) = record.data_inline {
                        inline
                    } else if let Some(offset) = record.data_external_offset {
                        let target_len = record.data_len
                            & crate::backend::native::v3::node::record::constants::MAX_DATA_LEN;
                        let buf = vec![0u8; target_len as usize];
                        let (buffer, _) = async_coordinator.read_at_offset(offset, buf).await?;
                        buffer
                    } else {
                        Vec::new()
                    };

                    let (kind, name, data) = Self::parse_node_data(&data_bytes, id);

                    Ok(GraphEntity {
                        id,
                        kind,
                        name,
                        file_path: None,
                        data,
                    })
                }
                None => {
                    if deleted_before_present.is_some() {
                        Err(SqliteGraphError::query(format!(
                            "Historical version for node {} is unavailable after delete",
                            id
                        )))
                    } else {
                        Err(SqliteGraphError::query(format!("Node {} not found", id)))
                    }
                }
            }
        }
    }

    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> impl std::future::Future<Output = Result<Vec<i64>, SqliteGraphError>> + Send {
        async move {
            Self::require_current_snapshot(snapshot_id)?;
            if let Some(csr_neighbors) = self.csr_neighbors_shared(snapshot_id, node, &query)? {
                return Ok(csr_neighbors.to_vec());
            }
            let async_coordinator = self.get_async_coordinator()?;

            if let Some(ref edge_type) = query.edge_type {
                let edge_store = self.edge_store.read();
                let dir = match query.direction {
                    BackendDirection::Outgoing => EdgeDirection::Outgoing,
                    BackendDirection::Incoming => EdgeDirection::Incoming,
                };
                let neighbors_arc = edge_store
                    .neighbors_filtered(node, dir, edge_type)
                    .map_err(map_v3_error)?;
                Ok(neighbors_arc.to_vec())
            } else {
                let (btree_lock, cache_lock, edge_types_lock, page_size) = {
                    let store = self.edge_store.read();
                    (
                        Arc::clone(store.btree_lock()),
                        Arc::clone(store.cache_lock()),
                        Arc::clone(store.edge_types_lock()),
                        store.page_size(),
                    )
                };

                let dir = match query.direction {
                    BackendDirection::Outgoing => EdgeDirection::Outgoing,
                    BackendDirection::Incoming => EdgeDirection::Incoming,
                };

                let neighbors_arc = crate::backend::native::v3::edge_async::neighbors_async(
                    &btree_lock,
                    &cache_lock,
                    &edge_types_lock,
                    page_size,
                    node,
                    dir,
                    &async_coordinator,
                )
                .await?;

                Ok(neighbors_arc.to_vec())
            }
        }
    }
}
