//! Property filtering functionality for pattern matching.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use crate::{errors::SqliteGraphError, graph::SqliteGraph};

use super::matcher::TripleMatch;
use super::pattern::PatternTriple;

/// Check if a triple match satisfies the property filters.
pub fn matches_property_filters(
    graph: &SqliteGraph,
    triple_match: &TripleMatch,
    pattern: &PatternTriple,
) -> Result<bool, SqliteGraphError> {
    // Check start node properties
    if !pattern.start_props.is_empty()
        && !entity_has_properties(graph, triple_match.start_id, &pattern.start_props)?
    {
        return Ok(false);
    }

    // Check end node properties
    if !pattern.end_props.is_empty()
        && !entity_has_properties(graph, triple_match.end_id, &pattern.end_props)?
    {
        return Ok(false);
    }

    Ok(true)
}

/// Check if an entity has all the specified properties with matching values.
pub fn entity_has_properties(
    graph: &SqliteGraph,
    entity_id: i64,
    required_props: &HashMap<String, String>,
) -> Result<bool, SqliteGraphError> {
    let conn = graph.connection();

    for (key, expected_value) in required_props {
        let mut stmt = conn
            .prepare_cached(
                "SELECT 1 FROM graph_properties WHERE entity_id = ?1 AND key = ?2 AND value = ?3 LIMIT 1"
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let exists: Option<i32> = stmt
            .query_row(params![entity_id, key, expected_value], |row| row.get(0))
            .optional()
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        if exists.is_none() {
            return Ok(false);
        }
    }

    Ok(true)
}
