//! SQLite backend snapshot isolation validation tests.
//!
//! These tests verify the temporal snapshot behavior of the SQLite backend:
//!
//! - `SnapshotId::current()` (lsn == 0) works for every operation (live reads).
//! - `neighbors()` and `bfs()` ACCEPT a checkpointed historical snapshot id and
//!   serve adjacency from the MVCC version chain. An *uncheckpointed* version
//!   errors ("not available").
//! - All other operations (`get_node`, `shortest_path`, `node_degree`, `k_hop`,
//!   `chain_query`, `pattern_search`, `query_nodes_by_kind`,
//!   `query_nodes_by_name_pattern`) REJECT a checkpointed historical snapshot
//!   with "not supported for {method}()", because the version chain stores
//!   untyped adjacency only — entity properties / typed edges are live reads.
//!
//! Before the temporal version-chain work, the backend rejected every non-zero
//! LSN. Now adjacency-only reads are served historically.

use sqlitegraph::{
    EdgeSpec, NeighborQuery, NodeSpec, SnapshotId,
    backend::{BackendDirection, GraphBackend, SqliteGraphBackend},
    multi_hop::ChainStep,
    pattern::PatternQuery,
};

fn make_node_spec(name: &str) -> NodeSpec {
    NodeSpec {
        kind: "test".to_string(),
        name: name.to_string(),
        file_path: None,
        data: serde_json::json!(null),
    }
}

fn insert_edge(backend: &SqliteGraphBackend, from: i64, to: i64) {
    backend
        .insert_edge(EdgeSpec {
            from,
            to,
            edge_type: "test_edge".to_string(),
            data: serde_json::json!(null),
        })
        .unwrap();
}

/// Build a 1 → 2 graph, warm the cache, and checkpoint a version. Returns the
/// checkpointed version number to use as a historical snapshot id.
fn checkpointed_version(backend: &SqliteGraphBackend) -> u64 {
    let n1 = backend.insert_node(make_node_spec("node1")).unwrap();
    let n2 = backend.insert_node(make_node_spec("node2")).unwrap();
    insert_edge(backend, n1, n2);
    // Warm the cache so the snapshot manager captures current adjacency.
    let _ = backend.neighbors(
        SnapshotId::current(),
        n1,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    );
    backend.graph().checkpoint()
}

// ============================================================================
// current() works everywhere
// ============================================================================

#[test]
fn test_sqlite_current_snapshot_works_get_node() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let node_id = backend.insert_node(make_node_spec("test_node")).unwrap();
    let node = backend
        .get_node(SnapshotId::current(), node_id)
        .expect("current snapshot should work");
    assert_eq!(node.name, "test_node");
}

#[test]
fn test_sqlite_current_snapshot_works_all_operations() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let n1 = backend.insert_node(make_node_spec("node1")).unwrap();
    let n2 = backend.insert_node(make_node_spec("node2")).unwrap();
    let n3 = backend.insert_node(make_node_spec("node3")).unwrap();
    insert_edge(&backend, n1, n2);
    insert_edge(&backend, n2, n3);

    let current = SnapshotId::current();
    assert!(backend.get_node(current, n1).is_ok());
    assert!(
        backend
            .neighbors(
                current,
                n1,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                }
            )
            .is_ok()
    );
    assert!(backend.bfs(current, n1, 2).is_ok());
    assert!(backend.shortest_path(current, n1, n3).is_ok());
    assert!(backend.node_degree(current, n1).is_ok());
    assert!(
        backend
            .k_hop(current, n1, 2, BackendDirection::Outgoing)
            .is_ok()
    );
    assert!(
        backend
            .k_hop_filtered(current, n1, 2, BackendDirection::Outgoing, &["test_edge"])
            .is_ok()
    );
    assert!(
        backend
            .chain_query(
                current,
                n1,
                &[ChainStep {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                }]
            )
            .is_ok()
    );
    assert!(
        backend
            .pattern_search(current, n1, &PatternQuery::default())
            .is_ok()
    );
    assert!(backend.query_nodes_by_kind(current, "test").is_ok());
    assert!(
        backend
            .query_nodes_by_name_pattern(current, "node*")
            .is_ok()
    );
}

// ============================================================================
// neighbors() / bfs() ACCEPT a checkpointed historical snapshot
// ============================================================================

#[test]
fn test_sqlite_historical_snapshot_neighbors_succeeds() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let node1 = 1; // first inserted node
    let neighbors = backend
        .neighbors(
            SnapshotId::from_lsn(v),
            node1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .expect("checkpointed historical neighbors should succeed");
    assert_eq!(neighbors, vec![2]);
}

#[test]
fn test_sqlite_historical_snapshot_bfs_succeeds() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let bfs = backend
        .bfs(SnapshotId::from_lsn(v), 1, 5)
        .expect("checkpointed historical bfs should succeed");
    assert_eq!(bfs, vec![1, 2]);
}

#[test]
fn test_sqlite_uncheckpointed_version_errors_neighbors() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    backend.insert_node(make_node_spec("node1")).unwrap();
    // Version 999 was never checkpointed.
    let result = backend.neighbors(
        SnapshotId::from_lsn(999),
        1,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    );
    assert!(result.is_err(), "uncheckpointed version should error");
    assert!(
        result.unwrap_err().to_string().contains("not available"),
        "error should mention the version is not available"
    );
}

#[test]
fn test_sqlite_uncheckpointed_version_errors_bfs() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    backend.insert_node(make_node_spec("node1")).unwrap();
    let result = backend.bfs(SnapshotId::from_lsn(555), 1, 2);
    assert!(result.is_err(), "uncheckpointed version should error");
}

// ============================================================================
// Live-only operations REJECT a checkpointed historical snapshot
// ============================================================================

#[test]
fn test_sqlite_historical_snapshot_rejected_get_node() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.get_node(SnapshotId::from_lsn(v), 1);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_shortest_path() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.shortest_path(SnapshotId::from_lsn(v), 1, 2);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_node_degree() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.node_degree(SnapshotId::from_lsn(v), 1);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_k_hop() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.k_hop(SnapshotId::from_lsn(v), 1, 2, BackendDirection::Outgoing);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_k_hop_filtered() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.k_hop_filtered(
        SnapshotId::from_lsn(v),
        1,
        2,
        BackendDirection::Outgoing,
        &["x"],
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_chain_query() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.chain_query(
        SnapshotId::from_lsn(v),
        1,
        &[ChainStep {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        }],
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_pattern_search() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.pattern_search(SnapshotId::from_lsn(v), 1, &PatternQuery::default());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_query_nodes_by_kind() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.query_nodes_by_kind(SnapshotId::from_lsn(v), "test");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}

#[test]
fn test_sqlite_historical_snapshot_rejected_query_nodes_by_name_pattern() {
    let backend = SqliteGraphBackend::in_memory().unwrap();
    let v = checkpointed_version(&backend);
    let result = backend.query_nodes_by_name_pattern(SnapshotId::from_lsn(v), "node*");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not supported for")
    );
}
