//! SQLite Backend Reopen/Durability Tests
//!
//! These tests verify that data persists correctly across close/reopen cycles
//! for the SQLite-based backend. They prove "reopen truth" not just
//! "same-process truth".
//!
//! **Key distinction**: These tests DROP the graph and reopen from file,
//! ensuring correctness comes from persistent storage, not in-memory caches.

use sqlitegraph::{
    EdgeSpec, GraphConfig, NodeSpec, SnapshotId,
    backend::{BackendDirection, NeighborQuery},
    open_graph,
};

/// Test 1: SQLite backend file-based reopen preserves nodes and edges
#[test]
fn test_sqlite_file_reopen_preserves_graph() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("reopen_test.db");

    // Phase 1: Create graph with nodes and edges
    let node1_id;
    let node2_id;
    let node3_id;
    {
        let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();

        node1_id = graph
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node1".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        node2_id = graph
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node2".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        node3_id = graph
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: "node3".to_string(),
                file_path: None,
                data: serde_json::json!({"phase": 1}),
            })
            .unwrap();

        // Create edges: node1 -> node2 -> node3
        graph
            .insert_edge(EdgeSpec {
                from: node1_id,
                to: node2_id,
                edge_type: "test_edge".to_string(),
                data: serde_json::json!({"order": 1}),
            })
            .unwrap();

        graph
            .insert_edge(EdgeSpec {
                from: node2_id,
                to: node3_id,
                edge_type: "test_edge".to_string(),
                data: serde_json::json!({"order": 2}),
            })
            .unwrap();
    } // Graph closes here

    // Phase 2: Reopen and verify data persists
    let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();

    // Verify all nodes exist
    let node1 = graph
        .get_node(SnapshotId::current(), node1_id)
        .expect("node1 should exist after reopen");
    assert_eq!(node1.name, "node1");
    assert_eq!(node1.data["phase"], 1);

    let node2 = graph
        .get_node(SnapshotId::current(), node2_id)
        .expect("node2 should exist after reopen");
    assert_eq!(node2.name, "node2");

    let node3 = graph
        .get_node(SnapshotId::current(), node3_id)
        .expect("node3 should exist after reopen");
    assert_eq!(node3.name, "node3");

    // Verify edges exist via neighbor queries
    let node1_neighbors = graph
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

    let node2_neighbors = graph
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

/// Test 2: BFS correctness after cold cache reopen
///
/// This test proves that BFS produces correct results even when
/// adjacency caches are empty (cold cache after reopen).
#[test]
fn test_bfs_correctness_after_reopen_cold_cache() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("bfs_reopen.db");

    // Create a linear chain: 1 -> 2 -> 3 -> 4 -> 5
    let node_ids;
    {
        let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();
        let mut ids = Vec::new();

        for i in 1..=5 {
            let id = graph
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
            graph
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[i + 1],
                    edge_type: "next".to_string(),
                    data: serde_json::json!(null),
                })
                .unwrap();
        }

        node_ids = ids;
    } // Close and drop graph

    // Reopen - adjacency caches are now COLD
    let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();

    // BFS from node1 should reach all nodes
    let bfs_result = graph
        .bfs(SnapshotId::current(), node_ids[0], 10)
        .expect("BFS should work with cold cache");

    assert_eq!(bfs_result.len(), 5, "BFS should find all 5 nodes");
    assert_eq!(bfs_result, node_ids, "BFS order should match chain");
}

/// Test 3: Shortest path correctness after cold cache reopen
#[test]
fn test_shortest_path_correctness_after_reopen_cold_cache() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("shortest_path_reopen.db");

    // Create a diamond graph:
    //   1
    //  / \
    // 2   3
    //  \ /
    //   4
    let (node1, node2, node3, node4);
    {
        let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();

        node1 = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "start".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node2 = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "left".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node3 = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "right".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        node4 = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: "end".to_string(),
                file_path: None,
                data: serde_json::json!(null),
            })
            .unwrap();

        // Diamond edges
        graph
            .insert_edge(EdgeSpec {
                from: node1,
                to: node2,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        graph
            .insert_edge(EdgeSpec {
                from: node1,
                to: node3,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        graph
            .insert_edge(EdgeSpec {
                from: node2,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();

        graph
            .insert_edge(EdgeSpec {
                from: node3,
                to: node4,
                edge_type: "edge".to_string(),
                data: serde_json::json!(null),
            })
            .unwrap();
    } // Close graph

    // Reopen with cold cache
    let graph = open_graph(&db_path, &GraphConfig::sqlite()).unwrap();

    // Shortest path from 1 to 4
    let path = graph
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

/// Test 4: HNSW index persistence across file close/reopen
///
/// This test verifies that HNSW indexes and their vectors persist
/// correctly across close/reopen cycles using direct database access.
#[test]
fn test_hnsw_persistence_across_reopen() {
    use rusqlite::Connection;
    use sqlitegraph::hnsw::{DistanceMetric, HnswConfig, HnswIndex};
    use sqlitegraph::schema;

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("hnsw_reopen.db");
    let index_name = "test_index";

    // Phase 1: Create index with persistent storage and insert vectors
    {
        let conn = Connection::open(&db_path).unwrap();
        schema::ensure_schema(&conn).unwrap();

        let config = HnswConfig::new(3, 16, 200, DistanceMetric::Euclidean);
        let mut hnsw = HnswIndex::with_persistent_storage(index_name, config, conn)
            .expect("Create HNSW with persistent storage should succeed");

        // Insert test vectors
        let vectors = [
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0],
        ];

        for (i, vec_data) in vectors.iter().enumerate() {
            hnsw.insert_vector(vec_data, Some(serde_json::json!(format!("vec_{}", i))))
                .expect("Insert should succeed");
        }
    } // Connection closes

    // Phase 2: Reopen and verify HNSW loads correctly
    let conn = Connection::open(&db_path).unwrap();
    let hnsw =
        HnswIndex::load_with_vectors(&conn, index_name).expect("HNSW should load after reopen");

    assert_eq!(hnsw.vector_count(), 4, "All 4 vectors should be preserved");

    // Verify search works
    let query = vec![1.0, 0.0, 0.0];
    let results = hnsw.search(&query, 2).expect("Search should work");

    assert!(!results.is_empty(), "Search should return results");
    let (_best_id, distance) = &results[0];
    assert!(*distance < 0.1, "Best match should be very close to query");
}
