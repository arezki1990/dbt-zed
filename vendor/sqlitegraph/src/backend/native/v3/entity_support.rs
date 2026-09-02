use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::NodeSpec;
use crate::backend::native::v3::NodeRecordV3;
use crate::graph::GraphEntity;
use crate::snapshot::SnapshotId;

impl V3Backend {
    /// Get node by ID (internal method)
    ///
    /// Looks up a node record by its ID using the B+Tree index.
    pub(super) fn get_node_internal(
        &self,
        node_id: i64,
    ) -> Result<Option<NodeRecordV3>, SqliteGraphError> {
        if let Some(record) = self.node_cache.get(node_id) {
            return Ok(Some(record));
        }

        let mut node_store = self.node_store.write();
        if let Some(record) = node_store.lookup_node(node_id).map_err(map_v3_error)? {
            self.node_cache.insert(node_id, record.clone());
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    pub(super) fn insert_node_support(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let kind = node.kind.clone();
        let name = node.name.clone();
        let data = node.data.clone();
        let next_version = self.next_graph_version();

        let node_id = self.insert_node_inner(node)?;
        self.sync_header()?;
        self.persist_inserted_node_properties(node_id, &kind, &name, &data, next_version)?;

        self.advance_snapshot_version();

        Ok(node_id)
    }

    pub(super) fn insert_edge_support(
        &self,
        edge: crate::backend::EdgeSpec,
    ) -> Result<i64, SqliteGraphError> {
        let from = edge.from;
        let to = edge.to;
        let edge_type = edge.edge_type.clone();
        let data = edge.data.clone();
        let next_version = self.next_graph_version();

        let edge_id = self.insert_edge_inner(edge)?;
        self.sync_header()?;
        self.persist_inserted_edge_attributes(from, to, &edge_type, &data, next_version)?;
        self.rebuild_csr_runtime_views(next_version)?;
        self.advance_snapshot_version();

        Ok(edge_id)
    }

    pub(super) fn update_node_support(
        &self,
        node_id: i64,
        node: NodeSpec,
    ) -> Result<i64, SqliteGraphError> {
        let kind = node.kind.clone();
        let name = node.name.clone();
        let next_version = self.next_graph_version();

        let updated_record = NodeRecordV3::new_inline(
            node_id,
            crate::backend::native::types::NodeFlags::empty(),
            0,
            0,
            serde_json::to_vec(&node.data).unwrap_or_default(),
            0,
            0,
            0,
            0,
        );

        let mut node_store = self.node_store.write();
        node_store
            .update_node(node_id, updated_record)
            .map_err(map_v3_error)?;
        drop(node_store);

        self.node_cache.invalidate(node_id);

        self.flush_to_disk()?;
        self.persist_updated_node_properties(node_id, &kind, &name, &node.data, next_version)?;
        self.rebuild_indexes();
        self.advance_snapshot_version();

        Ok(node_id)
    }

    pub(super) fn delete_entity_support(&self, id: i64) -> Result<(), SqliteGraphError> {
        let next_version = self.next_graph_version();
        let mut node_store = self.node_store.write();
        node_store.delete_node(id).map_err(map_v3_error)?;
        drop(node_store);

        self.node_cache.invalidate(id);

        {
            let mut header = self.header.write();
            header.node_count = header.node_count.saturating_sub(1);
        }
        self.sync_header()?;

        self.flush_to_disk()?;
        self.persist_deleted_node_marker(id, next_version)?;
        self.rebuild_indexes();
        self.advance_snapshot_version();

        Ok(())
    }

    pub(super) fn entity_ids_support(&self) -> Result<Vec<i64>, SqliteGraphError> {
        let node_store = self.node_store.read();
        node_store.node_ids().map_err(map_v3_error)
    }

    pub(super) fn get_node_support(
        &self,
        snapshot_id: SnapshotId,
        id: i64,
    ) -> Result<GraphEntity, SqliteGraphError> {
        let deleted_before_present = self.validate_node_snapshot_visibility(snapshot_id, id)?;

        match self.get_node_internal(id)? {
            Some(record) => {
                let data_bytes = self.load_node_record_bytes(&record)?;
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
