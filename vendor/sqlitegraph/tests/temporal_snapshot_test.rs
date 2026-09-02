//! Integration tests for the temporal version chain (MVCC historical reads).
//!
//! Exercises the full path: checkpoint() → mutate → neighbors(snapshot_id=N)
//! reads the historical adjacency, not the live graph.

use serde_json::json;
use sqlitegraph::snapshot::SnapshotId;
use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec, SqliteGraphBackend,
};

fn insert_node(backend: &SqliteGraphBackend, name: &str) -> i64 {
    backend
        .insert_node(NodeSpec {
            kind: "test".to_string(),
            name: name.to_string(),
            file_path: None,
            data: json!({}),
        })
        .expect("insert_node should succeed")
}

fn insert_edge(backend: &SqliteGraphBackend, from: i64, to: i64, edge_type: &str) {
    backend
        .insert_edge(EdgeSpec {
            from,
            to,
            edge_type: edge_type.to_string(),
            data: json!({}),
        })
        .expect("insert_edge should succeed");
}

fn outgoing(backend: &SqliteGraphBackend, snapshot: SnapshotId, node: i64) -> Vec<i64> {
    let mut result = backend
        .neighbors(
            snapshot,
            node,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .expect("neighbors should succeed");
    result.sort();
    result
}

#[test]
fn test_checkpoint_and_historical_neighbors() {
    let backend = SqliteGraphBackend::in_memory().expect("create backend");

    // Build a small graph: 1 → 2 → 3
    let n1 = insert_node(&backend, "n1");
    let n2 = insert_node(&backend, "n2");
    let n3 = insert_node(&backend, "n3");
    insert_edge(&backend, n1, n2, "flow");
    insert_edge(&backend, n2, n3, "flow");

    // Warm the cache so the snapshot manager sees current adjacency.
    let _ = outgoing(&backend, SnapshotId::current(), n1);
    let _ = outgoing(&backend, SnapshotId::current(), n2);

    // Checkpoint version 1: 1 → {2}, 2 → {3}
    let v1 = backend.graph().checkpoint();
    assert_eq!(v1, 1);

    // Mutate: add edge 1 → 3
    insert_edge(&backend, n1, n3, "flow");

    // Re-warm cache so the live state reflects the new edge.
    let _ = outgoing(&backend, SnapshotId::current(), n1);

    // Checkpoint version 2: 1 → {2, 3}, 2 → {3}
    let v2 = backend.graph().checkpoint();
    assert_eq!(v2, 2);

    // Live read sees the new edge.
    let live = outgoing(&backend, SnapshotId::current(), n1);
    assert_eq!(live, vec![n2, n3]);

    // Historical read at version 1 does NOT see the new edge.
    let hist1 = outgoing(&backend, SnapshotId::from_lsn(v1), n1);
    assert_eq!(hist1, vec![n2], "version 1 should only have n2");

    // Historical read at version 2 DOES see the new edge.
    let hist2 = outgoing(&backend, SnapshotId::from_lsn(v2), n1);
    assert_eq!(hist2, vec![n2, n3], "version 2 should have n2 and n3");
}

#[test]
fn test_historical_bfs_traverses_old_adjacency() {
    let backend = SqliteGraphBackend::in_memory().expect("create backend");

    // 1 → 2
    let n1 = insert_node(&backend, "n1");
    let n2 = insert_node(&backend, "n2");
    let n3 = insert_node(&backend, "n3");
    insert_edge(&backend, n1, n2, "flow");

    // Warm + checkpoint v1: BFS from 1 reaches {1, 2}
    let _ = outgoing(&backend, SnapshotId::current(), n1);
    let v1 = backend.graph().checkpoint();

    // Add 2 → 3, warm + checkpoint v2: BFS from 1 reaches {1, 2, 3}
    insert_edge(&backend, n2, n3, "flow");
    let _ = outgoing(&backend, SnapshotId::current(), n1);
    let _ = outgoing(&backend, SnapshotId::current(), n2);
    let v2 = backend.graph().checkpoint();

    // Live BFS reaches all 3.
    let mut live_bfs = backend.bfs(SnapshotId::current(), n1, 5).expect("live bfs");
    live_bfs.sort();
    assert_eq!(live_bfs, vec![n1, n2, n3]);

    // Historical BFS at v1: edge 2→3 didn't exist → only reaches {1, 2}.
    let mut hist_bfs = backend
        .bfs(SnapshotId::from_lsn(v1), n1, 5)
        .expect("historical bfs");
    hist_bfs.sort();
    assert_eq!(hist_bfs, vec![n1, n2], "v1 bfs should not reach n3");

    // Historical BFS at v2: reaches all 3.
    let mut hist_bfs2 = backend
        .bfs(SnapshotId::from_lsn(v2), n1, 5)
        .expect("historical bfs v2");
    hist_bfs2.sort();
    assert_eq!(hist_bfs2, vec![n1, n2, n3]);
}

#[test]
fn test_uncheckpointed_version_errors() {
    let backend = SqliteGraphBackend::in_memory().expect("create backend");
    let n1 = insert_node(&backend, "n1");

    // Version 5 was never checkpointed.
    let result = backend.neighbors(
        SnapshotId::from_lsn(5),
        n1,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    );
    assert!(result.is_err(), "uncheckpointed version should error");
}

#[test]
fn test_live_only_methods_reject_historical() {
    let backend = SqliteGraphBackend::in_memory().expect("create backend");
    let n1 = insert_node(&backend, "n1");
    let _ = outgoing(&backend, SnapshotId::current(), n1);
    let v1 = backend.graph().checkpoint();

    // get_node doesn't support historical (entity data not in version chain).
    let result = backend.get_node(SnapshotId::from_lsn(v1), n1);
    assert!(
        result.is_err(),
        "get_node should reject historical snapshot"
    );
}

#[test]
fn test_version_chain_metadata() {
    let backend = SqliteGraphBackend::in_memory().expect("create backend");
    insert_node(&backend, "n1");
    let _ = backend
        .neighbors(
            SnapshotId::current(),
            1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .ok();

    assert_eq!(backend.graph().snapshot_version_count(), 0);
    let v1 = backend.graph().checkpoint();
    let v2 = backend.graph().checkpoint();
    assert_eq!(backend.graph().snapshot_version_count(), 2);
    assert_eq!(backend.graph().snapshot_oldest_version(), Some(v1));
    assert_eq!(backend.graph().snapshot_newest_version(), Some(v2));

    backend.graph().clear_snapshot_history();
    assert_eq!(backend.graph().snapshot_version_count(), 0);
}
