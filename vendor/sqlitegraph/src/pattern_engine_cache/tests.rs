use crate::pattern_engine::match_triples;
use crate::pattern_engine_cache::fast_path_detection::can_use_fast_path;
use crate::pattern_engine_cache::fast_path_execution::match_triples_fast;
use crate::{GraphEdge, GraphEntity, PatternTriple, SqliteGraph};
use serde_json::json;

fn create_test_graph() -> SqliteGraph {
    SqliteGraph::open_in_memory().expect("Failed to create test graph")
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

#[test]
fn test_can_use_fast_path_detection() {
    // Should use fast path - edge type only
    let pattern1 = PatternTriple::new("CALLS");
    assert!(can_use_fast_path(&pattern1));

    // Should NOT use fast path - has start label
    let pattern2 = PatternTriple::new("CALLS").start_label("Function");
    assert!(!can_use_fast_path(&pattern2));

    // Should NOT use fast path - has property filter
    let pattern3 = PatternTriple::new("CALLS").start_property("lang", "rust");
    assert!(!can_use_fast_path(&pattern3));

    // Should NOT use fast path - has end label
    let pattern4 = PatternTriple::new("CALLS").end_label("Function");
    assert!(!can_use_fast_path(&pattern4));
}

#[test]
fn test_fast_path_basic_functionality() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let _edge_id = insert_edge(&graph, f1, f2, "CALLS");

    let pattern = PatternTriple::new("CALLS");
    let matches = match_triples_fast(&graph, &pattern).expect("Fast path failed");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start_id, f1);
    assert_eq!(matches[0].end_id, f2);
}

#[test]
fn test_fast_path_vs_sql_equality() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let f3 = insert_entity(&graph, "Function", "func3");

    insert_edge(&graph, f1, f2, "CALLS");
    insert_edge(&graph, f1, f3, "CALLS");
    insert_edge(&graph, f2, f3, "USES");

    let pattern = PatternTriple::new("CALLS");

    let sql_results = match_triples(&graph, &pattern).expect("SQL failed");
    let fast_results = match_triples_fast(&graph, &pattern).expect("Fast path failed");

    // Results must be identical
    assert_eq!(sql_results.len(), fast_results.len());
    assert_eq!(sql_results, fast_results);
}

#[test]
fn test_fast_path_deterministic_ordering() {
    let graph = create_test_graph();

    let f1 = insert_entity(&graph, "Function", "func1");
    let f2 = insert_entity(&graph, "Function", "func2");
    let f3 = insert_entity(&graph, "Function", "func3");

    let edge1 = insert_edge(&graph, f1, f2, "CALLS");
    let edge2 = insert_edge(&graph, f1, f3, "CALLS");
    let edge3 = insert_edge(&graph, f2, f3, "CALLS");

    let pattern = PatternTriple::new("CALLS");
    let matches = match_triples_fast(&graph, &pattern).expect("Fast path failed");

    assert_eq!(matches.len(), 3);

    // Verify deterministic ordering: start_id ASC, edge_id ASC, end_id ASC
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

    // Should be ordered by edge IDs we created
    let expected_order = [edge1, edge2, edge3];
    for (i, &expected_edge_id) in expected_order.iter().enumerate() {
        assert_eq!(matches[i].edge_id, expected_edge_id);
    }
}
