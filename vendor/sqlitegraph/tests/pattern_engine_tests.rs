//! Tests for the lightweight pattern engine.
//!
//! This test suite validates the deterministic triple pattern matching functionality
//! using TDD approach to ensure correctness and performance.

use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, PatternTriple, SqliteGraph,
    backend::BackendDirection,
    index::{add_label, add_property},
    match_triples,
};

/// Create a test graph with sample data for pattern matching tests
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

    // Insert modules
    let m1 = insert_entity(&graph, "Module", "core");
    let m2 = insert_entity(&graph, "Module", "utils");

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
    insert_edge(&graph, f1, m1, "BELONGS_TO");
    insert_edge(&graph, f2, m1, "BELONGS_TO");
    insert_edge(&graph, f3, m2, "BELONGS_TO");
    insert_edge(&graph, f4, m2, "BELONGS_TO");

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
    // Use query API to find entity by name
    let entity_ids = graph.list_entity_ids().expect("Failed to get entity IDs");
    for id in entity_ids {
        let entity = graph.get_entity(id).expect("Failed to get entity");
        if entity.name == name {
            return id;
        }
    }
    panic!("Entity with name '{}' not found", name);
}

#[test]
fn test_pattern_triple_basic_functionality() {
    let graph = create_test_graph();

    // Test basic edge type matching
    let pattern = PatternTriple::new("CALLS");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    // Should find all CALLS edges
    assert_eq!(matches.len(), 3); // f1->f2, f1->f3, f1->f4

    // Verify deterministic ordering
    let f1 = get_entity_by_name(&graph, "process_data");
    let f2 = get_entity_by_name(&graph, "validate_input");
    let f3 = get_entity_by_name(&graph, "handle_error");
    let f4 = get_entity_by_name(&graph, "log_result");

    // Should be ordered by start_id, then edge_id, then end_id
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[0].end_id, f2);
    assert_eq!(matches[1].start_id, f1);
    assert_eq!(matches[1].end_id, f3);
    assert_eq!(matches[2].start_id, f1);
    assert_eq!(matches[2].end_id, f4);
}

#[test]
fn test_pattern_triple_with_label_filters() {
    let graph = create_test_graph();

    // Test with start and end label filters
    let pattern = PatternTriple::new("USES")
        .start_label("private")
        .end_label("internal");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    // Should find only f3->s2 (private function uses internal struct)
    assert_eq!(matches.len(), 1);

    let f3 = get_entity_by_name(&graph, "handle_error");
    let s2 = get_entity_by_name(&graph, "ErrorHandler");

    assert_eq!(matches[0].start_id, f3);
    assert_eq!(matches[0].end_id, s2);
}

#[test]
fn test_pattern_triple_with_property_filters() {
    let graph = create_test_graph();

    // Test with property filters
    let pattern = PatternTriple::new("CALLS")
        .start_property("async", "true")
        .end_property("async", "false");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    // Should find async functions calling non-async functions
    // f1 (async=true) -> f2 (async=false) and f1 (async=true) -> f3 (async=false)
    assert_eq!(matches.len(), 2);

    let f1 = get_entity_by_name(&graph, "process_data");
    let f2 = get_entity_by_name(&graph, "validate_input");
    let f3 = get_entity_by_name(&graph, "handle_error");

    // Both matches should start from f1
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[1].start_id, f1);

    // Ends should be f2 and f3 in deterministic order
    let mut end_ids = vec![matches[0].end_id, matches[1].end_id];
    end_ids.sort();
    assert_eq!(end_ids, vec![f2, f3]);
}

#[test]
fn test_pattern_triple_combined_filters() {
    let graph = create_test_graph();

    // Test with combined label and property filters
    let pattern = PatternTriple::new("USES")
        .start_label("public")
        .start_property("async", "true")
        .end_label("exported")
        .end_property("thread_safe", "true");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    // Should find f4 (public, async=true) -> s1 (exported, thread_safe=true)
    assert_eq!(matches.len(), 1);

    let f4 = get_entity_by_name(&graph, "log_result");
    let s1 = get_entity_by_name(&graph, "DataProcessor");

    assert_eq!(matches[0].start_id, f4);
    assert_eq!(matches[0].end_id, s1);
}

#[test]
fn test_pattern_triple_direction() {
    let graph = create_test_graph();

    let f1 = get_entity_by_name(&graph, "process_data");
    let f2 = get_entity_by_name(&graph, "validate_input");

    // Test outgoing direction
    let pattern_outgoing = PatternTriple::new("CALLS").direction(BackendDirection::Outgoing);

    let matches_outgoing =
        match_triples(&graph, &pattern_outgoing).expect("Failed to match triples");

    // Should find f1->f2 in outgoing direction
    let f1_to_f2_match = matches_outgoing
        .iter()
        .find(|m| m.start_id == f1 && m.end_id == f2);
    assert!(f1_to_f2_match.is_some());

    // Test incoming direction
    let pattern_incoming = PatternTriple::new("CALLS").direction(BackendDirection::Incoming);

    let matches_incoming =
        match_triples(&graph, &pattern_incoming).expect("Failed to match triples");

    // Should find f2->f1 in incoming direction (reversed)
    let f2_to_f1_match = matches_incoming
        .iter()
        .find(|m| m.start_id == f2 && m.end_id == f1);
    assert!(f2_to_f1_match.is_some());
}

