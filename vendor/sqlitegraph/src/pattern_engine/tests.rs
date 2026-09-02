//! Unit tests for pattern engine functionality.

use crate::{GraphEdge, GraphEntity, backend::BackendDirection};
use serde_json::json;

use super::matcher::match_triples;
use super::pattern::PatternTriple;

fn create_test_graph() -> crate::graph::SqliteGraph {
    crate::graph::SqliteGraph::open_in_memory().expect("Failed to create test graph")
}

fn insert_entity(graph: &crate::graph::SqliteGraph, kind: &str, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: kind.into(),
            name: name.into(),
            file_path: None,
            data: json!({"name": name}),
        })
        .expect("Failed to insert entity")
}

fn insert_edge(graph: &crate::graph::SqliteGraph, from: i64, to: i64, edge_type: &str) -> i64 {
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: from,
            to_id: to,
            edge_type: edge_type.into(),
            data: json!({}),
        })
        .expect("Failed to insert edge")
}

fn add_label_to_entity(graph: &crate::graph::SqliteGraph, entity_id: i64, label: &str) {
    crate::index::add_label(graph, entity_id, label).expect("Failed to add label");
}

fn add_property_to_entity(
    graph: &crate::graph::SqliteGraph,
    entity_id: i64,
    key: &str,
    value: &str,
) {
    crate::index::add_property(graph, entity_id, key, value).expect("Failed to add property");
}

#[test]
fn test_pattern_triple_builder() {
    let pattern = PatternTriple::new("CALLS")
        .start_label("Function")
        .end_label("Function")
        .start_property("language", "rust")
        .end_property("language", "rust")
        .direction(BackendDirection::Outgoing);

    assert_eq!(pattern.edge_type, "CALLS");
    assert_eq!(pattern.start_label, Some("Function".to_string()));
    assert_eq!(pattern.end_label, Some("Function".to_string()));
    assert_eq!(
        pattern.start_props.get("language"),
        Some(&"rust".to_string())
    );
    assert_eq!(pattern.end_props.get("language"), Some(&"rust".to_string()));
    assert_eq!(pattern.direction, BackendDirection::Outgoing);
}

#[test]
fn test_pattern_triple_validation() {
    let valid_pattern = PatternTriple::new("CALLS");
    assert!(valid_pattern.validate().is_ok());

    let invalid_pattern = PatternTriple::new("");
    assert!(invalid_pattern.validate().is_err());
}

#[test]
fn test_match_triples_basic() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let _edge_id = insert_edge(&graph, f1, f2, "CALLS");

    let pattern = PatternTriple::new("CALLS");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[0].end_id, f2);
}

#[test]
fn test_match_triples_with_labels() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let s1 = insert_entity(&graph, "Struct", "struct1");

    add_label_to_entity(&graph, f1, "Function");
    add_label_to_entity(&graph, f2, "Function");
    add_label_to_entity(&graph, s1, "Struct");

    let _edge1 = insert_edge(&graph, f1, f2, "CALLS");
    let _edge2 = insert_edge(&graph, f2, s1, "USES");

    let pattern = PatternTriple::new("CALLS")
        .start_label("Function")
        .end_label("Function");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[0].end_id, f2);
}

#[test]
fn test_match_triples_with_properties() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let f3 = insert_entity(&graph, "Function", "func3");

    add_property_to_entity(&graph, f1, "language", "rust");
    add_property_to_entity(&graph, f2, "language", "rust");
    add_property_to_entity(&graph, f3, "language", "python");

    let _edge1 = insert_edge(&graph, f1, f2, "CALLS");
    let _edge2 = insert_edge(&graph, f1, f3, "CALLS");

    let pattern = PatternTriple::new("CALLS")
        .start_property("language", "rust")
        .end_property("language", "rust");

    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[0].end_id, f2);
}

#[test]
fn test_match_triples_direction() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let edge_id = insert_edge(&graph, f1, f2, "CALLS");

    // Test outgoing direction
    let pattern_outgoing = PatternTriple::new("CALLS").direction(BackendDirection::Outgoing);

    let matches_outgoing =
        match_triples(&graph, &pattern_outgoing).expect("Failed to match triples");
    assert_eq!(matches_outgoing.len(), 1);
    assert_eq!(matches_outgoing[0].start_id, f1);
    assert_eq!(matches_outgoing[0].end_id, f2);

    // Test incoming direction
    let pattern_incoming = PatternTriple::new("CALLS").direction(BackendDirection::Incoming);

    let matches_incoming =
        match_triples(&graph, &pattern_incoming).expect("Failed to match triples");
    assert_eq!(matches_incoming.len(), 1);
    assert_eq!(matches_incoming[0].start_id, f2); // Start is now original target
    assert_eq!(matches_incoming[0].end_id, f1); // End is now original source
    assert_eq!(matches_incoming[0].edge_id, edge_id);
}
