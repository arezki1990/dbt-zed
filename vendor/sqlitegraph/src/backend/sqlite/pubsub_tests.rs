//! TDD Tests for SQLite Backend Pub/Sub
//!
//! These tests verify that the SQLite backend supports Pub/Sub event notifications
//! for graph operations (node/edge changes).

use crate::backend::sqlite::SqliteGraphBackend;
use crate::backend::{EdgeSpec, GraphBackend, NodeSpec, PubSubEvent, SubscriptionFilter};

/// Test that subscribe returns a valid subscriber ID and receiver
#[test]
fn test_sqlite_pubsub_subscribe() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    // Subscribe to all events
    let filter = SubscriptionFilter::all();
    let (sub_id, _receiver) = graph.subscribe(filter).unwrap();

    // Should get a valid subscriber ID
    assert!(sub_id > 0);

    // Unsubscribe
    let removed = graph.unsubscribe(sub_id).unwrap();
    assert!(removed);

    // Unsubscribe again should return false
    let removed = graph.unsubscribe(sub_id).unwrap();
    assert!(!removed);
}

/// Test that node creation emits an event
#[test]
fn test_sqlite_pubsub_node_created_event() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    // Subscribe to node changes
    let filter = SubscriptionFilter {
        node_changes: true,
        edge_changes: false,
        kv_changes: false,
        snapshot_commits: false,
    };
    let (sub_id, receiver) = graph.subscribe(filter).unwrap();

    // Create a node
    let node_id = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "test_node".to_string(),
            file_path: None,
            data: serde_json::json!({"key": "value"}),
        })
        .unwrap();

    // Should receive a NodeChanged event
    let event = receiver.recv_timeout(std::time::Duration::from_secs(1));
    assert!(event.is_ok(), "Should receive node changed event");

    match event.unwrap() {
        PubSubEvent::NodeChanged {
            node_id: event_node_id,
            ..
        } => {
            assert_eq!(event_node_id, node_id);
        }
        _ => panic!("Expected NodeChanged event"),
    }

    graph.unsubscribe(sub_id).unwrap();
}

/// Test that edge creation emits an event
#[test]
fn test_sqlite_pubsub_edge_created_event() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    // Create two nodes first
    let node1 = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node2 = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "node2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Subscribe to edge changes
    let filter = SubscriptionFilter {
        node_changes: false,
        edge_changes: true,
        kv_changes: false,
        snapshot_commits: false,
    };
    let (sub_id, receiver) = graph.subscribe(filter).unwrap();

    // Create an edge
    let edge_id = graph
        .insert_edge(EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Should receive an EdgeChanged event
    let event = receiver.recv_timeout(std::time::Duration::from_secs(1));
    assert!(event.is_ok(), "Should receive edge changed event");

    match event.unwrap() {
        PubSubEvent::EdgeChanged {
            edge_id: event_edge_id,
            ..
        } => {
            assert_eq!(event_edge_id, edge_id);
        }
        _ => panic!("Expected EdgeChanged event"),
    }

    graph.unsubscribe(sub_id).unwrap();
}

/// Test that no events are received for unsubscribed event types
#[test]
fn test_sqlite_pubsub_filtered_events() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    // Subscribe only to edge changes (not nodes)
    let filter = SubscriptionFilter {
        node_changes: false,
        edge_changes: true,
        kv_changes: false,
        snapshot_commits: false,
    };
    let (sub_id, receiver) = graph.subscribe(filter).unwrap();

    // Create a node - should NOT receive an event
    let _node_id = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Should NOT receive any event for node creation
    let event = receiver.recv_timeout(std::time::Duration::from_millis(100));
    assert!(
        event.is_err(),
        "Should not receive event for unsubscribed node changes"
    );

    graph.unsubscribe(sub_id).unwrap();
}

/// Test multiple subscribers receiving same events
#[test]
fn test_sqlite_pubsub_multiple_subscribers() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    // Create two subscribers
    let filter = SubscriptionFilter::all();
    let (sub1, recv1) = graph.subscribe(filter).unwrap();
    let (sub2, recv2) = graph.subscribe(filter).unwrap();

    // Create a node
    let node_id = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "shared_node".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Both should receive the event
    let event1 = recv1.recv_timeout(std::time::Duration::from_secs(1));
    let event2 = recv2.recv_timeout(std::time::Duration::from_secs(1));

    assert!(event1.is_ok(), "Subscriber 1 should receive event");
    assert!(event2.is_ok(), "Subscriber 2 should receive event");

    match event1.unwrap() {
        PubSubEvent::NodeChanged { node_id: id, .. } => assert_eq!(id, node_id),
        _ => panic!("Subscriber 1 expected NodeChanged"),
    }

    match event2.unwrap() {
        PubSubEvent::NodeChanged { node_id: id, .. } => assert_eq!(id, node_id),
        _ => panic!("Subscriber 2 expected NodeChanged"),
    }

    graph.unsubscribe(sub1).unwrap();
    graph.unsubscribe(sub2).unwrap();
}

/// Test that events are not received after unsubscribing
#[test]
fn test_sqlite_pubsub_no_events_after_unsubscribe() {
    let graph = SqliteGraphBackend::in_memory().unwrap();

    let filter = SubscriptionFilter::all();
    let (sub_id, receiver) = graph.subscribe(filter).unwrap();

    // Unsubscribe immediately
    graph.unsubscribe(sub_id).unwrap();

    // Create a node
    let _node_id = graph
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "late_node".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Should NOT receive any event
    let event = receiver.recv_timeout(std::time::Duration::from_millis(100));
    assert!(
        event.is_err(),
        "Should not receive event after unsubscribing"
    );
}
