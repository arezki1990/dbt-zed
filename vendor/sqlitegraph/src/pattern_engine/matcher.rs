//! Main pattern matching logic and result types.

use crate::{errors::SqliteGraphError, graph::SqliteGraph};

use super::{
    pattern::PatternTriple,
    property::matches_property_filters,
    query::{execute_complex_edge_query, execute_simple_edge_query},
};

/// Result of a triple pattern match.
///
/// Represents a matched triple containing the start node, edge, and end node IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TripleMatch {
    /// ID of the start node
    pub start_id: i64,
    /// ID of the end node
    pub end_id: i64,
    /// ID of the matching edge
    pub edge_id: i64,
}

impl TripleMatch {
    /// Create a new triple match.
    pub fn new(start_id: i64, edge_id: i64, end_id: i64) -> Self {
        Self {
            start_id,
            end_id,
            edge_id,
        }
    }
}

/// Execute a lightweight triple pattern match.
///
/// This function provides a simple, deterministic way to match single-hop patterns
/// using the existing SQLite indexes for optimal performance.
///
/// # Arguments
/// * `graph` - The SQLiteGraph instance
/// * `pattern` - The pattern triple to match
///
/// # Returns
/// A vector of triple matches in deterministic order
pub fn match_triples(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    pattern.validate()?;

    let _conn = graph.connection();

    // Build and execute the query based on pattern complexity
    let matches = if pattern.start_label.is_none() && pattern.end_label.is_none() {
        // Simple case: no label filters
        execute_simple_edge_query(graph, pattern)?
    } else {
        // Complex case: with label filters
        execute_complex_edge_query(graph, pattern)?
    };

    // Apply property filters if specified
    let mut filtered_matches = Vec::new();
    for triple_match in matches {
        if matches_property_filters(graph, &triple_match, pattern)? {
            filtered_matches.push(triple_match);
        }
    }

    // Ensure deterministic ordering
    filtered_matches.sort_by(|a, b| {
        a.start_id
            .cmp(&b.start_id)
            .then_with(|| a.edge_id.cmp(&b.edge_id))
            .then_with(|| a.end_id.cmp(&b.end_id))
    });

    Ok(filtered_matches)
}
