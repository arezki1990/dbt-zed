use rusqlite::Connection;
use serde_json::json;
use sqlitegraph::{GraphEntity, SqliteGraph, SqliteGraphError, schema::ensure_schema};

fn sample_entity(kind: &str, name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({ "kind": kind, "name": name }),
    }
}

#[test]
fn test_create_schema_creates_tables() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    ensure_schema(&conn).expect("schema");

    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('graph_entities', 'graph_edges')",
        )
        .expect("prepare");
    let mut rows = stmt.query([]).expect("query");

    let mut found = Vec::new();
    while let Some(row) = rows.next().expect("rows") {
        found.push(row.get::<_, String>(0).expect("name"));
    }

    assert!(found.contains(&"graph_entities".to_string()));
    assert!(found.contains(&"graph_edges".to_string()));
}

#[test]
fn test_insert_and_get_entity_roundtrip() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let entity = sample_entity("Function", "alpha");
    let id = graph.insert_entity(&entity).expect("insert");
    let stored = graph.get_entity(id).expect("get");
    assert_eq!(stored.kind, "Function");
    assert_eq!(stored.name, "alpha");
    assert_eq!(stored.file_path, None);
}

#[test]
fn test_entity_update_persists_changes() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let mut entity = sample_entity("Struct", "Beta");
    let id = graph.insert_entity(&entity).expect("insert");
    entity.id = id;
    entity.name = "BetaRenamed".to_string();
    entity.file_path = Some("src/lib.rs".to_string());
    graph.update_entity(&entity).expect("update");
    let stored = graph.get_entity(id).expect("get");
    assert_eq!(stored.name, "BetaRenamed");
    assert_eq!(stored.file_path.as_deref(), Some("src/lib.rs"));
}

#[test]
fn test_entity_delete_removes_record() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let id = graph
        .insert_entity(&sample_entity("Mod", "gamma"))
        .expect("insert");
    graph.delete_entity(id).expect("delete");
    let err = graph.get_entity(id).expect_err("missing");
    match err {
        SqliteGraphError::NotFound(_) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn test_bulk_insert_entities_is_deterministic() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let ids: Vec<_> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|name| {
            graph
                .insert_entity(&sample_entity("Node", name))
                .expect("insert")
        })
        .collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
}

#[test]
fn test_insert_entity_requires_name() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let mut entity = sample_entity("Node", "");
    entity.name.clear();
    let err = graph.insert_entity(&entity).expect_err("invalid");
    match err {
        SqliteGraphError::InvalidInput(_) => {}
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn test_insert_entity_assigns_incrementing_ids() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let id1 = graph.insert_entity(&sample_entity("Node", "one")).unwrap();
    let id2 = graph.insert_entity(&sample_entity("Node", "two")).unwrap();
    let id3 = graph
        .insert_entity(&sample_entity("Node", "three"))
        .unwrap();
    assert!(id1 < id2 && id2 < id3);
    assert_eq!(id1, 1);
    assert_eq!(id3, 3);
}

#[test]
fn test_get_entity_not_found_returns_error() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let err = graph.get_entity(999).expect_err("missing");
    assert!(matches!(err, SqliteGraphError::NotFound(_)));
}
