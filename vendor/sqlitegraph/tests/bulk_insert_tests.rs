//! Tests for bulk insert primitives on `SqliteGraph` and `GraphBackend`.

use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, SqliteGraph, SqliteGraphBackend,
    backend::{EdgeSpec, GraphBackend, NodeSpec},
};

fn entity(kind: &str, name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({}),
    }
}

fn edge(from: i64, to: i64, kind: &str) -> GraphEdge {
    GraphEdge {
        id: 0,
        from_id: from,
        to_id: to,
        edge_type: kind.to_string(),
        data: json!({}),
    }
}

fn node_spec(kind: &str, name: &str) -> NodeSpec {
    NodeSpec {
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({}),
    }
}

fn edge_spec(from: i64, to: i64, kind: &str) -> EdgeSpec {
    EdgeSpec {
        from,
        to,
        edge_type: kind.to_string(),
        data: json!({}),
    }
}

#[test]
fn insert_entities_bulk_returns_ids_in_input_order() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let entities = vec![
        entity("Function", "a"),
        entity("Function", "b"),
        entity("Function", "c"),
    ];
    let ids = graph
        .insert_entities_bulk(&entities)
        .expect("bulk insert entities");
    assert_eq!(ids.len(), 3);
    assert!(ids[0] < ids[1]);
    assert!(ids[1] < ids[2]);
    // Verify roundtrip
    let stored = graph.get_entity(ids[1]).expect("get");
    assert_eq!(stored.name, "b");
}

#[test]
fn insert_entities_bulk_empty_input_returns_empty_vec() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let ids = graph.insert_entities_bulk(&[]).expect("bulk empty");
    assert!(ids.is_empty());
}

#[test]
fn insert_entities_bulk_rolls_back_on_error() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    // Insert one valid entity to anchor a baseline count.
    graph
        .insert_entity(&entity("Function", "baseline"))
        .unwrap();

    // Build a batch where the second entity has an invalid (empty) name.
    let entities = vec![
        entity("Function", "valid_one"),
        entity("Function", ""), // validate_entity rejects empty name
        entity("Function", "valid_two"),
    ];
    let result = graph.insert_entities_bulk(&entities);
    assert!(result.is_err(), "expected error for invalid entity");

    // Count must remain 1 — the partial inserts in this batch were rolled back.
    let ids = graph.list_entity_ids().unwrap();
    assert_eq!(ids.len(), 1, "expected rollback to undo partial inserts");
}

#[test]
fn insert_edges_bulk_returns_ids_in_input_order() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let a = graph.insert_entity(&entity("Node", "a")).unwrap();
    let b = graph.insert_entity(&entity("Node", "b")).unwrap();
    let c = graph.insert_entity(&entity("Node", "c")).unwrap();

    let edges = vec![
        edge(a, b, "CALL"),
        edge(b, c, "CALL"),
        edge(a, c, "IMPORTS"),
    ];
    let ids = graph.insert_edges_bulk(&edges).expect("bulk insert edges");
    assert_eq!(ids.len(), 3);
    assert!(ids[0] < ids[1]);
    assert!(ids[1] < ids[2]);
}

#[test]
fn insert_edges_bulk_empty_input_returns_empty_vec() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let ids = graph.insert_edges_bulk(&[]).expect("bulk empty");
    assert!(ids.is_empty());
}

#[test]
fn graph_backend_insert_nodes_bulk_via_sqlite_backend() {
    let backend = SqliteGraphBackend::in_memory().expect("backend");
    let specs = vec![
        node_spec("Function", "alpha"),
        node_spec("Function", "beta"),
        node_spec("Function", "gamma"),
    ];
    let ids = backend
        .insert_nodes_bulk(&specs)
        .expect("bulk insert nodes");
    assert_eq!(ids.len(), 3);
    assert!(ids[0] < ids[1]);
}

#[test]
fn graph_backend_insert_edges_bulk_via_sqlite_backend() {
    let backend = SqliteGraphBackend::in_memory().expect("backend");
    let node_specs = vec![node_spec("Node", "a"), node_spec("Node", "b")];
    let ids = backend.insert_nodes_bulk(&node_specs).expect("nodes");
    let edge_specs = vec![edge_spec(ids[0], ids[1], "LINK")];
    let edge_ids = backend
        .insert_edges_bulk(&edge_specs)
        .expect("bulk insert edges");
    assert_eq!(edge_ids.len(), 1);
}

#[test]
fn bulk_insert_matches_single_insert_observable_state() {
    let single = SqliteGraph::open_in_memory().expect("single");
    let bulk = SqliteGraph::open_in_memory().expect("bulk");

    let entities = vec![
        entity("Function", "a"),
        entity("Function", "b"),
        entity("Function", "c"),
    ];
    for e in &entities {
        single.insert_entity(e).unwrap();
    }
    let bulk_ids = bulk.insert_entities_bulk(&entities).unwrap();

    // Same observable state: same names, same kinds, same file_paths,
    // same id ordering, same count.
    assert_eq!(bulk_ids.len(), 3);
    for (i, id) in bulk_ids.iter().enumerate() {
        let stored = bulk.get_entity(*id).unwrap();
        assert_eq!(stored.name, entities[i].name);
        assert_eq!(stored.kind, entities[i].kind);
    }
    let bulk_count = bulk.list_entity_ids().unwrap().len();
    let single_count = single.list_entity_ids().unwrap().len();
    assert_eq!(bulk_count, single_count);
}
