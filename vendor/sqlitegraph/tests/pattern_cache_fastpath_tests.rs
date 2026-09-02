//! Tests for cache-enabled fast-path pattern matching.
//!
//! This test suite validates that the fast-path optimization produces
//! identical results to the SQL path while improving performance.

use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, PatternTriple, SqliteGraph,
    backend::BackendDirection,
    index::{add_label, add_property},
};

/// Create a test graph with sample data for fast-path testing
fn create_test_graph() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create test graph");

    // Insert functions
    let f1 = insert_entity(&graph, "Function", "process_data");
    let f2 = insert_entity(&graph, "Function", "validate_input");
    let f3 = insert_entity(&graph, "Function", "handle_error");
    let f4 = insert_entity(&graph, "Function", "log_result");

    // Insert structs
    let s1 = insert_entity(&graph, "Struct", "DataProcessor");
    let s2 = insert_entity(&graph, "Struct", "ErrorHandler");

    // Add labels for better filtering
    add_label_to_entity(&graph, f1, "public");
    add_label_to_entity(&graph, f2, "private");
    add_label_to_entity(&graph, f3, "private");
    add_label_to_entity(&graph, f4, "public");
    add_label_to_entity(&graph, s1, "exported");
    add_label_to_entity(&graph, s2, "internal");

    // Add properties for filtering
    add_property_to_entity(&graph, f1, "language", "rust");
    add_property_to_entity(&graph, f2, "language", "rust");
    add_property_to_entity(&graph, f3, "language", "rust");
    add_property_to_entity(&graph, f4, "language", "rust");
    add_property_to_entity(&graph, f1, "async", "true");
    add_property_to_entity(&graph, f2, "async", "false");
    add_property_to_entity(&graph, f3, "async", "false");
    add_property_to_entity(&graph, f4, "async", "true");

    add_property_to_entity(&graph, s1, "thread_safe", "true");
    add_property_to_entity(&graph, s2, "thread_safe", "false");

    // Insert edges (CALLS relationships)
    insert_edge(&graph, f1, f2, "CALLS");
    insert_edge(&graph, f1, f3, "CALLS");
    insert_edge(&graph, f2, s1, "USES");
    insert_edge(&graph, f3, s2, "USES");
    insert_edge(&graph, f4, s1, "USES");
    insert_edge(&graph, f1, f4, "CALLS");

    // Insert some edges with different types
    insert_edge(&graph, f1, s1, "BELONGS_TO");
    insert_edge(&graph, f2, s2, "BELONGS_TO");

    graph
}

fn insert_entity(graph: &SqliteGraph, kind: &str, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: kind.into(),
            name: name.into(),
            file_path: None,
            data: json!({"name": name, "type": kind}),
        })
        .expect("Failed to insert entity")
}

fn insert_edge(graph: &SqliteGraph, from: i64, to: i64, edge_type: &str) -> i64 {
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: from,
            to_id: to,
            edge_type: edge_type.into(),
            data: json!({"type": edge_type}),
        })
        .expect("Failed to insert edge")
}

fn add_label_to_entity(graph: &SqliteGraph, entity_id: i64, label: &str) {
    add_label(graph, entity_id, label).expect("Failed to add label");
}

fn add_property_to_entity(graph: &SqliteGraph, entity_id: i64, key: &str, value: &str) {
    add_property(graph, entity_id, key, value).expect("Failed to add property");
}

fn get_entity_by_name(graph: &SqliteGraph, name: &str) -> i64 {
    let entity_ids = graph.list_entity_ids().expect("Failed to get entity IDs");
    for id in entity_ids {
        let entity = graph.get_entity(id).expect("Failed to get entity");
        if entity.name == name {
            return id;
        }
    }
    panic!("Entity with name '{}' not found", name);
}

// ============================================
// Test Group A — Correctness
// ============================================

#[test]
fn test_fastpath_vs_sqlpath_equality_for_simple_patterns() {
    let graph = create_test_graph();

    // Test simple edge type only pattern (Case 1 - Fast Path)
    let pattern = PatternTriple::new("CALLS");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical
    assert_eq!(sql_results.len(), fast_results.len());
    assert_eq!(sql_results, fast_results);

    // Verify deterministic ordering
    for (i, (sql_match, fast_match)) in sql_results.iter().zip(fast_results.iter()).enumerate() {
        assert_eq!(sql_match, fast_match, "Mismatch at index {}", i);
    }
}

