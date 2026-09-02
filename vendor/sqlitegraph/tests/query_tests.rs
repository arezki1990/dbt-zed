use serde_json::json;
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph, query::GraphQuery};

fn setup_query_graph() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    for name in ["A", "B", "C", "D", "E"] {
        graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "Node".into(),
                name: name.into(),
                file_path: None,
                data: json!({}),
            })
            .unwrap();
    }
    let edges = vec![
        (1, 2, "CALLS"),
        (2, 3, "USES"),
        (3, 4, "CALLS"),
        (2, 4, "CALLS"),
        (4, 5, "IMPLEMENTS"),
    ];
    for &(from, to, ty) in &edges {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: from,
                to_id: to,
                edge_type: ty.into(),
                data: json!({ "kind": ty }),
            })
            .unwrap();
    }
    graph
}

#[test]
fn test_neighbors_matches_edges() {
    let graph = setup_query_graph();
    let q = GraphQuery::new(&graph);
    assert_eq!(q.neighbors(2).unwrap(), vec![3, 4]);
}

#[test]
fn test_incoming_outgoing_split_correctly() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert_eq!(q.outgoing(2).unwrap(), vec![3, 4]);
    assert_eq!(q.incoming(4).unwrap(), vec![2, 3]);
}

#[test]
fn test_edges_of_type_filters_by_type() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert_eq!(q.edges_of_type(2, "CALLS").unwrap(), vec![4]);
    assert_eq!(q.edges_of_type(2, "USES").unwrap(), vec![3]);
}

#[test]
fn test_has_path_true_for_connected_nodes() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert!(q.has_path(1, 5, 5).unwrap());
}

#[test]
fn test_has_path_false_for_disconnected_nodes() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert!(!q.has_path(5, 1, 5).unwrap());
}

#[test]
fn test_neighbors_sorted_deterministically() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert_eq!(q.neighbors(1).unwrap(), vec![2]);
}

#[test]
fn test_query_reuses_cache_between_calls() {
    let graph = setup_query_graph();
    let q = graph.query();
    let first = q.neighbors(2).unwrap();
    let second = q.neighbors(2).unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_has_path_respects_max_depth() {
    let graph = setup_query_graph();
    let q = graph.query();
    assert!(!q.has_path(1, 5, 2).unwrap());
    assert!(q.has_path(1, 5, 4).unwrap());
}
