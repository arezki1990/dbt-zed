//! Smoke tests for sqlitegraph as an embedded library.
//!
//! These tests treat sqlitegraph as if it were used by SynCore,
//! ensuring public API surface is clean and functional.

use sqlitegraph::{
    GraphEdge, GraphEntity, GraphQuery, PatternTriple, SqliteGraph, SqliteGraphError,
    match_triples, match_triples_fast,
};
use tempfile::tempdir;

#[test]
fn test_can_construct_graph_from_path() {
    // Test that we can create/open a database file using public API
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");

    // Create graph using public API
    let graph = SqliteGraph::open(&db_path).expect("Failed to create graph");

    // Insert entities using public API
    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({"type": "test"}),
        })
        .expect("Failed to insert entity1");

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "node2".to_string(),
            file_path: None,
            data: serde_json::json!({"type": "test"}),
        })
        .expect("Failed to insert entity2");

    // Insert edge using public API
    let edge_id = graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entity1,
            to_id: entity2,
            edge_type: "TEST_EDGE".to_string(),
            data: serde_json::json!({"relationship": "test"}),
        })
        .expect("Failed to insert edge");

    // Query using public API
    let query = GraphQuery::new(&graph);
    let neighbors = query.neighbors(entity1).expect("Failed to get neighbors");
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], entity2);

    // Verify edge exists
    let retrieved_edge = graph.get_edge(edge_id).expect("Failed to get edge");
    assert_eq!(retrieved_edge.from_id, entity1);
    assert_eq!(retrieved_edge.to_id, entity2);
    assert_eq!(retrieved_edge.edge_type, "TEST_EDGE");
}

#[test]
fn test_pattern_triple_basic_through_lib_api() {
    // Test that PatternTriple and match_triples work through public API
    let graph = SqliteGraph::open_in_memory().expect("Failed to create in-memory graph");

    // Insert test data
    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Function".to_string(),
            name: "func1".to_string(),
            file_path: None,
            data: serde_json::json!({"lang": "rust"}),
        })
        .expect("Failed to insert entity1");

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Function".to_string(),
            name: "func2".to_string(),
            file_path: None,
            data: serde_json::json!({"lang": "rust"}),
        })
        .expect("Failed to insert entity2");

    let _edge_id = graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entity1,
            to_id: entity2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    // Test pattern matching using public API
    let pattern = PatternTriple::new("CALLS");
    let matches = match_triples(&graph, &pattern).expect("Failed to match triples");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start_id, entity1);
    assert_eq!(matches[0].end_id, entity2);

    // Test fast-path pattern matching using public API
    let fast_matches = match_triples_fast(&graph, &pattern).expect("Failed to match triples fast");

    assert_eq!(fast_matches.len(), 1);
    assert_eq!(fast_matches[0].start_id, entity1);
    assert_eq!(fast_matches[0].end_id, entity2);

    // Fast-path and regular should return identical results
    assert_eq!(matches, fast_matches);
}

#[test]
fn test_snapshot_and_wal_through_lib_api() {
    // Test snapshot functionality through public API
    let graph = SqliteGraph::open_in_memory().expect("Failed to create in-memory graph");

    // Insert test data
    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert entity1");

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "node2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert entity2");

    let _edge_id = graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entity1,
            to_id: entity2,
            edge_type: "TEST_REL".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    // Warm up cache first (needed for snapshot to see data)
    let query = GraphQuery::new(&graph);
    let _warmup1 = query.neighbors(entity1);
    let _warmup2 = query.neighbors(entity2);

    // Test snapshot acquisition through public API
    let snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");

    // Verify snapshot contains expected data
    assert!(snapshot.node_count() > 0);
    assert!(snapshot.edge_count() > 0);
    assert!(snapshot.contains_node(entity1));
    assert!(snapshot.contains_node(entity2));

    // Test that we can query snapshot state
    let outgoing = snapshot.get_outgoing(entity1);
    assert!(outgoing.is_some());
    assert!(outgoing.unwrap().contains(&entity2));
}

#[test]
fn test_error_types_through_lib_api() {
    // Test that SqliteGraphError is properly exported and usable
    let graph = SqliteGraph::open_in_memory().expect("Failed to create in-memory graph");

    // Test not found error
    let result = graph.get_entity(999);
    assert!(result.is_err());
    match result.unwrap_err() {
        SqliteGraphError::NotFound(_) => {} // Expected
        other => panic!("Expected NotFound error, got: {:?}", other),
    }

    // Test invalid input error
    let invalid_entity = GraphEntity {
        id: 0,
        kind: "".to_string(), // Empty kind should be invalid
        name: "test".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    };
    let result = graph.insert_entity(&invalid_entity);
    assert!(result.is_err());
    match result.unwrap_err() {
        SqliteGraphError::InvalidInput(_) => {} // Expected
        other => panic!("Expected InvalidInput error, got: {:?}", other),
    }
}
