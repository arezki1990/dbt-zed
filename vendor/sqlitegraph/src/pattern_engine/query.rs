//! SQL query execution for pattern matching.

use rusqlite::params;

use crate::{backend::BackendDirection, errors::SqliteGraphError, graph::SqliteGraph};

use super::matcher::TripleMatch;
use super::pattern::PatternTriple;

/// Execute simple edge query without label filters.
pub fn execute_simple_edge_query(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    let conn = graph.connection();

    let sql = match pattern.direction {
        BackendDirection::Outgoing => {
            "SELECT ge.from_id, ge.id, ge.to_id FROM graph_edges ge WHERE ge.edge_type = ?1 ORDER BY ge.from_id, ge.id, ge.to_id"
        }
        BackendDirection::Incoming => {
            "SELECT ge.to_id, ge.id, ge.from_id FROM graph_edges ge WHERE ge.edge_type = ?1 ORDER BY ge.to_id, ge.id, ge.from_id"
        }
    };

    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;

    let rows = stmt
        .query_map(params![&pattern.edge_type], |row| {
            Ok(TripleMatch::new(
                row.get(0)?, // start_id
                row.get(1)?, // edge_id
                row.get(2)?, // end_id
            ))
        })
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;

    let mut matches = Vec::new();
    for row in rows {
        matches.push(row.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }

    Ok(matches)
}

/// Execute complex edge query with label filters.
pub fn execute_complex_edge_query(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    let conn = graph.connection();

    // Build SQL dynamically based on which labels are present
    let mut sql = match pattern.direction {
        BackendDirection::Outgoing => {
            "SELECT ge.from_id, ge.id, ge.to_id FROM graph_edges ge".to_string()
        }
        BackendDirection::Incoming => {
            "SELECT ge.to_id, ge.id, ge.from_id FROM graph_edges ge".to_string()
        }
    };

    sql.push_str(" WHERE ge.edge_type = ?1");

    let mut param_count = 1;

    // Add start label filter
    if let Some(_start_label) = &pattern.start_label {
        param_count += 1;
        sql.push_str(" AND EXISTS (");
        sql.push_str("  SELECT 1 FROM graph_labels gl");
        sql.push_str("  WHERE gl.entity_id = ");
        sql.push_str(if pattern.direction == BackendDirection::Outgoing {
            "ge.from_id"
        } else {
            "ge.to_id"
        });
        sql.push_str(&format!("  AND gl.label = ?{}", param_count));
        sql.push_str(" )");
    }

    // Add end label filter
    if let Some(_end_label) = &pattern.end_label {
        param_count += 1;
        sql.push_str(" AND EXISTS (");
        sql.push_str("  SELECT 1 FROM graph_labels gl");
        sql.push_str("  WHERE gl.entity_id = ");
        sql.push_str(if pattern.direction == BackendDirection::Outgoing {
            "ge.to_id"
        } else {
            "ge.from_id"
        });
        sql.push_str(&format!("  AND gl.label = ?{}", param_count));
        sql.push_str(" )");
    }

    // Add deterministic ordering
    sql.push_str(" ORDER BY ");
    if pattern.direction == BackendDirection::Outgoing {
        sql.push_str("ge.from_id, ge.id, ge.to_id");
    } else {
        sql.push_str("ge.to_id, ge.id, ge.from_id");
    }

    // Execute query with appropriate parameters
    let matches =
        if let (Some(start_label), Some(end_label)) = (&pattern.start_label, &pattern.end_label) {
            let mut stmt = conn
                .prepare_cached(&sql)
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let rows = stmt
                .query_map(params![&pattern.edge_type, start_label, end_label], |row| {
                    Ok(TripleMatch::new(row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            collect_triple_matches(rows)?
        } else if let Some(start_label) = &pattern.start_label {
            let mut stmt = conn
                .prepare_cached(&sql)
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let rows = stmt
                .query_map(params![&pattern.edge_type, start_label], |row| {
                    Ok(TripleMatch::new(row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            collect_triple_matches(rows)?
        } else if let Some(end_label) = &pattern.end_label {
            let mut stmt = conn
                .prepare_cached(&sql)
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let rows = stmt
                .query_map(params![&pattern.edge_type, end_label], |row| {
                    Ok(TripleMatch::new(row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            collect_triple_matches(rows)?
        } else {
            unreachable!("This case should be handled by execute_simple_edge_query")
        };

    Ok(matches)
}

/// Collect triple matches from query rows.
fn collect_triple_matches(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<TripleMatch>>,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    let mut matches = Vec::new();
    for row in rows {
        matches.push(row.map_err(|e| SqliteGraphError::query(e.to_string()))?);
    }
    Ok(matches)
}