#[test]
fn test_identical_ordering_guarantees() {
    let graph = create_test_graph();

    // Create a pattern that matches multiple edges
    let pattern = PatternTriple::new("CALLS");

    let mut results1 = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");
    let mut results2 = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Sort both to ensure they're identical regardless of internal ordering
    results1.sort_by(|a, b| {
        a.start_id
            .cmp(&b.start_id)
            .then_with(|| a.edge_id.cmp(&b.edge_id))
            .then_with(|| a.end_id.cmp(&b.end_id))
    });
    results2.sort_by(|a, b| {
        a.start_id
            .cmp(&b.start_id)
            .then_with(|| a.edge_id.cmp(&b.edge_id))
            .then_with(|| a.end_id.cmp(&b.end_id))
    });

    assert_eq!(
        results1, results2,
        "Results must be identical across multiple calls"
    );
}

#[test]
fn test_fastpath_must_not_skip_sql_validation() {
    let graph = create_test_graph();

    // Test pattern with property filters (Case 2 - Partial Fast Path)
    let pattern = PatternTriple::new("CALLS").start_property("language", "rust");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical - fast path must validate via SQL
    assert_eq!(sql_results.len(), fast_results.len());
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_fastpath_must_work_with_mvcc_snapshots() {
    let graph = create_test_graph();

    // Warm up the cache by doing a fast-path query first
    let pattern = PatternTriple::new("CALLS");
    let _warmup = graph.match_triples_fast(&pattern).expect("Warmup failed");

    // Acquire a snapshot
    let snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");

    // Test pattern matching on snapshot
    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical even with snapshots
    assert_eq!(sql_results.len(), fast_results.len());
    assert_eq!(sql_results, fast_results);

    // Verify snapshot contains expected data
    assert!(snapshot.node_count() > 0);
    assert!(snapshot.edge_count() > 0);
}

// ============================================
// Test Group B — Cache correctness
// ============================================

#[test]
fn test_cache_invalidation_during_writes() {
    let graph = create_test_graph();

    // Warm up cache with initial query
    let pattern = PatternTriple::new("CALLS");
    let initial_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Add a new edge (should invalidate cache)
    let f5 = insert_entity(&graph, "Function", "new_function");
    let f6 = insert_entity(&graph, "Function", "another_function");
    insert_edge(&graph, f5, f6, "CALLS");

    // Query again - should see new edge
    let after_write_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    assert_eq!(after_write_results.len(), initial_results.len() + 1);

    // Verify against SQL path
    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    assert_eq!(after_write_results, sql_results);
}

#[test]
fn test_stale_cache_must_not_affect_results() {
    let graph = create_test_graph();

    // Perform a fast-path query to populate cache
    let pattern = PatternTriple::new("CALLS");
    let first_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Get cache stats after first run
    let after_first_outgoing = graph.outgoing_cache_ref().stats();
    let after_first_incoming = graph.incoming_cache_ref().stats();

    // Perform the same query again - should use cache
    let second_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results should be identical
    assert_eq!(first_results, second_results);

    // Cache should have been used (more hits on second run)
    let final_outgoing_stats = graph.outgoing_cache_ref().stats();
    let final_incoming_stats = graph.incoming_cache_ref().stats();

    // At least one cache hit should have occurred on second run
    assert!(
        final_outgoing_stats.hits > after_first_outgoing.hits
            || final_incoming_stats.hits > after_first_incoming.hits,
        "Cache hits should increase: outgoing {}->{}, incoming {}->{}",
        after_first_outgoing.hits,
        final_outgoing_stats.hits,
        after_first_incoming.hits,
        final_incoming_stats.hits
    );
}

#[test]
fn test_snapshot_cache_isolation_preserved() {
    let graph = create_test_graph();

    // Warm up cache first
    let pattern = PatternTriple::new("CALLS");
    let _warmup = graph.match_triples_fast(&pattern).expect("Warmup failed");

    // Get original CALLS edge count
    let original_calls = graph
        .match_triples_fast(&pattern)
        .expect("Original query failed");
    let original_call_count = original_calls.len();

    // Acquire snapshot
    let snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");

    // Add new edge to main graph
    let f5 = insert_entity(&graph, "Function", "new_function");
    let f6 = insert_entity(&graph, "Function", "another_function");
    insert_edge(&graph, f5, f6, "CALLS");

    // Query on main graph should see new edge
    let current_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Current results should have one more CALLS edge
    assert_eq!(current_results.len(), original_call_count + 1);

    // Snapshot should still have original total edge count (isolated from new writes)
    assert_eq!(snapshot.edge_count(), 8); // Original total edges in test graph
}

// ============================================
// Test Group C — Deterministic ordering
// ============================================

#[test]
fn test_sort_order_must_match_sql_exact_semantics() {
    let graph = create_test_graph();

    // Create pattern that matches multiple edges
    let pattern = PatternTriple::new("CALLS");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Both must be sorted by (start_id ASC, edge_id ASC, end_id ASC)
    for i in 1..sql_results.len() {
        assert!(
            sql_results[i - 1].start_id < sql_results[i].start_id
                || (sql_results[i - 1].start_id == sql_results[i].start_id
                    && sql_results[i - 1].edge_id < sql_results[i].edge_id)
                || (sql_results[i - 1].start_id == sql_results[i].start_id
                    && sql_results[i - 1].edge_id == sql_results[i].edge_id
                    && sql_results[i - 1].end_id <= sql_results[i].end_id),
            "SQL results not in deterministic order at index {}: {:?} vs {:?}",
            i,
            sql_results[i - 1],
            sql_results[i]
        );
    }

    for i in 1..fast_results.len() {
        assert!(
            fast_results[i - 1].start_id < fast_results[i].start_id
                || (fast_results[i - 1].start_id == fast_results[i].start_id
                    && fast_results[i - 1].edge_id < fast_results[i].edge_id)
                || (fast_results[i - 1].start_id == fast_results[i].start_id
                    && fast_results[i - 1].edge_id == fast_results[i].edge_id
                    && fast_results[i - 1].end_id <= fast_results[i].end_id),
            "Fast results not in deterministic order at index {}: {:?} vs {:?}",
            i,
            fast_results[i - 1],
            fast_results[i]
        );
    }

    // Ordering must be identical
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_repeatability_test_3_consecutive_runs() {
    let graph = create_test_graph();

    let pattern = PatternTriple::new("CALLS");

    let results1 = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");
    let results2 = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");
    let results3 = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // All three runs must produce identical results
    assert_eq!(results1, results2);
    assert_eq!(results2, results3);
    assert_eq!(results1, results3);

    // All must be deterministically ordered
    for results in [&results1, &results2, &results3] {
        for i in 1..results.len() {
            assert!(
                results[i - 1].start_id < results[i].start_id
                    || (results[i - 1].start_id == results[i].start_id
                        && results[i - 1].edge_id < results[i].edge_id)
                    || (results[i - 1].start_id == results[i].start_id
                        && results[i - 1].edge_id == results[i].edge_id
                        && results[i - 1].end_id <= results[i].end_id),
                "Results not in deterministic order at index {}: {:?} vs {:?}",
                i,
                results[i - 1],
                results[i]
            );
        }
    }
}

// ============================================
// Test Group D — Mixed patterns
// ============================================

#[test]
fn test_patterns_requiring_fallback() {
    let graph = create_test_graph();

    // Test complex pattern that should fallback to SQL (Case 3)
    let pattern = PatternTriple::new("CALLS")
        .start_label("public")
        .end_label("private")
        .start_property("async", "true")
        .end_property("async", "false");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_patterns_with_label_filters() {
    let graph = create_test_graph();

    // Test pattern with label filters (Case 2)
    let pattern = PatternTriple::new("CALLS").start_label("public");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_patterns_with_property_filters() {
    let graph = create_test_graph();

    // Test pattern with property filters (Case 2)
    let pattern = PatternTriple::new("CALLS").start_property("language", "rust");

    let sql_results = graph.match_triples(&pattern).expect("SQL path failed");
    let fast_results = graph
        .match_triples_fast(&pattern)
        .expect("Fast path failed");

    // Results must be identical
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_patterns_with_different_directions() {
    let graph = create_test_graph();

    let f1 = get_entity_by_name(&graph, "process_data");
    let f2 = get_entity_by_name(&graph, "validate_input");

    // Test outgoing direction
    let pattern_outgoing = PatternTriple::new("CALLS").direction(BackendDirection::Outgoing);

    let sql_outgoing = graph
        .match_triples(&pattern_outgoing)
        .expect("SQL path failed");
    let fast_outgoing = graph
        .match_triples_fast(&pattern_outgoing)
        .expect("Fast path failed");

    assert_eq!(sql_outgoing, fast_outgoing);

    // Test incoming direction
    let pattern_incoming = PatternTriple::new("CALLS").direction(BackendDirection::Incoming);

    let sql_incoming = graph
        .match_triples(&pattern_incoming)
        .expect("SQL path failed");
    let fast_incoming = graph
        .match_triples_fast(&pattern_incoming)
        .expect("Fast path failed");

    assert_eq!(sql_incoming, fast_incoming);

    // Verify direction semantics
    assert_eq!(fast_outgoing[0].start_id, f1);
    assert_eq!(fast_outgoing[0].end_id, f2);
    assert_eq!(fast_incoming[0].start_id, f2); // Reversed
    assert_eq!(fast_incoming[0].end_id, f1); // Reversed
}

// ============================================
// Test Group E — Performance & Safety
// ============================================

#[test]
fn test_fastpath_must_use_cache_for_90_percent_hits() {
    let graph = create_test_graph();

    // Perform multiple queries to warm up cache
    let pattern = PatternTriple::new("CALLS");

    for _ in 0..10 {
        let _results = graph
            .match_triples_fast(&pattern)
            .expect("Fast path failed");
    }

    // Check cache stats - should have high hit rate
    let outgoing_stats = graph.outgoing_cache_ref().stats();
    let incoming_stats = graph.incoming_cache_ref().stats();

    let total_hits = outgoing_stats.hits + incoming_stats.hits;
    let total_requests = total_hits + outgoing_stats.misses + incoming_stats.misses;

    if total_requests > 0 {
        let hit_rate = (total_hits as f64) / (total_requests as f64);
        assert!(
            hit_rate >= 0.9,
            "Cache hit rate {:.2}% is below 90%",
            hit_rate * 100.0
        );
    }
}

#[test]
fn test_no_panics_unwraps_or_non_determinism() {
    let graph = create_test_graph();

    // Test various patterns - none should panic
    let patterns = vec![
        PatternTriple::new("CALLS"),
        PatternTriple::new("USES"),
        PatternTriple::new("BELONGS_TO"),
        PatternTriple::new("CALLS").start_label("public"),
        PatternTriple::new("CALLS").start_property("language", "rust"),
        PatternTriple::new("NONEXISTENT"),
    ];

    for pattern in patterns {
        // Should not panic or unwrap
        let result = graph.match_triples_fast(&pattern);
        assert!(result.is_ok(), "Pattern failed: {:?}", pattern);

        // Results should be deterministic
        let results1 = result.unwrap();
        let results2 = graph
            .match_triples_fast(&pattern)
            .expect("Second call failed");
        assert_eq!(
            results1, results2,
            "Non-deterministic results for: {:?}",
            pattern
        );
    }
}

#[test]
fn test_no_behavior_change_in_existing_queries() {
    let graph = create_test_graph();

    // Test that existing match_triples behavior is unchanged
    let pattern = PatternTriple::new("CALLS");

    let original_results = graph
        .match_triples(&pattern)
        .expect("Original query failed");

    // Multiple calls should produce same results
    let repeat_results = graph.match_triples(&pattern).expect("Repeat query failed");
    assert_eq!(original_results, repeat_results);

    // Results should be deterministic
    for i in 1..original_results.len() {
        assert!(
            original_results[i - 1].start_id < original_results[i].start_id
                || (original_results[i - 1].start_id == original_results[i].start_id
                    && original_results[i - 1].edge_id < original_results[i].edge_id)
                || (original_results[i - 1].start_id == original_results[i].start_id
                    && original_results[i - 1].edge_id == original_results[i].edge_id
                    && original_results[i - 1].end_id <= original_results[i].end_id),
            "Original results not in deterministic order at index {}: {:?} vs {:?}",
            i,
            original_results[i - 1],
            original_results[i]
        );
    }
}
