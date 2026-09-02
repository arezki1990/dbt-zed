use rusqlite::params;

use crate::{
    SqliteGraphError,
    graph::{GraphEntity, SqliteGraph},
};

pub fn add_label(graph: &SqliteGraph, entity_id: i64, label: &str) -> Result<(), SqliteGraphError> {
    graph
        .connection()
        .execute(
            "INSERT OR IGNORE INTO graph_labels(entity_id, label) VALUES(?1, ?2)",
            params![entity_id, label],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    Ok(())
}

pub fn get_entities_by_label(
    graph: &SqliteGraph,
    label: &str,
) -> Result<Vec<GraphEntity>, SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT entity_id FROM graph_labels WHERE label=?1 ORDER BY entity_id")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![label], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }
    fetch_entities(graph, ids)
}

pub fn add_property(
    graph: &SqliteGraph,
    entity_id: i64,
    key: &str,
    value: &str,
) -> Result<(), SqliteGraphError> {
    graph
        .connection()
        .execute(
            "INSERT INTO graph_properties(entity_id, key, value) VALUES(?1, ?2, ?3)",
            params![entity_id, key, value],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    Ok(())
}

pub fn get_entities_by_property(
    graph: &SqliteGraph,
    key: &str,
    value: &str,
) -> Result<Vec<GraphEntity>, SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached(
            "SELECT entity_id FROM graph_properties \
             WHERE key=?1 AND value=?2 ORDER BY entity_id",
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map(params![key, value], |row| row.get(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }
    fetch_entities(graph, ids)
}

fn fetch_entities(
    graph: &SqliteGraph,
    ids: Vec<i64>,
) -> Result<Vec<GraphEntity>, SqliteGraphError> {
    let mut entities = Vec::new();
    for id in ids {
        entities.push(graph.get_entity(id)?);
    }
    Ok(entities)
}
