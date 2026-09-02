use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::edge_compat::Direction as EdgeDirection;
use crate::backend::{BackendDirection, NeighborQuery};
use crate::snapshot::SnapshotId;
use std::sync::Arc;

impl V3Backend {
    /// Return a shared neighbor slice without cloning into a `Vec`.
    ///
    /// This is intended for focused benchmarking of the edge-store hot path.
    pub fn neighbors_shared(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Arc<[i64]>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        if let Some(csr_neighbors) = self.csr_neighbors_shared(snapshot_id, node, &query)? {
            return Ok(csr_neighbors);
        }
        let edge_store = self.edge_store.read();

        if let Some(ref edge_type) = query.edge_type {
            let dir = match query.direction {
                BackendDirection::Outgoing => EdgeDirection::Outgoing,
                BackendDirection::Incoming => EdgeDirection::Incoming,
            };
            edge_store
                .neighbors_filtered(node, dir, edge_type)
                .map_err(map_v3_error)
        } else {
            match query.direction {
                BackendDirection::Outgoing => edge_store.outgoing(node).map_err(map_v3_error),
                BackendDirection::Incoming => edge_store.incoming(node).map_err(map_v3_error),
            }
        }
    }

    /// Return shared (neighbor_id, weight) pairs without allocating per call.
    ///
    /// For unfiltered queries, neighbors are returned in descending weight
    /// order with neighbor ID as a deterministic tie-breaker.
    pub fn neighbors_weighted_shared(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Arc<[(i64, f32)]>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        if let Some(csr_neighbors) =
            self.csr_neighbors_weighted_shared(snapshot_id, node, &query)?
        {
            return Ok(csr_neighbors);
        }
        let edge_store = self.edge_store.read();

        let dir = match query.direction {
            BackendDirection::Outgoing => EdgeDirection::Outgoing,
            BackendDirection::Incoming => EdgeDirection::Incoming,
        };

        if let Some(ref edge_type) = query.edge_type {
            edge_store
                .neighbors_weighted_filtered(node, dir, edge_type)
                .map_err(map_v3_error)
        } else {
            edge_store
                .neighbors_weighted(node, dir)
                .map_err(map_v3_error)
        }
    }

    /// Return (neighbor_id, weight) pairs for the given node.
    pub fn neighbors_weighted(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        let shared = self.neighbors_weighted_shared(snapshot_id, node, query)?;
        Ok(shared.to_vec())
    }

    /// Warm the weighted-neighbor cache for a set of source nodes.
    ///
    /// This preloads edge clusters for the requested direction so the first
    /// `neighbors_weighted_shared` call can hit in-memory cache.
    pub fn warm_neighbors_for_sources(
        &self,
        snapshot_id: SnapshotId,
        sources: &[i64],
        query: NeighborQuery,
    ) -> Result<usize, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        let edge_store = self.edge_store.read();
        let dir = match query.direction {
            BackendDirection::Outgoing => EdgeDirection::Outgoing,
            BackendDirection::Incoming => EdgeDirection::Incoming,
        };
        edge_store
            .warm_weighted_neighbors(sources, dir)
            .map_err(map_v3_error)
    }

    /// Return current edge-cache hit and miss counters.
    pub fn edge_cache_stats(&self) -> (u64, u64, u64, u64, usize) {
        self.edge_store.read().cache_stats()
    }

    /// Reset edge-cache counters without evicting cached rows.
    pub fn reset_edge_cache_stats(&self) {
        self.edge_store.read().reset_stats();
    }
}
