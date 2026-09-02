use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::EdgeSpec;
use crate::backend::NodeSpec;
use crate::backend::native::v3::edge_compat::Direction as EdgeDirection;

#[path = "mutation_support/node_record_support.rs"]
mod node_record_support;
#[path = "mutation_support/persistence_support.rs"]
mod persistence_support;

impl V3Backend {
    /// Insert node without syncing (internal use only)
    ///
    /// Used by WriteBatchGuard to accumulate changes.
    /// Marked pub for benchmark access.
    pub fn insert_node_inner(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let requested_id = node
            .data
            .get("id")
            .or_else(|| node.data.get("node_id"))
            .and_then(|v| v.as_i64());

        let node_record = self.build_insert_node_record(&node, requested_id)?;

        let mut node_store = self.node_store.write();
        let node_id = node_store
            .insert_node(node_record, requested_id)
            .map_err(map_v3_error)?;

        self.kind_index.insert(node.kind.clone(), node_id);
        self.name_index.insert(node.name.clone(), node_id);

        let mut header = self.header.write();
        header.node_count += 1;

        if let Some(root_page) = node_store.btree_root_page_id() {
            header.root_index_page = root_page;
        }
        if let Some(tree_height) = node_store.btree_height() {
            header.btree_height = tree_height;
        }

        {
            let pub_guard = self.publisher.read();
            if let Some(ref publisher) = *pub_guard
                && publisher.has_subscribers()
            {
                let current_lsn = if let Some(ref wal) = self.wal {
                    wal.read().committed_lsn()
                } else {
                    *self.current_snapshot_version.read()
                };
                publisher.emit(
                    crate::backend::native::v3::pubsub::types::PubSubEvent::NodeChanged {
                        node_id,
                        snapshot_id: current_lsn,
                    },
                );
            }
        }

        Ok(node_id)
    }

    /// Insert edge without syncing (internal use only)
    ///
    /// Used by WriteBatchGuard to accumulate changes.
    pub(super) fn insert_edge_inner(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        let from_exists = self.get_node_internal(edge.from)?.is_some();
        let to_exists = self.get_node_internal(edge.to)?.is_some();
        if !from_exists || !to_exists {
            return Err(SqliteGraphError::invalid_input(
                "edge endpoints must reference existing entities",
            ));
        }

        let edge_store = self.edge_store.write();

        let edge_type = if edge.edge_type.is_empty() {
            None
        } else {
            Some(edge.edge_type.clone())
        };

        let weight = edge
            .data
            .get("weight")
            .and_then(|v| v.as_f64())
            .map(|w| w as f32)
            .unwrap_or(1.0);

        edge_store
            .insert_edge_weighted(
                edge.from,
                edge.to,
                EdgeDirection::Outgoing,
                edge_type.clone(),
                weight,
            )
            .map_err(map_v3_error)?;
        edge_store
            .insert_edge_weighted(
                edge.to,
                edge.from,
                EdgeDirection::Incoming,
                edge_type,
                weight,
            )
            .map_err(map_v3_error)?;

        let mut header = self.header.write();
        header.edge_count += 1;
        let edge_id = header.edge_count as i64;

        {
            let pub_guard = self.publisher.read();
            if let Some(ref publisher) = *pub_guard
                && publisher.has_subscribers()
            {
                let current_lsn = if let Some(ref wal) = self.wal {
                    wal.read().committed_lsn()
                } else {
                    *self.current_snapshot_version.read()
                };
                publisher.emit(
                    crate::backend::native::v3::pubsub::types::PubSubEvent::EdgeChanged {
                        edge_id,
                        from_node: edge.from,
                        to_node: edge.to,
                        snapshot_id: current_lsn,
                    },
                );
            }
        }

        Ok(edge_id)
    }
}
