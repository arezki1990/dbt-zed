//! TDD: Edge durability across reopen
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

#[test]
fn test_edge_type_survives_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    {
        let backend = V3Backend::create(&db_path).unwrap();
        let center = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let helper = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "helper".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let util = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "util".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let other = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "other".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // 4 edges to reproduce and verify issue 2
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: util,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: helper,
                to: util,
                edge_type: "DEPENDS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: util,
                to: other,
                edge_type: "LINKS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend.flush().unwrap();
    }
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let current = SnapshotId::current();
        let all_ids = backend.entity_ids().unwrap();
        let center = all_ids
            .iter()
            .find(|&&id| backend.get_node(current, id).unwrap().name == "center")
            .copied()
            .unwrap();
        let helper = all_ids
            .iter()
            .find(|&&id| backend.get_node(current, id).unwrap().name == "helper")
            .copied()
            .unwrap();
        let util = all_ids
            .iter()
            .find(|&&id| backend.get_node(current, id).unwrap().name == "util")
            .copied()
            .unwrap();

        let all = backend
            .neighbors(
                current,
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();
        assert_eq!(all.len(), 2, "Should have 2 neighbors after reopen");
        let calls = backend
            .neighbors(
                current,
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("CALLS".to_string()),
                },
            )
            .unwrap();
        assert_eq!(calls.len(), 1, "Should have 1 CALLS neighbor after reopen");

        let depends = backend
            .neighbors(
                current,
                helper,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("DEPENDS".to_string()),
                },
            )
            .unwrap();
        assert_eq!(
            depends.len(),
            1,
            "Should have 1 DEPENDS neighbor after reopen"
        );

        let links = backend
            .neighbors(
                current,
                util,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("LINKS".to_string()),
                },
            )
            .unwrap();
        assert_eq!(links.len(), 1, "Should have 1 LINKS neighbor after reopen");
    }
}
