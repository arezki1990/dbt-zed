use serde_json::json;
use sqlitegraph::{
    graph::{GraphEntity, SqliteGraph},
    index::{add_label, add_property, get_entities_by_label, get_entities_by_property},
};

fn graph() -> SqliteGraph {
    SqliteGraph::open_in_memory().expect("graph")
}

fn insert_node(graph: &SqliteGraph, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Node".into(),
            name: name.into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap()
}

#[test]
fn test_label_roundtrip() {
    let g = graph();
    let id = insert_node(&g, "A");
    add_label(&g, id, "Module").unwrap();
    let entities = get_entities_by_label(&g, "Module").unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].id, id);
}

#[test]
fn test_property_roundtrip() {
    let g = graph();
    let id = insert_node(&g, "A");
    add_property(&g, id, "role", "leaf").unwrap();
    let entities = get_entities_by_property(&g, "role", "leaf").unwrap();
    assert_eq!(entities[0].id, id);
}

#[test]
fn test_multi_label_entries() {
    let g = graph();
    let id = insert_node(&g, "A");
    let id2 = insert_node(&g, "B");
    add_label(&g, id, "Service").unwrap();
    add_label(&g, id2, "Service").unwrap();
    let entities = get_entities_by_label(&g, "Service").unwrap();
    assert_eq!(entities.len(), 2);
    assert!(entities[0].id < entities[1].id);
}

#[test]
fn test_ordering_consistency() {
    let g = graph();
    let id1 = insert_node(&g, "A");
    let id2 = insert_node(&g, "B");
    add_property(&g, id2, "tier", "1").unwrap();
    add_property(&g, id1, "tier", "1").unwrap();
    let entities = get_entities_by_property(&g, "tier", "1").unwrap();
    assert_eq!(
        entities.into_iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![id1, id2]
    );
}

#[test]
fn test_selective_property_match() {
    let g = graph();
    let id = insert_node(&g, "A");
    add_property(&g, id, "kind", "fn").unwrap();
    let empty = get_entities_by_property(&g, "kind", "type").unwrap();
    assert!(empty.is_empty());
}
