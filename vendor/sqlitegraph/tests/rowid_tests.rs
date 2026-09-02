use serde_json::json;
use sqlitegraph::GraphEdge;
use sqlitegraph::{GraphEntity, SqliteGraph};

fn sample_entity(name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: "Node".into(),
        name: name.into(),
        file_path: None,
        data: json!({ "name": name }),
    }
}

fn sample_edge(from: i64, to: i64) -> GraphEdge {
    GraphEdge {
        id: 0,
        from_id: from,
        to_id: to,
        edge_type: "LINK".into(),
        data: json!({}),
    }
}

#[test]
fn test_entity_rowids_are_monotonic_even_after_deletes() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let first = graph.insert_entity(&sample_entity("A")).unwrap();
    let second = graph.insert_entity(&sample_entity("B")).unwrap();
    assert!(first < second);
    graph.delete_entity(second).unwrap();
    let third = graph.insert_entity(&sample_entity("C")).unwrap();
    assert!(third > second);
}

#[test]
fn test_edge_rowids_are_monotonic_even_after_deletes() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = graph.insert_entity(&sample_entity("A")).unwrap();
    let b = graph.insert_entity(&sample_entity("B")).unwrap();
    let first = graph.insert_edge(&sample_edge(a, b)).unwrap();
    let second = graph.insert_edge(&sample_edge(b, a)).unwrap();
    assert!(first < second);
    graph.delete_edge(second).unwrap();
    let third = graph.insert_edge(&sample_edge(a, b)).unwrap();
    assert!(third > second);
}
