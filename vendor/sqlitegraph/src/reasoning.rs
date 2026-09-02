use std::cmp::Ordering;

use crate::{
    SqliteGraphError, backend::BackendDirection, graph::SqliteGraph, multi_hop,
    pattern::PatternQuery,
};

#[derive(Clone, Debug)]
pub struct ReasoningConfig {
    pub expansion_depth: u32,
    pub direction: BackendDirection,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            expansion_depth: 2,
            direction: BackendDirection::Outgoing,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReasoningCandidate {
    pub root: i64,
    pub pattern_path: Vec<i64>,
    pub expansion: Vec<i64>,
    pub score: f64,
}

pub struct GraphReasoner<'a> {
    graph: &'a SqliteGraph,
}

impl<'a> GraphReasoner<'a> {
    pub fn new(graph: &'a SqliteGraph) -> Self {
        Self { graph }
    }

    pub fn analyze(
        &self,
        start: i64,
        pattern: &PatternQuery,
        config: &ReasoningConfig,
    ) -> Result<Vec<ReasoningCandidate>, SqliteGraphError> {
        let matches = self.graph.query().pattern_matches(start, pattern)?;
        let mut candidates = Vec::new();
        for pattern_match in matches {
            if pattern_match.nodes.is_empty() {
                continue;
            }
            let anchor = *pattern_match.nodes.last().unwrap_or(&start);
            let expansion = if config.expansion_depth == 0 {
                Vec::new()
            } else {
                multi_hop::k_hop(self.graph, anchor, config.expansion_depth, config.direction)?
            };
            let degree =
                self.graph.fetch_outgoing(anchor)?.len() + self.graph.fetch_incoming(anchor)?.len();
            let score = pattern_match.nodes.len() as f64 + expansion.len() as f64 + degree as f64;
            candidates.push(ReasoningCandidate {
                root: pattern_match.nodes[0],
                pattern_path: pattern_match.nodes.clone(),
                expansion,
                score,
            });
        }
        candidates.sort_by(compare_candidates);
        Ok(candidates)
    }
}

fn compare_candidates(a: &ReasoningCandidate, b: &ReasoningCandidate) -> Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(Ordering::Equal)
        .then(a.pattern_path.cmp(&b.pattern_path))
}

impl SqliteGraph {
    pub fn reasoner(&self) -> GraphReasoner<'_> {
        GraphReasoner::new(self)
    }
}
