use serde_json::json;
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph, cache::AdjacencyCache};

#[test]
fn test_adjacency_cache_get_insert_clear() {
    let cache = AdjacencyCache::new();
    assert!(cache.get(1).is_none());
    cache.insert(1, vec![2, 3]);
    assert_eq!(cache.get(1).map(|a| a.to_vec()), Some(vec![2, 3]));
    cache.clear();
    assert!(cache.get(1).is_none());
}

fn insert_entity(graph: &SqliteGraph, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Node".into(),
            name: name.into(),
            file_path: None,
            data: json!({ "name": name }),
        })
        .expect("entity")
}

fn insert_edge(graph: &SqliteGraph, from: i64, to: i64) -> i64 {
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: from,
            to_id: to,
            edge_type: "LINK".into(),
            data: json!({}),
        })
        .expect("edge")
}

#[test]
fn test_cache_invalidated_when_edges_change() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");

    insert_edge(&graph, a, b);
    let first = graph.query().outgoing(a).expect("outgoing");
    assert_eq!(first, vec![b]);

    let edge_bc = insert_edge(&graph, a, c);
    let second = graph.query().outgoing(a).expect("outgoing");
    assert_eq!(second, vec![b, c]);

    graph.delete_edge(edge_bc).expect("delete edge");
    let third = graph.query().outgoing(a).expect("outgoing");
    assert_eq!(third, vec![b]);
}
