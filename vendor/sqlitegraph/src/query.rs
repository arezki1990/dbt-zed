use std::collections::VecDeque;

use ahash::AHashSet;
use rusqlite::params;

use crate::{
    backend::BackendDirection,
    errors::SqliteGraphError,
    graph::SqliteGraph,
    multi_hop::{self, ChainStep},
    pattern::{self, PatternMatch, PatternQuery},
};

pub struct GraphQuery<'a> {
    graph: &'a SqliteGraph,
}

impl<'a> GraphQuery<'a> {
    pub fn new(graph: &'a SqliteGraph) -> Self {
        Self { graph }
    }

    pub fn neighbors(&self, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
        self.graph.fetch_outgoing(id)
    }

    pub fn incoming(&self, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
        self.graph.fetch_incoming(id)
    }

    pub fn outgoing(&self, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
        self.graph.fetch_outgoing(id)
    }

    pub fn edges_of_type(&self, id: i64, edge_type: &str) -> Result<Vec<i64>, SqliteGraphError> {
        if edge_type.trim().is_empty() {
            return Err(SqliteGraphError::invalid_input("edge_type required"));
        }
        let conn = self.graph.connection();
        let mut stmt = conn
            .prepare_cached(
                "SELECT to_id FROM graph_edges WHERE from_id=?1 AND edge_type=?2 ORDER BY to_id, id",
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let rows = stmt
            .query_map(params![id, edge_type], |row| row.get(0))
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut ids = Vec::new();
        for entry in rows {
            ids.push(entry.map_err(|e| SqliteGraphError::query(e.to_string()))?);
        }
        Ok(ids)
    }

    pub fn has_path(&self, a: i64, b: i64, max_depth: u32) -> Result<bool, SqliteGraphError> {
        if a == b {
            return Ok(true);
        }
        if max_depth == 0 {
            return Ok(false);
        }
        let mut visited = AHashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back((a, 0));
        visited.insert(a);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for next in self.graph.fetch_outgoing(node)? {
                if next == b {
                    return Ok(true);
                }
                if visited.insert(next) {
                    queue.push_back((next, depth + 1));
                }
            }
        }
        Ok(false)
    }

    pub fn k_hop_outgoing(&self, start: i64, depth: u32) -> Result<Vec<i64>, SqliteGraphError> {
        multi_hop::k_hop(self.graph, start, depth, BackendDirection::Outgoing)
    }

    pub fn k_hop_direction(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        multi_hop::k_hop(self.graph, start, depth, direction)
    }

    pub fn k_hop_filtered(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        multi_hop::k_hop_filtered(self.graph, start, depth, direction, allowed_edge_types)
    }

    pub fn chain(&self, start: i64, chain: &[ChainStep]) -> Result<Vec<i64>, SqliteGraphError> {
        multi_hop::chain_query(self.graph, start, chain)
    }

    pub fn pattern_matches(
        &self,
        start: i64,
        pattern_query: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        pattern::execute_pattern(self.graph, start, pattern_query)
    }
}

impl SqliteGraph {
    pub fn query(&self) -> GraphQuery<'_> {
        GraphQuery::new(self)
    }
}
