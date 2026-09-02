//! Batched write guard for V3 backend inserts.
//!
//! This guard amortizes durability costs by staging node and edge inserts and
//! flushing them with a single sync at commit.

use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::{EdgeSpec, NodeSpec};
use rusqlite::ToSql;

/// Write batch guard for amortized durability
///
/// Accumulates node/edge inserts in memory and performs a single
/// fsync at commit, matching SQLite in-transaction semantics.
pub struct WriteBatchGuard<'a> {
    backend: &'a V3Backend,
    node_count: u64,
    edge_count: u64,
    staged_nodes: Vec<(i64, String, String, serde_json::Value)>,
    staged_edges: Vec<EdgeSpec>,
    committed: bool,
}

impl<'a> WriteBatchGuard<'a> {
    /// Create a new write batch guard
    pub(crate) fn new(backend: &'a V3Backend) -> Self {
        {
            let mut node_store = backend.node_store.write();
            node_store.begin_batch();
        }

        Self {
            backend,
            node_count: 0,
            edge_count: 0,
            staged_nodes: Vec::new(),
            staged_edges: Vec::new(),
            committed: false,
        }
    }

    /// Insert a node without syncing (accumulated in batch)
    pub fn insert_node(&mut self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let kind = node.kind.clone();
        let name = node.name.clone();
        let data = node.data.clone();
        let node_id = self.backend.insert_node_inner(node)?;
        self.node_count += 1;
        self.staged_nodes.push((node_id, kind, name, data));
        Ok(node_id)
    }

    /// Insert a node with a manually specified ID without syncing (accumulated in batch)
    pub fn insert_node_with_id(
        &mut self,
        mut node: NodeSpec,
        node_id: i64,
    ) -> Result<i64, SqliteGraphError> {
        if let Some(obj) = node.data.as_object_mut() {
            obj.insert("id".to_string(), serde_json::Value::Number(node_id.into()));
        } else {
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), serde_json::Value::Number(node_id.into()));
            node.data = serde_json::Value::Object(obj);
        }
        let kind = node.kind.clone();
        let name = node.name.clone();
        let data = node.data.clone();
        let node_id = self.backend.insert_node_inner(node)?;
        self.node_count += 1;
        self.staged_nodes.push((node_id, kind, name, data));
        Ok(node_id)
    }

    /// Insert an edge without syncing (accumulated in batch)
    pub fn insert_edge(&mut self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        let edge_id = self.backend.insert_edge_inner(edge.clone())?;
        self.edge_count += 1;
        self.staged_edges.push(edge);
        Ok(edge_id)
    }

    /// Commit all accumulated writes with single fsync
    pub fn commit(mut self) -> Result<(), SqliteGraphError> {
        if self.committed {
            return Ok(());
        }

        if self.node_count > 0 {
            let mut node_store = self.backend.node_store.write();
            node_store
                .commit_batch()
                .map_err(|e| SqliteGraphError::connection(format!("Batch commit failed: {}", e)))?;
        }

        if self.node_count > 0 || self.edge_count > 0 {
            self.backend.sync_header()?;
            let next_version = self.backend.next_graph_version();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| {
                    SqliteGraphError::connection(format!(
                        "System clock before UNIX epoch during batch commit: {}",
                        e
                    ))
                })?
                .as_secs() as i64;

            if self.node_count > 0 {
                let conn = self.backend.sqlite_conn.lock();
                for (node_id, kind, name, data) in &self.staged_nodes {
                    let data_json = serde_json::to_string(data).unwrap_or_default();
                    conn.execute(
                        "INSERT INTO node_properties (node_id, kind, name, data, created_at, created_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        [
                            node_id as &dyn ToSql,
                            kind as &dyn ToSql,
                            name as &dyn ToSql,
                            &data_json as &dyn ToSql,
                            &timestamp as &dyn ToSql,
                            &next_version as &dyn ToSql,
                        ],
                    ).map_err(|e| SqliteGraphError::connection(format!("Failed to insert node properties: {}", e)))?;
                }
                drop(conn);
            }

            if self.edge_count > 0 {
                let conn = self.backend.sqlite_conn.lock();
                for edge in &self.staged_edges {
                    let data_json = serde_json::to_string(&edge.data).unwrap_or_default();
                    conn.execute(
                        "INSERT OR REPLACE INTO edge_attributes (src, dst, attr_name, attr_value, created_version) VALUES (?1, ?2, ?3, ?4, ?5)",
                        [
                            &edge.from as &dyn ToSql,
                            &edge.to as &dyn ToSql,
                            &edge.edge_type as &dyn ToSql,
                            &data_json as &dyn ToSql,
                            &next_version as &dyn ToSql,
                        ],
                    ).map_err(|e| SqliteGraphError::connection(format!("Failed to insert edge attributes: {}", e)))?;
                }
                drop(conn);
                self.backend.rebuild_csr_runtime_views(next_version)?;
            }

            self.backend.advance_snapshot_version();
            self.backend.flush_to_disk()?;
        }

        self.committed = true;
        Ok(())
    }

    /// Get number of nodes staged in this batch
    pub fn node_count(&self) -> u64 {
        self.node_count
    }

    /// Get number of edges staged in this batch
    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }
}

impl<'a> Drop for WriteBatchGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            let mut node_store = self.backend.node_store.write();
            node_store.rollback_batch();
        }
    }
}
