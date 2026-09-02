//! Phase 44.2 — Cluster Size Contract Tests
//!
//! Micro regression test to verify cluster size contract is correct

use sqlitegraph::{
    BackendDirection, EdgeSpec, NeighborQuery, NodeSpec, SnapshotId, SqliteGraphError,
    config::GraphConfig, open_graph,
};
use tempfile::TempDir;

#[test]
fn test_cluster_size_contract_explicit() -> Result<(), SqliteGraphError> {
    // 1) Create graph with 2 nodes
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config).expect("Failed to create test graph");

    // Insert 2 nodes
    let node1_spec = NodeSpec {
        kind: "Function".to_string(),
        name: "func1".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    };
    let node1_id = graph.insert_node(node1_spec)?;

    let node2_spec = NodeSpec {
        kind: "Function".to_string(),
        name: "func2".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    };
    let node2_id = graph.insert_node(node2_spec)?;

    // 2) Insert 3 edges 1->2 with known payloads
    for i in 1..=3 {
        let edge_spec = EdgeSpec {
            from: node1_id,
            to: node2_id,
            edge_type: format!("CALL_{}", i),
            data: serde_json::json!({"seq": i}),
        };

        let edge_id = graph.insert_edge(edge_spec)?;
        assert!(edge_id > 0, "Edge {} should be inserted successfully", i);
    }

    // 3) Verify neighbors count
    let neighbors = graph.neighbors(
        SnapshotId::current(),
        node1_id,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    )?;

    assert_eq!(
        neighbors.len(),
        1,
        "Should have exactly 1 neighbor (node2), not {} neighbors",
        neighbors.len()
    );
    assert_eq!(neighbors[0], node2_id, "Neighbor should be node2");

    println!(
        "SUCCESS: Phase 44.2 contract test passed - found {} neighbors",
        neighbors.len()
    );

    Ok(())
}
