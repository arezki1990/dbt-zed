//! Native-v3 scale optimization specification (Karpathy Phase 1).
//!
//! Spec tests for performance optimizations at scale:
//! - Pre-aggregated stats table (snapshot_stats)
//! - Composite indexes for snapshot queries
//! - Partition-ready schema design
//!
//! Tests are written to fail first (TDD) and will drive implementation.

use sqlitegraph::SqliteGraph;
use sqlitegraph::graph::{GraphEdge, GraphEntity};

/// Spec: Pre-aggregated stats table for O(1) snapshot counts.
#[test]
fn spec_snapshot_stats_table_exists() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Verify snapshot_stats table exists
    let table_exists: bool = graph.with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='snapshot_stats')")
            .unwrap();
        let exists: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        Ok(exists == 1)
    }).unwrap();

    assert!(table_exists, "snapshot_stats table should exist");
}

/// Spec: Snapshot stats updated on batch insert.
#[test]
fn spec_snapshot_stats_updated_on_batch_insert() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "stats_snapshot_001";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    let entities = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent1".to_string(),
        file_path: None,
        data: serde_json::json!({"role": "worker"}),
    }];

    graph
        .batch_insert_entities_with_snapshot(&entities, snapshot_id)
        .unwrap();

    // Verify stats table has correct count
    let count: i64 = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT entity_count FROM snapshot_stats WHERE snapshot_id=?1")
                .unwrap();
            let count: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap_or(0);
            Ok(count)
        })
        .unwrap();

    assert_eq!(count, 1, "Stats table should reflect entity count");
}

/// Spec: Time-travel query uses snapshot_stats (O(1) not O(N)).
#[test]
fn spec_time_travel_uses_pre_aggregated_stats() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "time_travel_test";
    let timestamp = graph.create_snapshot(snapshot_id).unwrap();

    let entities = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent1".to_string(),
        file_path: None,
        data: serde_json::json!({"version": 1}),
    }];

    graph
        .batch_insert_entities_with_snapshot(&entities, snapshot_id)
        .unwrap();

    // Query stats as-of timestamp should use snapshot_stats table
    let stats = graph.query_as_of(timestamp).unwrap();
    assert_eq!(
        stats.total_entities, 1,
        "Time-travel query returns correct count"
    );
}

/// Spec: Composite index (snapshot_id, created_at) exists.
#[test]
fn spec_composite_index_exists() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Verify composite index exists
    let index_exists: bool = graph.with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_graph_entities_snapshot_created')")
            .unwrap();
        let exists: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        Ok(exists == 1)
    }).unwrap();

    assert!(index_exists, "Composite index should exist");
}

/// Spec: Snapshot stats include edge counts.
#[test]
fn spec_snapshot_stats_tracks_edges() {
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

    let snapshot_id = "edge_stats_test";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    let edges = vec![GraphEdge {
        id: 0,
        from_id: entity1,
        to_id: entity2,
        edge_type: "connects".to_string(),
        data: serde_json::json!({"weight": 1.0}),
    }];

    graph
        .batch_insert_edges_with_snapshot(&edges, snapshot_id)
        .unwrap();

    // Verify stats table tracks edges
    let edge_count: i64 = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT edge_count FROM snapshot_stats WHERE snapshot_id=?1")
                .unwrap();
            let count: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap_or(0);
            Ok(count)
        })
        .unwrap();

    assert_eq!(edge_count, 1, "Stats table should track edge counts");
}

/// Spec: Snapshot stats support ON CONFLICT upsert.
#[test]
fn spec_snapshot_stats_upsert() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let snapshot_id = "upsert_test";
    let _timestamp = graph.create_snapshot(snapshot_id).unwrap();

    // Insert entities in first batch
    let entities_v1 = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent1".to_string(),
        file_path: None,
        data: serde_json::json!({"version": 1}),
    }];

    graph
        .batch_insert_entities_with_snapshot(&entities_v1, snapshot_id)
        .unwrap();

    // Insert more entities in second batch (should update stats)
    let entities_v2 = vec![GraphEntity {
        id: 0,
        kind: "Agent".to_string(),
        name: "agent2".to_string(),
        file_path: None,
        data: serde_json::json!({"version": 2}),
    }];

    graph
        .batch_insert_entities_with_snapshot(&entities_v2, snapshot_id)
        .unwrap();

    // Verify stats updated (should be 2 total, not 1)
    let count: i64 = graph
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT entity_count FROM snapshot_stats WHERE snapshot_id=?1")
                .unwrap();
            let count: i64 = stmt.query_row([snapshot_id], |row| row.get(0)).unwrap_or(0);
            Ok(count)
        })
        .unwrap();

    assert_eq!(count, 2, "Stats should upsert, not duplicate");
}
