use serde_json::json;
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph, SqliteGraphError};

fn sample_entity(name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: "Node".to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({ "name": name }),
    }
}

fn sample_edge(from_id: i64, to_id: i64, edge_type: &str) -> GraphEdge {
    GraphEdge {
        id: 0,
        from_id,
        to_id,
        edge_type: edge_type.to_string(),
        data: json!({ "type": edge_type }),
    }
}

fn prepared_graph() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    for name in ["a", "b", "c", "d"] {
        graph.insert_entity(&sample_entity(name)).expect("entity");
    }
    graph
}

#[test]
fn test_insert_and_get_edge_roundtrip() {
    let graph = prepared_graph();
    let id = graph
        .insert_edge(&sample_edge(1, 2, "CALLS"))
        .expect("insert edge");
    let stored = graph.get_edge(id).expect("edge");
    assert_eq!(stored.from_id, 1);
    assert_eq!(stored.to_id, 2);
    assert_eq!(stored.edge_type, "CALLS");
}

#[test]
fn test_edges_from_indexed_by_from_id() {
    let graph = prepared_graph();
    for &(from, to) in &[(1, 2), (1, 3), (2, 3)] {
        graph
            .insert_edge(&sample_edge(from, to, "USES"))
            .expect("edge");
    }
    let query = graph.query();
    let outgoing = query.outgoing(1).expect("outgoing");
    assert_eq!(outgoing, vec![2, 3]);
}

#[test]
fn test_edges_to_indexed_by_to_id() {
    let graph = prepared_graph();
    for &(from, to) in &[(1, 2), (3, 2), (4, 2)] {
        graph
            .insert_edge(&sample_edge(from, to, "INCLUDES"))
            .expect("edge");
    }
    let query = graph.query();
    let incoming = query.incoming(2).expect("incoming");
    assert_eq!(incoming, vec![1, 3, 4]);
}

#[test]
fn test_delete_edge_removes_record() {
    let graph = prepared_graph();
    let id = graph
        .insert_edge(&sample_edge(1, 2, "DECLARES"))
        .expect("edge");
    graph.delete_edge(id).expect("delete");
    let err = graph.get_edge(id).expect_err("missing");
    assert!(matches!(err, SqliteGraphError::NotFound(_)));
}

#[test]
fn test_bulk_edges_deterministic_ordering() {
    let graph = prepared_graph();
    let ids: Vec<_> = vec![(1, 2), (1, 3), (2, 4), (3, 4)]
        .into_iter()
        .map(|(from, to)| graph.insert_edge(&sample_edge(from, to, "FLOW")).unwrap())
        .collect();
    assert_eq!(ids, vec![1, 2, 3, 4]);
}

#[test]
fn test_insert_edge_requires_type() {
    let graph = prepared_graph();
    let mut edge = sample_edge(1, 2, "");
    edge.edge_type.clear();
    let err = graph.insert_edge(&edge).expect_err("invalid");
    assert!(matches!(err, SqliteGraphError::InvalidInput(_)));
}

#[test]
fn test_insert_edge_allows_self_loops() {
    let graph = prepared_graph();
    let edge = sample_edge(1, 1, "CALLS");
    let id = graph.insert_edge(&edge).expect("self loop ok");
    let stored = graph.get_edge(id).expect("edge");
    assert_eq!(stored.from_id, 1);
    assert_eq!(stored.to_id, 1);
}

#[test]
fn test_get_edge_not_found_returns_error() {
    let graph = prepared_graph();
    let err = graph.get_edge(44).expect_err("missing");
    assert!(matches!(err, SqliteGraphError::NotFound(_)));
}
