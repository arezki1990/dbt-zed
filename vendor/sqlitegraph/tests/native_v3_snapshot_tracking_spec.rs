//! Native-v3 MVCC Snapshot Tracking Specification (Karpathy Phase 1).
//!
//! Spec tests for full snapshot isolation and time-travel query optimization
//! in native-v3 storage. These operations complete the MVCC system:
//!
//! - Snapshot metadata table for tracking named snapshots
//! - Entity/edge versioning with snapshot tags
//! - Time-travel queries using visible_at/snapshot_id columns
//! - Snapshot cleanup and retention policies
//!
//! Tests are written to fail first (TDD) and will drive implementation.

use sqlitegraph::SqliteGraph;
use sqlitegraph::graph::{GraphEdge, GraphEntity};

/// Spec: Create snapshot persists metadata record.
#[test]
fn spec_snapshot_metadata_persisted() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "test_snapshot_001";
    let timestamp = graph.create_snapshot(snapshot_id).unwrap();

    assert!(timestamp > 0, "Snapshot timestamp positive");

    // Verify snapshot metadata record exists
    let record_exists: bool = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT EXISTS(SELECT 1 FROM snapshots WHERE snapshot_id=?1 LIMIT 1)")
                .unwrap();
            let exists: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap();
            Ok(exists == 1)
        })
        .unwrap();

    assert!(record_exists, "Snapshot metadata should be persisted");
}

/// Spec: Batch insert entities tags rows with snapshot_id.
#[test]
fn spec_entity_insert_tagged_with_snapshot() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "entity_snapshot_001";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    let entities = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent1".to_string(),
        file_path: None,
        data: serde_json::json!({"role": "worker"}),
    }];

    let ids = graph
        .batch_insert_entities_with_snapshot(&entities, snapshot_id)
        .unwrap();
    assert_eq!(ids.len(), 1);

    // Verify entity is tagged with snapshot_id
    let tagged: bool = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT snapshot_id FROM graph_entities WHERE id=?1")
                .unwrap();
            let tag: String = stmt.query_row([ids[0]], |row| row.get(0)).unwrap();
            Ok(tag == snapshot_id)
        })
        .unwrap();

    assert!(tagged, "Entity should be tagged with snapshot_id");
}

/// Spec: Batch insert edges tags rows with snapshot_id.
#[test]
fn spec_edge_insert_tagged_with_snapshot() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create entities first
    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Node".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Node".to_string(),
            name: "node2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let snapshot_id = "edge_snapshot_001";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    let edges = vec![GraphEdge {
        id: 0,
        from_id: entity1,
        to_id: entity2,
        edge_type: "connects".to_string(),
        data: serde_json::json!({"weight": 1.0}),
    }];

    let ids = graph
        .batch_insert_edges_with_snapshot(&edges, snapshot_id)
        .unwrap();
    assert_eq!(ids.len(), 1);

    // Verify edge is tagged with snapshot_id
    let tagged: bool = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT snapshot_id FROM graph_edges WHERE id=?1")
                .unwrap();
            let tag: String = stmt.query_row([ids[0]], |row| row.get(0)).unwrap();
            Ok(tag == snapshot_id)
        })
        .unwrap();

    assert!(tagged, "Edge should be tagged with snapshot_id");
}

/// Spec: Time-travel query filters by visible_at timestamp.
#[test]
fn spec_time_travel_filters_by_visible_at() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create first snapshot and insert entities
    let snapshot_id_1 = "snapshot_t1";
    let timestamp_t1 = graph.create_snapshot(snapshot_id_1).unwrap();

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

    // Query state at t1 (should see 1 entity)
    let stats_t1 = graph.query_as_of(timestamp_t1).unwrap();
    assert_eq!(stats_t1.total_entities, 1, "t1 should see initial entity");

    // Ensure timestamps differ
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Create second snapshot and insert more entities
    let snapshot_id_2 = "snapshot_t2";
    let timestamp_t2 = graph.create_snapshot(snapshot_id_2).unwrap();

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

    // Query state at t2 (should see 2 entities)
    let stats_t2 = graph.query_as_of(timestamp_t2).unwrap();
    assert_eq!(stats_t2.total_entities, 2, "t2 should see both entities");

    // Time-travel back to t1 (should see 1 entity)
    let stats_back = graph.query_as_of(timestamp_t1).unwrap();
    assert_eq!(
        stats_back.total_entities, 1,
        "time-travel to t1 should see 1 entity"
    );
}

/// Spec: Snapshot list query returns all snapshots.
#[test]
fn spec_list_snapshots_returns_metadata() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "list_test_snapshot";
    let timestamp = graph.create_snapshot(snapshot_id).unwrap();

    // List snapshots and verify ours appears
    let snapshots = graph.list_snapshots().unwrap();
    assert!(!snapshots.is_empty(), "Should have at least one snapshot");

    let found = snapshots
        .iter()
        .any(|s| s.snapshot_id == snapshot_id && s.timestamp == timestamp);

    assert!(found, "Created snapshot should appear in list");
}

/// Spec: Snapshot deletion removes metadata and cascades.
#[test]
fn spec_snapshot_deletion_cascades() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "delete_test_snapshot";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    // Delete snapshot
    graph.delete_snapshot(snapshot_id).unwrap();

    // Verify metadata removed
    let exists: bool = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT EXISTS(SELECT 1 FROM snapshots WHERE snapshot_id=?1)")
                .unwrap();
            let ex: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap();
            Ok(ex == 1)
        })
        .unwrap();

    assert!(!exists, "Snapshot metadata should be deleted");
}
