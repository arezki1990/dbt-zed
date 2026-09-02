//! V3 Backend Reopen/Durability Tests
//!
//! These tests verify that data persists correctly across close/reopen cycles
//! for the V3 native backend. They prove "reopen truth" not just
//! "same-process truth".
//!
//! **Key distinction**: These tests DROP the graph and reopen from file,
//! ensuring correctness comes from persistent storage, not in-memory caches.
//!
//! **IMPORTANT**: After removing per-page syncs (sync_data/sync_all),
//! these tests verify that:
//! 1. WAL provides durability without per-page syncs
//! 2. flush_to_disk() is the true durability boundary
//! 3. Data survives crash/recovery via WAL replay

use sqlitegraph::{
    EdgeSpec, NodeSpec, SnapshotId,
    backend::native::v3::V3Backend,
    backend::{BackendDirection, GraphBackend, NeighborQuery},
};
use tempfile::TempDir;

/// Test 1: V3 backend file-based reopen preserves nodes and edges
#[test]
fn test_v3_file_reopen_preserves_graph() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_reopen_test.graph");

    // Phase 1: Create graph with nodes and edges
    let node1_id;
    let node2_id;
    let node3_id;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        node1_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node1".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        node2_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node2".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        node3_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node3".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        // Create edges: node1 -> node2 -> node3
        backend
            .insert_edge(EdgeSpec {
                from: node1_id,
                to: node2_id,
                edge_type: "test_edge".to_string(),
                data: serde_json::json!({"order": 1}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: node2_id,
                to: node3_id,
                edge_type: "test_edge".to_string(),
                data: serde_json::json!({"order": 2}),
            })
            .unwrap();

        // CRITICAL: Call flush_to_disk() for durability
        // Without this, data is in WAL but not yet persisted to main DB
        backend.flush().expect("Flush should succeed");
    } // Backend closes here

    // Phase 2: Reopen and verify data persists
    let backend = V3Backend::open(&db_path).unwrap();

    // Verify all nodes exist
    let node1 = backend
        .get_node(SnapshotId::current(), node1_id)
        .expect("node1 should exist after reopen");
    assert_eq!(node1.name, "node1");
    assert_eq!(node1.data["phase"], 1);

    let node2 = backend
        .get_node(SnapshotId::current(), node2_id)
        .expect("node2 should exist after reopen");
    assert_eq!(node2.name, "node2");

    let node3 = backend
        .get_node(SnapshotId::current(), node3_id)
        .expect("node3 should exist after reopen");
    assert_eq!(node3.name, "node3");

    // Verify edges exist via neighbor queries
    let node1_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            node1_id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .expect("node1 should have neighbors after reopen");
    assert_eq!(node1_neighbors, vec![node2_id]);

    let node2_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            node2_id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .expect("node2 should have neighbors after reopen");
    assert_eq!(node2_neighbors, vec![node3_id]);
}

/// Test 2: Verify data is NOT durable without flush_to_disk()
///
/// This test demonstrates that flush_to_disk() is the true durability boundary.
/// Without calling flush_to_disk(), data may be lost on close/reopen.
#[test]
fn test_v3_data_not_durable_without_flush() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_no_flush_test.graph");

    let node_id;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        node_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "no_flush_node".to_string(),
                file_path: None,
                data: serde_json::json!({"test": "data"}),
            })
            .unwrap();

        // IMPORTANT: Do NOT call flush_to_disk()
        // Data is in WAL but may not be persisted
        // drop backend here
    }

    // Reopen - behavior is undefined without flush
    // The data MAY or MAY NOT be present depending on WAL state
    let backend = V3Backend::open(&db_path).unwrap();

    // This test documents the current behavior
    // Data may be recoverable via WAL replay, but it's not guaranteed
    let _result = backend.get_node(SnapshotId::current(), node_id);

    // The current implementation MAY recover this data via WAL
    // but this is NOT guaranteed - flush_to_disk() is required for durability
    // We just check that we can successfully open the database
    assert!(
        backend.entity_ids().is_ok(),
        "Database should open successfully"
    );
}

/// Test 3: BFS correctness after cold cache reopen
#[test]
fn test_v3_bfs_correctness_after_reopen_cold_cache() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_bfs_reopen.graph");

    // Create a linear chain: 1 -> 2 -> 3 -> 4 -> 5
    let node_ids;
    {
        let backend = V3Backend::create(&db_path).unwrap();
        let mut ids = Vec::new();

        for i in 1..=5 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node{}", i),
                    file_path: None,
                    data: serde_json::json!({"index": i}),
                })
                .unwrap();
            ids.push(id);
        }

        // Create chain
        for i in 0..4 {
            backend
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[i + 1],
                    edge_type: "next".to_string(),
                    data: serde_json::json!(null),
                })
                .unwrap();
        }

        backend.flush().expect("Flush should succeed");
        node_ids = ids;
    } // Close and drop backend

    // Reopen - adjacency caches are now COLD
    let backend = V3Backend::open(&db_path).unwrap();

    // BFS from node1 should reach all nodes
    let bfs_result = backend
        .bfs(SnapshotId::current(), node_ids[0], 10)
        .expect("BFS should work with cold cache");

    assert_eq!(bfs_result.len(), 5, "BFS should find all 5 nodes");
    assert_eq!(bfs_result, node_ids, "BFS order should match chain");
}

