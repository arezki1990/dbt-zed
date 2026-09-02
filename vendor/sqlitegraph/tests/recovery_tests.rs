use serde_json::json;
use sqlitegraph::{
    add_label, add_property,
    graph::{GraphEdge, GraphEntity, SqliteGraph},
    recovery::{dump_graph_to_writer, load_graph_from_reader},
};

fn sample_graph() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let root = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Module".into(),
            name: "core".into(),
            file_path: Some("core.rs".into()),
            data: json!({"size": 10}),
        })
        .unwrap();
    let child = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "run".into(),
            file_path: Some("core.rs".into()),
            data: json!({"exports": true}),
        })
        .unwrap();
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: root,
            to_id: child,
            edge_type: "CONTAINS".into(),
            data: json!({}),
        })
        .unwrap();
    add_label(&graph, root, "Module").unwrap();
    add_property(&graph, child, "role", "entry").unwrap();
    graph
}

#[test]
fn dump_and_load_roundtrip_preserves_metadata() {
    let source = sample_graph();
    let mut buffer = Vec::new();
    dump_graph_to_writer(&source, &mut buffer).unwrap();

    let target = SqliteGraph::open_in_memory().unwrap();
    load_graph_from_reader(&target, &buffer[..]).unwrap();

    let source_ids = source.list_entity_ids().unwrap();
    let target_ids = target.list_entity_ids().unwrap();
    assert_eq!(source_ids.len(), target_ids.len());

    let labeled = sqlitegraph::index::get_entities_by_label(&target, "Module").unwrap();
    assert_eq!(labeled.len(), 1);
    assert_eq!(labeled[0].name, "core");

    let props = sqlitegraph::index::get_entities_by_property(&target, "role", "entry").unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, "run");
}

#[test]
fn load_overwrites_existing_data() {
    let source = sample_graph();
    let mut buffer = Vec::new();
    dump_graph_to_writer(&source, &mut buffer).unwrap();

    let target = SqliteGraph::open_in_memory().unwrap();
    // pre-populate with extra entries
    target
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "temp".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    load_graph_from_reader(&target, &buffer[..]).unwrap();

    let ids = target.list_entity_ids().unwrap();
    assert_eq!(ids.len(), 2);
    let props = sqlitegraph::index::get_entities_by_property(&target, "role", "entry").unwrap();
    assert_eq!(props.len(), 1);
}
