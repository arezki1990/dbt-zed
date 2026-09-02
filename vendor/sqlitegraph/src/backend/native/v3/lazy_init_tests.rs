//! TDD Tests for Lazy KV and Pub/Sub Initialization in V3Backend
//!
//! These tests verify that:
//! 1. KV store is not initialized until first use
//! 2. Publisher is not initialized until first use
//! 3. Graph operations work without KV/PubSub
//! 4. Lazy initialization works correctly

use crate::backend::native::v3::{KvValue, V3Backend};
use crate::backend::{EdgeSpec, GraphBackend, NodeSpec, SubscriptionFilter};
use crate::snapshot::SnapshotId;
use tempfile::TempDir;

/// Test that graph operations work without KV/PubSub initialization
#[test]
fn test_graph_ops_without_kv_or_pubsub() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert nodes - pure graph operation
    let node1 = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "main.rs".to_string(),
            file_path: Some("src/main.rs".to_string()),
            data: serde_json::json!({"lines": 100}),
        })
        .unwrap();

    let node2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "main".to_string(),
            file_path: None,
            data: serde_json::json!({"public": true}),
        })
        .unwrap();

    // Insert edge - pure graph operation
    backend
        .insert_edge(EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "contains".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Query operations
    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), 2);

    let outgoing = backend.fetch_outgoing(node1).unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0], node2);

    let incoming = backend.fetch_incoming(node2).unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0], node1);
}

/// Test that KV store is NOT initialized on read operations (lazy write)
#[test]
fn test_kv_not_initialized_on_get() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Before any KV operation, kv_store should not be initialized
    assert!(
        !backend.is_kv_initialized(),
        "KV store should not be initialized on creation"
    );

    // First KV get should NOT initialize it (reads return None for uninitialized)
    let snapshot_id = SnapshotId::current();
    let result = backend.kv_get_v3(snapshot_id, b"test_key");
    assert!(result.is_none(), "Should return None for non-existent key");

    // KV should still NOT be initialized (only writes initialize)
    assert!(
        !backend.is_kv_initialized(),
        "KV store should NOT be initialized after read-only get"
    );
}

/// Test that KV store is lazily initialized on first kv_set
#[test]
fn test_kv_lazy_init_on_set() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Before any KV operation
    assert!(
        !backend.is_kv_initialized(),
        "KV store should not be initialized on creation"
    );

    // First KV set should initialize it
    backend.kv_set_v3(
        b"my_key".to_vec(),
        KvValue::String("value".to_string()),
        None,
    );

    // Now KV should be initialized
    assert!(
        backend.is_kv_initialized(),
        "KV store should be initialized after first set"
    );
}

/// Test that Publisher is lazily initialized on first subscribe
#[test]
fn test_pubsub_lazy_init_on_subscribe() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Before any PubSub operation
    assert!(
        !backend.is_pubsub_initialized(),
        "Publisher should not be initialized on creation"
    );

    // First subscribe should initialize it
    let filter = SubscriptionFilter::all();
    let (sub_id, _receiver) = backend.subscribe(filter).unwrap();

    // Now PubSub should be initialized
    assert!(
        backend.is_pubsub_initialized(),
        "Publisher should be initialized after first subscribe"
    );

    // Cleanup
    backend.unsubscribe(sub_id).unwrap();
}

/// Test that KV operations work correctly after lazy initialization
#[test]
fn test_kv_operations_after_lazy_init() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let snapshot_id = SnapshotId::current();

    // Key shouldn't exist initially
    let value = backend.kv_get_v3(snapshot_id, b"counter");
    assert!(value.is_none());

    // Set a value
    backend.kv_set_v3(b"counter".to_vec(), KvValue::Integer(42), None);

    // Read it back
    let value = backend.kv_get_v3(snapshot_id, b"counter");
    assert!(value.is_some());
    match value.unwrap() {
        KvValue::Integer(v) => assert_eq!(v, 42),
        _ => panic!("Expected integer value"),
    }

    // Delete it
    backend.kv_delete_v3(b"counter");

    // Should be gone
    let value = backend.kv_get_v3(snapshot_id, b"counter");
    assert!(value.is_none());
}

/// Test that PubSub operations work correctly after lazy initialization
#[test]
fn test_pubsub_operations_after_lazy_init() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Subscribe
    let filter = SubscriptionFilter::all();
    let (sub_id, _receiver) = backend.subscribe(filter).unwrap();

    // Unsubscribe
    let removed = backend.unsubscribe(sub_id).unwrap();
    assert!(removed);

    // Unsubscribe again should return false
    let removed = backend.unsubscribe(sub_id).unwrap();
    assert!(!removed);
}

/// Test that multiple graph operations don't trigger KV/PubSub initialization
#[test]
fn test_multiple_graph_ops_no_kv_pubsub_init() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Perform many graph operations
    for i in 0..10 {
        let node = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        if i > 0 {
            backend
                .insert_edge(EdgeSpec {
                    from: (i as i64),
                    to: node,
                    edge_type: "links".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }

    // Verify no KV or PubSub was initialized
    assert!(
        !backend.is_kv_initialized(),
        "KV should not be initialized after graph ops"
    );
    assert!(
        !backend.is_pubsub_initialized(),
        "PubSub should not be initialized after graph ops"
    );
}

/// Test that interleaved graph and KV operations work correctly
#[test]
fn test_interleaved_graph_and_kv_ops() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let snapshot_id = SnapshotId::current();

    // Insert some nodes
    let node1 = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "a.rs".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Set some KV data
    backend.kv_set_v3(
        b"metadata".to_vec(),
        KvValue::Json(serde_json::json!({"author": "test"})),
        None,
    );

    // More graph operations
    let node2 = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "b.rs".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "depends_on".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Read KV back
    let value = backend.kv_get_v3(snapshot_id, b"metadata");
    assert!(value.is_some());

    // Verify graph structure
    let outgoing = backend.fetch_outgoing(node1).unwrap();
    assert_eq!(outgoing.len(), 1);
}
