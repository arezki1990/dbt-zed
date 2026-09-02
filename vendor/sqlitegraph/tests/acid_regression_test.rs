//! Regression test for ACID bug: public APIs could observe uncommitted state
//!
//! This test reproduces the original bug where read operations could see
//! uncommitted data because they bypassed the transaction system.
//!
//! **Hard Rule:** No API may observe state not bound to a committed snapshot_id.
//! If a value cannot be tied to a committed snapshot → it does not exist.
//!
//! # Implementation Status
//!
//! - ✅ **Phase 38-03 COMPLETE**: GraphBackend trait updated with snapshot_id parameters
//! - ⏳ **Phase 38-04 PENDING**: WAL filtering not yet implemented
//! - ❌ **Native/SQLite implementations**: Need to be updated to match trait signature
//!
//! # Current Blocker
//!
//! The GraphBackend trait has been updated with snapshot_id parameters, but the
//! NativeGraphBackend and SqliteGraphBackend implementations haven't been updated yet.
//! This causes compilation errors:
//!
//! ```text
//! error[E0050]: method `get_node` has 2 parameters but the declaration in trait `GraphBackend::get_node` has 3
//! ```
//!
//! Once 38-04 (WAL filtering) is complete and implementations are updated, these tests
//! should compile and verify snapshot isolation works correctly.

use sqlitegraph::snapshot::SnapshotId;

// Note: These tests verify SnapshotId type functionality (38-02 - COMPLETE).
// Full integration tests are blocked until implementations match trait signature (38-04).

// ============================================================================
// SnapshotId Type Tests (Phase 38-02 - COMPLETE)
// ============================================================================

/// Verify SnapshotId::current() returns a valid snapshot
///
/// Current implementation returns SnapshotId(0) to indicate "all committed data".
/// Future enhancement will track max committed transaction ID from WAL.
#[test]
fn test_snapshot_id_current() {
    let snapshot = SnapshotId::current();
    assert!(
        snapshot.is_valid(),
        "Current snapshot should always be valid"
    );
    assert_eq!(
        snapshot.as_u64(),
        0,
        "Current implementation returns 0 (all committed data)"
    );
}

/// Verify SnapshotId can be created from explicit transaction ID
///
/// This is used when you want to read from a specific historical snapshot.
#[test]
fn test_snapshot_id_from_tx() {
    let tx_id = 12345;
    let snapshot = SnapshotId::from_tx(tx_id);
    assert_eq!(snapshot.as_u64(), tx_id);
    assert!(snapshot.is_valid());
}

/// Verify invalid sentinel value works for error cases
///
/// Used to indicate "no valid snapshot exists" in error paths.
#[test]
fn test_snapshot_id_invalid() {
    let invalid = SnapshotId::invalid();
    assert!(!invalid.is_valid());
    assert_eq!(invalid.as_u64(), u64::MAX);
}

/// Verify SnapshotId implements Copy, Clone, Hash, Eq, PartialEq
///
/// These traits are required for snapshot_id to be used as:
/// - Hash map keys (caching layers)
/// - Copyable parameters (API ergonomics)
/// - Comparable values (snapshot ordering)
#[test]
fn test_snapshot_id_traits() {
    use std::collections::HashMap;

    // Copy: Clone, not move
    let s1 = SnapshotId(100);
    let s2 = s1;
    assert_eq!(s1.as_u64(), 100, "Copy should work");
    assert_eq!(s2.as_u64(), 100);

    // Eq: PartialEq
    let s3 = SnapshotId(100);
    let s4 = SnapshotId(200);
    assert_eq!(s1, s3, "Equal snapshots should compare equal");
    assert_ne!(s1, s4, "Different snapshots should not be equal");

    // Hash: Can be used as HashMap key
    let mut map = HashMap::new();
    map.insert(SnapshotId(100), "snapshot_100");
    map.insert(SnapshotId(200), "snapshot_200");
    // Duplicate key overwrites (normal HashMap behavior)
    map.insert(SnapshotId(100), "duplicate");
    assert_eq!(map.len(), 2);
    // After overwrite, key 100 has "duplicate" value
    assert_eq!(map.get(&SnapshotId(100)), Some(&"duplicate"));
}

