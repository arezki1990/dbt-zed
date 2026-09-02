use serde::{Deserialize, Serialize};

use crate::errors::SqliteGraphError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEntity {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub file_path: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEdge {
    pub id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub edge_type: String,
    pub data: serde_json::Value,
}

/// Graph statistics for MVCC queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphStats {
    /// Total number of entities in the graph
    pub total_entities: i64,
    /// Total number of edges in the graph
    pub total_edges: i64,
    /// Entity counts grouped by kind (optional, can be empty)
    pub entity_counts: Vec<(String, i64)>,
    /// Edge counts grouped by type (optional, can be empty)
    pub edge_counts: Vec<(String, i64)>,
}

/// Snapshot metadata for MVCC tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotMetadata {
    /// Unique snapshot identifier
    pub snapshot_id: String,
    /// Unix timestamp when snapshot was created
    pub timestamp: i64,
    /// Snapshot description (optional)
    pub description: Option<String>,
}

pub fn validate_entity(entity: &GraphEntity) -> Result<(), SqliteGraphError> {
    if entity.kind.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("entity kind must be set"));
    }
    if entity.name.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("entity name must be set"));
    }
    Ok(())
}

pub fn validate_edge(edge: &GraphEdge) -> Result<(), SqliteGraphError> {
    if edge.edge_type.trim().is_empty() {
        return Err(SqliteGraphError::invalid_input("edge type must be set"));
    }
    if edge.from_id <= 0 || edge.to_id <= 0 {
        return Err(SqliteGraphError::invalid_input(
            "edge endpoints must be positive ids",
        ));
    }
    Ok(())
}

pub fn row_to_entity(row: &rusqlite::Row<'_>) -> Result<GraphEntity, rusqlite::Error> {
    let data: String = row.get(4)?;
    let value: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            data.len(),
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    Ok(GraphEntity {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        file_path: row.get(3)?,
        data: value,
    })
}

pub fn row_to_edge(row: &rusqlite::Row<'_>) -> Result<GraphEdge, rusqlite::Error> {
    let data: String = row.get(4)?;
    let value: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            data.len(),
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    Ok(GraphEdge {
        id: row.get(0)?,
        from_id: row.get(1)?,
        to_id: row.get(2)?,
        edge_type: row.get(3)?,
        data: value,
    })
}