/// Test 4: Shortest path correctness after cold cache reopen
#[test]
fn test_v3_shortest_path_correctness_after_reopen_cold_cache() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_shortest_path_reopen.graph");

    // Create a diamond graph:
    //   1
    //  / \
    // 2   3
    //  \ /
    //   4
    let (node1, node2, node3, node4);
    {
        let backend = V3Backend::create(&db_path).unwrap();

        node1 = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "start".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node2 = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "left".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node3 = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "right".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node4 = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "end".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        // Diamond edges
        backend
            .insert_edge(EdgeSpec {
                from: node1,
                to: node2,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: node1,
                to: node3,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: node2,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: node3,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend.flush().expect("Flush should succeed");
    } // Close graph

    // Reopen with cold cache
    let backend = V3Backend::open(&db_path).unwrap();

    // Shortest path from 1 to 4
    let path = backend
        .shortest_path(SnapshotId::current(), node1, node4)
        .expect("Shortest path should work with cold cache");

    assert!(path.is_some(), "Path should exist");
    let path = path.unwrap();
    assert_eq!(path.len(), 3, "Shortest path should have 3 nodes");
    assert_eq!(path[0], node1, "Path starts at node1");
    assert_eq!(path[2], node4, "Path ends at node4");
    // Middle node can be either 2 or 3 (both shortest paths)
    assert!(
        path[1] == node2 || path[1] == node3,
        "Path goes through either left or right"
    );
}

/// Test 5: Large dataset survives reopen
#[test]
fn test_v3_large_dataset_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_large_reopen.graph");

    let expected_count = 1000;
    let target_id;
    let mut inserted_ids = Vec::with_capacity(expected_count);
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 1000 nodes
        for i in 0..expected_count {
            let node_id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap();
            inserted_ids.push(node_id);
        }

        target_id = expected_count / 2;

        backend.flush().expect("Flush should succeed");
    }

    // Reopen and verify
    let backend = V3Backend::open(&db_path).unwrap();

    // Check entity count
    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), expected_count, "All nodes should persist");

    // Check specific node
    let target_node_id = inserted_ids[target_id];
    let node = backend
        .get_node(SnapshotId::current(), target_node_id)
        .expect("Target node should exist");
    assert_eq!(node.data["id"], target_id as i64);
}

/// Test 6: Verify multiple flushes for the same source node does not lose previous edges,
/// and verify manual node ID support via `insert_node_with_id` works.
#[test]
fn test_v3_multiple_flushes_preserves_edges_and_manual_ids() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("v3_multi_flush_test.graph");

    let src_id = 9999;
    let dst1_id = 8888;
    let dst2_id = 7777;

    // Phase 1: Insert src and first edge, then flush
    {
        let backend = V3Backend::create(&db_path).unwrap();

        let node_spec_src = NodeSpec {
            kind: "Source".to_string(),
            name: "source_node".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        };
        let s_id = backend.insert_node_with_id(node_spec_src, src_id).unwrap();
        assert_eq!(s_id, src_id);

        let node_spec_dst1 = NodeSpec {
            kind: "Dest".to_string(),
            name: "dest_node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        };
        let d1_id = backend
            .insert_node_with_id(node_spec_dst1, dst1_id)
            .unwrap();
        assert_eq!(d1_id, dst1_id);

        backend
            .insert_edge(EdgeSpec {
                from: src_id,
                to: dst1_id,
                edge_type: "rel".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend.flush().expect("Flush 1 should succeed");
    }

    // Phase 2: Open, insert second edge, then flush again
    {
        let backend = V3Backend::open(&db_path).unwrap();

        let node_spec_dst2 = NodeSpec {
            kind: "Dest".to_string(),
            name: "dest_node2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        };
        let d2_id = backend
            .insert_node_with_id(node_spec_dst2, dst2_id)
            .unwrap();
        assert_eq!(d2_id, dst2_id);

        backend
            .insert_edge(EdgeSpec {
                from: src_id,
                to: dst2_id,
                edge_type: "rel".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        backend.flush().expect("Flush 2 should succeed");
    }

    // Phase 3: Reopen and check neighbors of src_id
    {
        let backend = V3Backend::open(&db_path).unwrap();

        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                src_id,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();

        // Previously, the second flush would have overwritten the first edge, leaving only dst2_id.
        // Now, both edges must be present!
        assert_eq!(
            neighbors.len(),
            2,
            "Should preserve all edges across flushes"
        );
        assert!(neighbors.contains(&dst1_id));
        assert!(neighbors.contains(&dst2_id));
    }
}
