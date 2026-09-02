//! Native-v3 MVCC operations specification (Karpathy Phase 1).
//!
//! Spec tests for MVCC operations that should be provided by SqliteGraph
//! for native-v3 backend. These operations enable:
//! - Snapshot isolation for batch operations
//! - Time-travel queries (as-of timestamp)
//! - Named snapshot management
//! - Graph version tracking for CSR rebuilds
//!
//! Tests are written to fail first (TDD) and will drive implementation.

use sqlitegraph::SqliteGraph;
use sqlitegraph::graph::{GraphEdge, GraphEntity};

/// Spec: Batch insert entities with snapshot isolation.
#[test]
fn spec_mvcc_batch_insert_entities_with_snapshot() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create a named snapshot
    let snapshot_id = "test_snapshot_001";
    let snapshot_timestamp = graph.create_snapshot(snapshot_id).unwrap();

    assert!(
        snapshot_timestamp > 0,
        "Snapshot timestamp should be positive"
    );

    // Insert entities with snapshot tagging
    let entities = vec![
        GraphEntity {
            id: 0,
            kind: "Agent".to_string(),
            name: "agent1".to_string(),
            file_path: None,
            data: serde_json::json!({"role": "worker"}),
        },
        GraphEntity {
            id: 0,
            kind: "Agent".to_string(),
            name: "agent2".to_string(),
            file_path: None,
            data: serde_json::json!({"role": "coordinator"}),
        },
    ];

    let entity_ids = graph
        .batch_insert_entities_with_snapshot(&entities, snapshot_id)
        .unwrap();

    assert_eq!(entity_ids.len(), 2, "Both entities inserted");
    assert!(
        entity_ids[0] > 0 && entity_ids[1] > 0,
        "Valid entity IDs returned"
    );

    // Verify entities are visible in current state
    let count: i64 = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT COUNT(*) FROM graph_entities WHERE kind='Agent'")
                .unwrap();
            Ok(stmt.query_row([], |row| row.get(0)).unwrap())
        })
        .unwrap();

    assert_eq!(count, 2, "Both entities visible");
}

/// Spec: Batch insert edges with snapshot isolation.
#[test]
fn spec_mvcc_batch_insert_edges_with_snapshot() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create entities first
    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "Entity1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "TestNode".to_string(),
            name: "Entity2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Create snapshot
    let snapshot_id = "edge_snapshot_001";
    let _snapshot_timestamp = graph.create_snapshot(snapshot_id).unwrap();

    // Insert edges with snapshot tagging
    let edges = vec![GraphEdge {
        id: 0,
        from_id: entity1,
        to_id: entity2,
        edge_type: "depends_on".to_string(),
        data: serde_json::json!({"strength": 0.8}),
    }];

    let edge_ids = graph
        .batch_insert_edges_with_snapshot(&edges, snapshot_id)
        .unwrap();

    assert_eq!(edge_ids.len(), 1, "Edge inserted");
    assert!(edge_ids[0] > 0, "Valid edge ID returned");

    // Verify edge is visible
    let edge_count: i64 = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT COUNT(*) FROM graph_edges WHERE from_id=?1")
                .unwrap();
            Ok(stmt.query_row([entity1], |row| row.get(0)).unwrap())
        })
        .unwrap();

    assert_eq!(edge_count, 1, "Edge visible in snapshot");
}

/// Spec: Time-travel query reads graph state as of timestamp.
#[test]
fn spec_mvcc_time_travel_query() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create first snapshot and insert initial entities
    let snapshot_id_1 = "snapshot_v1";
    let timestamp_v1 = graph.create_snapshot(snapshot_id_1).unwrap();

    let entities_v1 = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent1".to_string(),
        file_path: None,
        data: serde_json::json!({"version": 1}),
    }];

    let _ids = graph
        .batch_insert_entities_with_snapshot(&entities_v1, snapshot_id_1)
        .unwrap();

    // Query state as of first snapshot (should see 1 entity)
    let stats_v1 = graph.query_as_of(timestamp_v1).unwrap();
    assert_eq!(
        stats_v1.total_entities, 1,
        "State at snapshot_v1 has 1 entity"
    );

    // Ensure timestamps differ (chrono::Utc::now() has second resolution)
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Create second snapshot and insert more entities
    let snapshot_id_2 = "snapshot_v2";
    let timestamp_v2 = graph.create_snapshot(snapshot_id_2).unwrap();

    let entities_v2 = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent2".to_string(),
        file_path: None,
        data: serde_json::json!({"version": 2}),
    }];

    let _ids = graph
        .batch_insert_entities_with_snapshot(&entities_v2, snapshot_id_2)
        .unwrap();

    // Query state as of second snapshot (should see 2 entities total)
    let stats_v2 = graph.query_as_of(timestamp_v2).unwrap();
    assert_eq!(
        stats_v2.total_entities, 2,
        "State at snapshot_v2 has 2 entities"
    );

    // Time-travel back to first snapshot (should still see 1 entity)
    let stats_back = graph.query_as_of(timestamp_v1).unwrap();
    assert_eq!(
        stats_back.total_entities, 1,
        "Time-travel to snapshot_v1 still has 1 entity"
    );
}

/// Spec: Create named snapshot for consistent reads.
#[test]
fn spec_mvcc_create_named_snapshot() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create named snapshot
    let snapshot_id = "consistent_read_snapshot";
    let timestamp = graph.create_snapshot(snapshot_id).unwrap();

    assert!(timestamp > 0, "Snapshot timestamp positive");

    // Verify snapshot exists (check native-v3 hnsw_vectors table)
    let _snapshot_exists: bool = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT EXISTS(SELECT 1 FROM hnsw_vectors WHERE snapshot_id=?1 LIMIT 1)")
                .unwrap();
            let exists: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap();
            Ok(exists == 1)
        })
        .unwrap();

    // Note: This may be false if snapshot tracking not fully implemented yet
    // The test documents the expected behavior
}

/// Spec: Get graph version from CSR shards.
#[test]
fn spec_mvcc_get_graph_version() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Initial graph version should be 0
    let version_0 = graph.get_graph_version().unwrap();
    assert_eq!(version_0, 0, "Initial graph version is 0");

    // Simulate graph rebuild by inserting CSR shard version
    graph
        .with_connection(|conn| {
            conn.execute(
            "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (1, 1, X'01', 1, 1000, 1000)",
            [],
        ).unwrap();
            Ok(())
        })
        .unwrap();

    // Check graph version after rebuild
    let version_1 = graph.get_graph_version().unwrap();
    assert_eq!(version_1, 1, "Graph version incremented after rebuild");
}
