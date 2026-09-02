use serde_json::json;
use sqlitegraph::backend::{
    BackendDirection, CombinedGraphBackend, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec,
};
use sqlitegraph::snapshot::SnapshotId;

#[test]
fn test_combined_backend_in_memory_is_sqlite_authoritative_today() {
    let backend = CombinedGraphBackend::in_memory().expect("combined in-memory backend");

    let a = backend
        .insert_node(NodeSpec {
            kind: "User".into(),
            name: "Alice".into(),
            file_path: None,
            data: json!({"role": "admin"}),
        })
        .expect("insert Alice");
    let b = backend
        .insert_node(NodeSpec {
            kind: "User".into(),
            name: "Bob".into(),
            file_path: None,
            data: json!({"role": "reader"}),
        })
        .expect("insert Bob");

    backend
        .insert_edge(EdgeSpec {
            from: a,
            to: b,
            edge_type: "KNOWS".into(),
            data: json!({}),
        })
        .expect("insert edge");

    let neighbors = backend
        .neighbors(
            SnapshotId::current(),
            a,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("KNOWS".into()),
            },
        )
        .expect("neighbors");

    assert_eq!(neighbors, vec![b]);
    assert!(backend.get_graph_ref().is_some());
    assert_eq!(
        backend
            .query_nodes_by_kind(SnapshotId::current(), "User")
            .expect("query kind"),
        vec![a, b]
    );
}
