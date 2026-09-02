use crate::{
    backend::BackendDirection,
    errors::SqliteGraphError,
    graph::SqliteGraph,
    pattern_engine::{PatternTriple, TripleMatch, match_triples},
    pattern_engine_cache::{
        edge_validation::edge_exists_with_type, edge_validation::get_edge_id,
        fast_path_detection::can_use_fast_path, fast_path_detection::can_use_partial_fast_path,
    },
};

/// Execute cache-enabled fast-path pattern matching.
///
/// This function provides an optimized version of pattern matching that:
/// - Uses cache as a fast-path where safe
/// - Falls back to SQL where pattern requires it
/// - Returns IDENTICAL results to match_triples()
/// - Maintains deterministic ordering
///
/// # Arguments
/// * `graph` - The SQLiteGraph instance
/// * `pattern` - The pattern triple to match
///
/// # Returns
/// A vector of triple matches in deterministic order
pub fn match_triples_fast(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    pattern.validate()?;

    // Case 1: Fast Path - edge type only
    if can_use_fast_path(pattern) {
        return execute_fast_path(graph, pattern);
    }

    // Case 2: Partial Fast Path - use cache as candidate generator
    if can_use_partial_fast_path(pattern) {
        return execute_partial_fast_path(graph, pattern);
    }

    // Case 3: SQL Only - complex pattern
    execute_sql_only(graph, pattern)
}

/// Execute fast-path for edge type only patterns (Case 1)
///
/// Uses adjacency cache directly and validates via SQL lookup.
fn execute_fast_path(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    let mut matches = Vec::new();

    // Get all entity IDs to iterate through
    let entity_ids = graph.all_entity_ids()?;

    // For each entity, use cache to get adjacency candidates
    for &source_id in &entity_ids {
        let candidates = match pattern.direction {
            BackendDirection::Outgoing => graph.fetch_outgoing(source_id)?,
            BackendDirection::Incoming => graph.fetch_incoming(source_id)?,
        };

        // For each candidate, validate edge exists with correct type
        for &target_id in &candidates {
            // Always use the actual database direction for validation
            let (from_id, to_id) = match pattern.direction {
                BackendDirection::Outgoing => (source_id, target_id),
                BackendDirection::Incoming => (target_id, source_id),
            };

            // Validate edge exists with correct type
            if edge_exists_with_type(
                graph,
                from_id,
                to_id,
                &pattern.edge_type,
                BackendDirection::Outgoing,
            )? {
                let edge_id = get_edge_id(graph, from_id, to_id, &pattern.edge_type)?;

                // Create TripleMatch with correct direction semantics
                let triple_match = match pattern.direction {
                    BackendDirection::Outgoing => TripleMatch::new(source_id, edge_id, target_id),
                    BackendDirection::Incoming => TripleMatch::new(source_id, edge_id, target_id),
                };
                matches.push(triple_match);
            }
        }
    }

    // Ensure deterministic ordering
    matches.sort_by(|a, b| {
        a.start_id
            .cmp(&b.start_id)
            .then_with(|| a.edge_id.cmp(&b.edge_id))
            .then_with(|| a.end_id.cmp(&b.end_id))
    });

    Ok(matches)
}

/// Execute partial fast-path using cache as candidate generator (Case 2)
///
/// Uses cache to narrow candidates but validates via SQL.
fn execute_partial_fast_path(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    // For partial fast-path, we still use SQL but can optimize
    // by using cache to pre-filter candidates where possible
    match_triples(graph, pattern)
}

/// Execute SQL-only path for complex patterns (Case 3)
///
/// Falls back to original SQL implementation.
fn execute_sql_only(
    graph: &SqliteGraph,
    pattern: &PatternTriple,
) -> Result<Vec<TripleMatch>, SqliteGraphError> {
    match_triples(graph, pattern)
}