// ============================================================================
// Regression Test Specifications (Blocked until 38-04 implementation complete)
// ============================================================================

// Note: The following test specifications are documented but commented out
// because the GraphBackend implementations haven't been updated to match
// the new trait signature (with snapshot_id parameters).
//
// Once 38-04 is complete, uncomment these tests to verify snapshot isolation.

/*
/// **REGRESSION TEST**: Original bug - public APIs could observe uncommitted state
///
/// # Expected Behavior (after 38-04 implementation)
///
/// 1. Create graph
/// 2. Take snapshot_1 before any writes
/// 3. Insert node (commits to snapshot_2)
/// 4. Read with snapshot_1 → should NOT see the new node
/// 5. Read with snapshot_2 → should see the new node
///
/// # Implementation Required
///
/// - [ ] GraphBackend implementations updated to match trait signature
/// - [ ] WAL filtering filters records by tx_id <= snapshot_id
/// - [ ] open_graph() API provides GraphBackend wrapper
#[test]
fn test_no_uncommitted_reads_via_public_api() {
    use sqlitegraph::{open_graph, GraphConfig, BackendKind, snapshot::SnapshotId};
    use tempfile::NamedTempFile;

    let temp = NamedTempFile::new().unwrap();
    let cfg = GraphConfig::native();
    let graph = open_graph(temp.path(), &cfg).unwrap();

    // Get snapshot BEFORE any writes
    let snapshot_before = SnapshotId::current();

    // Insert node (should auto-commit and create new snapshot)
    let _node_id = graph.insert_node(NodeSpec {
        kind: "test".to_string(),
        name: "uncommitted".to_string(),
        data: None,
    }).unwrap();

    // Get snapshot AFTER commit
    let snapshot_after = SnapshotId::current();

    // Read with snapshot_before should NOT see uncommitted node
    let result_before = graph.get_node(snapshot_before, 1);
    assert!(result_before.is_err(), "snapshot_before should not see node inserted after it");

    // Read with snapshot_after should see committed data
    let result_after = graph.get_node(snapshot_after, 1);
    assert!(result_after.is_ok(), "snapshot_after should see committed data");
    assert_eq!(result_after.unwrap().name, "uncommitted");
}

/// **REGRESSION TEST**: Committed data visible after snapshot
///
/// # Expected Behavior
///
/// Verifies the basic snapshot isolation guarantee:
/// - Snapshots taken before a commit don't see that commit
/// - Snapshots taken after a commit do see that commit
#[test]
fn test_committed_data_visible_after_snapshot() {
    use sqlitegraph::{open_graph, GraphConfig, BackendKind, snapshot::SnapshotId};
    use tempfile::NamedTempFile;

    let temp = NamedTempFile::new().unwrap();
    let cfg = GraphConfig::native();
    let graph = open_graph(temp.path(), &cfg).unwrap();

    // Insert and commit
    let snapshot_before = SnapshotId::current();
    let node_id = graph.insert_node(NodeSpec {
        kind: "test".to_string(),
        name: "committed".to_string(),
        data: None,
    }).unwrap();
    let snapshot_after = SnapshotId::current();

    // Read with snapshot_before should NOT see the node
    let result_before = graph.get_node(snapshot_before, node_id);
    assert!(result_before.is_err(), "snapshot_before should not see node inserted after it");

    // Read with snapshot_after should see committed data
    let result_after = graph.get_node(snapshot_after, node_id);
    assert!(result_after.is_ok(), "snapshot_after should see committed data");
    assert_eq!(result_after.unwrap().name, "committed");
}
*/
