use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::{GraphBackend, NeighborQuery, PatternMatch, PatternQuery};
use crate::graph::GraphEntity;
use crate::snapshot::SnapshotId;
use std::collections::HashMap;

impl V3Backend {
    pub(super) fn pattern_search_matches(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        Self::require_current_snapshot(snapshot_id)?;

        if let Some(root_constraint) = &pattern.root {
            let root = self.get_node(snapshot_id, start)?;
            if !root_constraint.matches(&root) {
                return Ok(Vec::new());
            }
        }

        let mut cache: HashMap<i64, GraphEntity> = HashMap::new();
        let mut sequences: Vec<Vec<i64>> = vec![vec![start]];

        for leg in &pattern.legs {
            let mut next_sequences = Vec::new();

            for sequence in &sequences {
                let current = *sequence.last().expect("sequence non-empty");
                let neighbors = self.neighbors(
                    snapshot_id,
                    current,
                    NeighborQuery {
                        direction: leg.direction,
                        edge_type: leg.edge_type.clone(),
                    },
                )?;

                for neighbor in neighbors {
                    let matches_constraint = match leg.constraint.as_ref() {
                        None => true,
                        Some(constraint) => {
                            let entity = if let Some(entity) = cache.get(&neighbor) {
                                entity.clone()
                            } else {
                                let entity = self.get_node(snapshot_id, neighbor)?;
                                cache.insert(neighbor, entity.clone());
                                entity
                            };
                            constraint.matches(&entity)
                        }
                    };

                    if matches_constraint {
                        let mut next = sequence.clone();
                        next.push(neighbor);
                        next_sequences.push(next);
                    }
                }
            }

            if next_sequences.is_empty() {
                return Ok(Vec::new());
            }

            next_sequences.sort();
            next_sequences.dedup();
            sequences = next_sequences;
        }

        let mut matches: Vec<PatternMatch> = sequences
            .into_iter()
            .map(|nodes| PatternMatch { nodes })
            .collect();
        matches.sort_by(|a, b| a.nodes.cmp(&b.nodes));
        Ok(matches)
    }
}
