use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::{BackendDirection, NeighborQuery};
use crate::snapshot::SnapshotId;
use std::collections::{HashMap, HashSet, VecDeque};

impl V3Backend {
    pub(super) fn bfs_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        visited.insert(start);
        queue.push_back((start, 0));

        while let Some((node_id, current_depth)) = queue.pop_front() {
            if current_depth > depth {
                continue;
            }

            result.push(node_id);

            if current_depth < depth {
                let neighbors = self.neighbors_shared(
                    snapshot_id,
                    node_id,
                    NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    },
                )?;

                for neighbor in neighbors.iter() {
                    if visited.insert(*neighbor) {
                        queue.push_back((*neighbor, current_depth + 1));
                    }
                }
            }
        }

        Ok(result)
    }

    pub(super) fn shortest_path_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;

        if start == end {
            return Ok(Some(vec![start]));
        }

        let mut visited = HashMap::new();
        let mut queue = VecDeque::new();

        visited.insert(start, None);
        queue.push_back(start);

        while let Some(node_id) = queue.pop_front() {
            let neighbors = self.neighbors_shared(
                snapshot_id,
                node_id,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )?;

            for neighbor in neighbors.iter() {
                if !visited.contains_key(neighbor) {
                    visited.insert(*neighbor, Some(node_id));

                    if *neighbor == end {
                        let mut path = vec![end];
                        let mut current = node_id;

                        while let Some(&parent) = visited.get(&current) {
                            path.push(current);
                            match parent {
                                Some(p) => current = p,
                                None => break,
                            }
                        }

                        path.reverse();
                        return Ok(Some(path));
                    }

                    queue.push_back(*neighbor);
                }
            }
        }

        Ok(None)
    }

    pub(super) fn node_degree_counts(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;

        let outgoing = self
            .neighbors_shared(
                snapshot_id,
                node,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )?
            .len();

        let incoming = self
            .neighbors_shared(
                snapshot_id,
                node,
                NeighborQuery {
                    direction: BackendDirection::Incoming,
                    edge_type: None,
                },
            )?
            .len();

        Ok((outgoing, incoming))
    }

    pub(super) fn k_hop_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        visited.insert(start);
        queue.push_back((start, 0));

        while let Some((node_id, current_depth)) = queue.pop_front() {
            if current_depth > depth {
                continue;
            }

            if current_depth > 0 || depth == 0 {
                result.push(node_id);
            }

            if current_depth < depth {
                let neighbors = match direction {
                    BackendDirection::Outgoing => self.neighbors_shared(
                        snapshot_id,
                        node_id,
                        NeighborQuery {
                            direction: BackendDirection::Outgoing,
                            edge_type: None,
                        },
                    )?,
                    BackendDirection::Incoming => self.neighbors_shared(
                        snapshot_id,
                        node_id,
                        NeighborQuery {
                            direction: BackendDirection::Incoming,
                            edge_type: None,
                        },
                    )?,
                };

                for neighbor in neighbors.iter() {
                    if visited.insert(*neighbor) {
                        queue.push_back((*neighbor, current_depth + 1));
                    }
                }
            }
        }

        Ok(result)
    }

    pub(super) fn k_hop_filtered_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        _allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.k_hop_traversal(snapshot_id, start, depth, direction)
    }

    pub(super) fn bfs_filtered_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        _direction: BackendDirection,
        _allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.bfs_traversal(snapshot_id, start, depth)
    }

    pub(super) fn shortest_path_filtered_traversal(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
        _allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.shortest_path_traversal(snapshot_id, start, end)
    }
}
