use sqlitegraph::algo::async_traversal::{bfs_async, k_hop_async};
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{BackendDirection, EdgeSpec, GraphBackend, NodeSpec};
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

#[tokio::test]
async fn test_async_bfs_and_k_hop() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("async_traversal.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert test graph nodes: 1 -> 2 -> 3
    let _n1 = backend
        .insert_node_with_id(
            NodeSpec {
                kind: "Node".to_string(),
                name: "A".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            },
            1,
        )
        .unwrap();

    let _n2 = backend
        .insert_node_with_id(
            NodeSpec {
                kind: "Node".to_string(),
                name: "B".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            },
            2,
        )
        .unwrap();

    let _n3 = backend
        .insert_node_with_id(
            NodeSpec {
                kind: "Node".to_string(),
                name: "C".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            },
            3,
        )
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: 1,
            to: 2,
            edge_type: "Edge".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: 2,
            to: 3,
            edge_type: "Edge".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Flush data to index and disk persistent pages
    backend.flush_to_disk().unwrap();

    let snapshot_id = SnapshotId::current();

    // Run async BFS
    let visited_bfs = bfs_async(&backend, snapshot_id, 1, 2).await.unwrap();
    assert_eq!(visited_bfs, vec![1, 2, 3]);

    // Run async K-Hop
    let visited_khop = k_hop_async(&backend, snapshot_id, 1, 2, BackendDirection::Outgoing)
        .await
        .unwrap();
    assert_eq!(visited_khop, vec![1, 2, 3]);
}
