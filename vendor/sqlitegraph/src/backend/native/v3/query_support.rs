use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::v3::edge_compat::Direction;
use crate::pattern_engine::{PatternTriple, TripleMatch};
use crate::snapshot::SnapshotId;

impl V3Backend {
    pub(super) fn query_nodes_by_kind_support(
        &self,
        snapshot_id: SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;
        Ok(self.kind_index.get(kind))
    }

    pub(super) fn query_nodes_by_name_pattern_support(
        &self,
        snapshot_id: SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;

        if pattern.ends_with('*') && !pattern.starts_with('*') && !pattern.contains('?') {
            let prefix = &pattern[..pattern.len() - 1];
            Ok(self.name_index.get_prefix(prefix))
        } else if pattern.contains('*') || pattern.contains('?') {
            Ok(self.name_index.get_glob(pattern))
        } else {
            Ok(self.name_index.get_exact(pattern))
        }
    }

    pub(super) fn match_triples_support(
        &self,
        pattern: &PatternTriple,
    ) -> Result<Vec<TripleMatch>, SqliteGraphError> {
        pattern.validate()?;

        let mut matches = Vec::new();
        let v3_direction = match pattern.direction {
            crate::backend::BackendDirection::Outgoing => Direction::Outgoing,
            crate::backend::BackendDirection::Incoming => Direction::Incoming,
        };

        let start_candidates: Vec<i64> = if let Some(ref start_label) = pattern.start_label {
            self.kind_index.get(start_label)
        } else {
            self.get_all_node_ids()?
        };

        for start_id in start_candidates {
            let edge_store = self.edge_store.read();
            let neighbors =
                match edge_store.neighbors_filtered(start_id, v3_direction, &pattern.edge_type) {
                    Ok(neighbors) => neighbors,
                    Err(_) => continue,
                };

            drop(edge_store);

            for end_id in neighbors.iter() {
                if let Some(ref end_label) = pattern.end_label {
                    let label_matches = self.kind_index.get(end_label).contains(end_id);
                    if !label_matches {
                        continue;
                    }
                }

                if !self.check_property_filters(start_id, *end_id, pattern)? {
                    continue;
                }

                let edge_id =
                    self.generate_edge_id(start_id, *end_id, v3_direction, &pattern.edge_type);

                matches.push(TripleMatch::new(start_id, edge_id, *end_id));
            }
        }

        matches.sort_by(|a, b| {
            a.start_id
                .cmp(&b.start_id)
                .then_with(|| a.edge_id.cmp(&b.edge_id))
                .then_with(|| a.end_id.cmp(&b.end_id))
        });

        Ok(matches)
    }
}
