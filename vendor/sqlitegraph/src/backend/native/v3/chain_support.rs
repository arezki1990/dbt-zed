use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::edge_compat::Direction as EdgeDirection;
use crate::backend::{BackendDirection, ChainStep};
use crate::snapshot::SnapshotId;

impl V3Backend {
    pub(super) fn chain_query_steps(
        &self,
        _snapshot_id: SnapshotId,
        start: i64,
        chain: &[ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if chain.is_empty() {
            return Ok(vec![start]);
        }

        let mut current_nodes = vec![start];

        for step in chain {
            let dir = match step.direction {
                BackendDirection::Outgoing => EdgeDirection::Outgoing,
                BackendDirection::Incoming => EdgeDirection::Incoming,
            };

            let mut next_nodes = Vec::new();
            let edge_store = self.edge_store.read();

            for &node_id in &current_nodes {
                let neighbors = match step.edge_type.as_ref() {
                    Some(edge_type) => edge_store
                        .neighbors_filtered(node_id, dir, edge_type)
                        .map_err(map_v3_error)?,
                    None => edge_store.neighbors(node_id, dir).map_err(map_v3_error)?,
                };

                next_nodes.extend(neighbors.iter().copied());
            }

            if next_nodes.is_empty() {
                return Ok(Vec::new());
            }

            next_nodes.sort_unstable();
            next_nodes.dedup();
            current_nodes = next_nodes;
        }

        Ok(current_nodes)
    }
}
