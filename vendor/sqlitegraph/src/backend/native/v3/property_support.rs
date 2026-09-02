use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::GraphBackend;
use crate::backend::native::v3::edge_compat::Direction;
use crate::graph::GraphEntity;
use crate::pattern_engine::PatternTriple;
use crate::snapshot::SnapshotId;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

impl V3Backend {
    /// Helper: Get all node IDs in the graph (expensive, use with caution)
    pub(super) fn get_all_node_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        let all_kinds = self.kind_index.all_kinds();
        let mut all_ids = Vec::new();
        for kind in all_kinds {
            let kind_ids = self.kind_index.get(&kind);
            all_ids.extend(kind_ids);
        }

        all_ids.sort();
        all_ids.dedup();

        Ok(all_ids)
    }

    /// Helper: Check property filters for start and end nodes
    pub(super) fn check_property_filters(
        &self,
        start_id: i64,
        end_id: i64,
        pattern: &PatternTriple,
    ) -> Result<bool, SqliteGraphError> {
        if !pattern.start_props.is_empty() {
            let start_node = self.get_node(SnapshotId::current(), start_id)?;
            if !self.node_matches_properties(&start_node, &pattern.start_props) {
                return Ok(false);
            }
        }

        if !pattern.end_props.is_empty() {
            let end_node = self.get_node(SnapshotId::current(), end_id)?;
            if !self.node_matches_properties(&end_node, &pattern.end_props) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Helper: Check if a node's properties match the required filters
    pub(super) fn node_matches_properties(
        &self,
        node: &GraphEntity,
        required_props: &HashMap<String, String>,
    ) -> bool {
        for (key, expected_value) in required_props {
            match key.as_str() {
                "kind" if node.kind == *expected_value => continue,
                "name" if node.name == *expected_value => continue,
                "file_path" if node.file_path.as_deref() == Some(expected_value.as_str()) => {
                    continue;
                }
                _ => {}
            }

            let data_obj = match &node.data {
                Value::Object(obj) if !obj.is_empty() => obj,
                _ => return false,
            };

            match data_obj.get(key) {
                Some(Value::String(actual_value)) if actual_value == expected_value => continue,
                Some(Value::Number(n)) if n.to_string() == *expected_value => continue,
                Some(Value::Bool(b)) if b.to_string() == *expected_value => continue,
                _ => return false,
            }
        }

        true
    }

    /// Helper: Generate synthetic edge_id for triple matching
    ///
    /// V3 edge store uses (src, dst, dir) tuples, not traditional edge_id.
    /// This generates a deterministic edge_id for compatibility.
    pub(super) fn generate_edge_id(
        &self,
        src: i64,
        dst: i64,
        direction: Direction,
        edge_type: &str,
    ) -> i64 {
        let mut hasher = DefaultHasher::new();
        src.hash(&mut hasher);
        dst.hash(&mut hasher);
        direction.hash(&mut hasher);
        edge_type.hash(&mut hasher);

        let hash = hasher.finish();
        let edge_id = (hash % i64::MAX as u64) as i64;

        edge_id.max(1)
    }

    /// Get node properties from SQLite
    pub fn get_node_properties(
        &self,
        node_id: i64,
    ) -> Result<Option<(String, String, Value)>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        let mut stmt = conn
            .prepare("SELECT kind, name, data FROM node_properties WHERE node_id = ?1")
            .map_err(|e| SqliteGraphError::connection(format!("Failed to prepare query: {}", e)))?;

        let mut rows = stmt
            .query([node_id])
            .map_err(|e| SqliteGraphError::connection(format!("Failed to query: {}", e)))?;

        let result = rows
            .next()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to fetch row: {}", e)))?;

        match result {
            Some(row) => {
                let kind: String = row.get(0).map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to get kind: {}", e))
                })?;
                let name: String = row.get(1).map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to get name: {}", e))
                })?;
                let data_str: String = row.get(2).map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to get data: {}", e))
                })?;
                let data = serde_json::from_str(&data_str).unwrap_or(Value::Null);
                Ok(Some((kind, name, data)))
            }
            None => Ok(None),
        }
    }

    /// Get edge attributes from SQLite
    pub fn get_edge_attributes(
        &self,
        src: i64,
        dst: i64,
    ) -> Result<Vec<(String, Value)>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT attr_name, attr_value FROM edge_attributes WHERE src = ?1 AND dst = ?2",
            )
            .map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to prepare edge attributes query: {}",
                    e
                ))
            })?;

        let mut attributes = Vec::new();
        let mut rows = stmt.query([src, dst]).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to query edge attributes: {}", e))
        })?;

        loop {
            let result = rows
                .next()
                .map_err(|e| SqliteGraphError::connection(format!("Failed to fetch row: {}", e)))?;
            match result {
                Some(row) => {
                    let attr_name: String = row.get(0).map_err(|e| {
                        SqliteGraphError::connection(format!("Failed to get attr_name: {}", e))
                    })?;
                    let attr_value_str: String = row.get(1).map_err(|e| {
                        SqliteGraphError::connection(format!("Failed to get attr_value: {}", e))
                    })?;
                    let attr_value = serde_json::from_str(&attr_value_str).unwrap_or(Value::Null);
                    attributes.push((attr_name, attr_value));
                }
                None => break,
            }
        }

        Ok(attributes)
    }
}