#[test]
fn test_pattern_triple_no_matches() {
    let graph = create_test_graph();

    // Test with non-existent edge type
    let pattern = PatternTriple::new("NONEXISTENT");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");
    assert_eq!(matches.len(), 0);

    // Test with non-existent label
    let pattern = PatternTriple::new("CALLS").start_label("NONEXISTENT_LABEL");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");
    assert_eq!(matches.len(), 0);

    // Test with non-existent property
    let pattern = PatternTriple::new("CALLS").start_property("nonexistent", "value");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");
    assert_eq!(matches.len(), 0);
}

#[test]
fn test_pattern_triple_validation() {
    let graph = create_test_graph();

    // Test empty edge type validation
    let pattern = PatternTriple::new("");
    let result = match_triples(&graph, &pattern);
    assert!(result.is_err());

    // Test whitespace-only edge type validation
    let pattern = PatternTriple::new("   ");
    let result = match_triples(&graph, &pattern);
    assert!(result.is_err());
}

#[test]
fn test_pattern_triple_deterministic_ordering() {
    let graph = create_test_graph();

    // Create a pattern that matches multiple edges
    let pattern = PatternTriple::new("BELONGS_TO");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    assert_eq!(matches.len(), 4);

    // Verify deterministic ordering: sorted by start_id, then edge_id, then end_id
    for i in 1..matches.len() {
        assert!(
            matches[i - 1].start_id < matches[i].start_id
                || (matches[i - 1].start_id == matches[i].start_id
                    && matches[i - 1].edge_id < matches[i].edge_id)
                || (matches[i - 1].start_id == matches[i].start_id
                    && matches[i - 1].edge_id == matches[i].edge_id
                    && matches[i - 1].end_id <= matches[i].end_id),
            "Matches not in deterministic order at index {}: {:?} vs {:?}",
            i,
            matches[i - 1],
            matches[i]
        );
    }
}

#[test]
fn test_pattern_triple_sqlitegraph_integration() {
    let graph = create_test_graph();

    // Test the SqliteGraph.match_triples() method
    let pattern = PatternTriple::new("CALLS");
    let matches = graph
        .match_triples(&pattern)
        .expect("Failed to match triples");

    // Should find all CALLS edges
    assert_eq!(matches.len(), 3);

    // Verify the matches are TripleMatch structs
    for triple_match in &matches {
        assert!(triple_match.start_id > 0);
        assert!(triple_match.end_id > 0);
        assert!(triple_match.edge_id > 0);
    }
}

#[test]
fn test_pattern_triple_performance_with_large_dataset() {
    let graph = SqliteGraph::open_in_memory().expect("Failed to create test graph");

    // Create a larger dataset for performance testing
    let mut entity_ids = Vec::new();
    for i in 0..100 {
        let id = insert_entity(&graph, "Node", &format!("node_{}", i));
        entity_ids.push(id);

        // Add some labels and properties
        if i % 2 == 0 {
            add_label_to_entity(&graph, id, "even");
        } else {
            add_label_to_entity(&graph, id, "odd");
        }

        add_property_to_entity(&graph, id, "index", &i.to_string());
        add_property_to_entity(
            &graph,
            id,
            "parity",
            if i % 2 == 0 { "even" } else { "odd" },
        );
    }

    // Create edges between consecutive nodes
    for i in 0..99 {
        insert_edge(&graph, entity_ids[i], entity_ids[i + 1], "NEXT");
    }

    // Test pattern matching performance
    let start = std::time::Instant::now();

    let pattern = PatternTriple::new("NEXT")
        .start_property("parity", "even")
        .end_property("parity", "odd");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    let duration = start.elapsed();

    // Should find all even->odd transitions
    assert_eq!(matches.len(), 50); // 0->1, 2->3, ..., 96->97, 98->99

    // Performance should be reasonable (less than 1 second for this dataset)
    assert!(
        duration.as_secs() < 1,
        "Pattern matching took too long: {:?}",
        duration
    );

    // Verify deterministic ordering
    for i in 1..matches.len() {
        assert!(
            matches[i - 1].start_id < matches[i].start_id,
            "Matches not ordered by start_id"
        );
    }
}

#[test]
fn test_pattern_triple_complex_property_combinations() {
    let graph = create_test_graph();

    // Test multiple property filters on the same node
    let pattern = PatternTriple::new("CALLS")
        .start_property("language", "rust")
        .start_property("async", "true")
        .end_property("language", "rust")
        .end_property("async", "false");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    // Should find f1 (rust, async=true) -> f2 and f3 (rust, async=false)
    assert_eq!(matches.len(), 2);

    let f1 = get_entity_by_name(&graph, "process_data");
    let f2 = get_entity_by_name(&graph, "validate_input");
    let f3 = get_entity_by_name(&graph, "handle_error");

    // Both matches should start from f1
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[1].start_id, f1);

    // Ends should be f2 and f3
    let mut end_ids = vec![matches[0].end_id, matches[1].end_id];
    end_ids.sort();
    assert_eq!(end_ids, vec![f2, f3]);
}
