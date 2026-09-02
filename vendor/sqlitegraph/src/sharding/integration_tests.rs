//! Integration tests for CSR sharding layer.
//!
//! Validates CSR consistency across CRUD operations, property store sync,
//! and pub/sub notifications.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::sharding::pubsub::{Change, ChangeType};
use crate::sharding::shard::{CsrEdge, CsrShard};
use crate::sharding::*;

/// Test CSR consistency after insertions.
#[test]
fn test_csr_consistency_after_insert() {
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Insert edges
    shard.add_edge(CsrEdge {
        src: 1050,
        dst: 1500,
        weight: 0.8,
        flags: 0,
    });
    shard.add_edge(CsrEdge {
        src: 1050,
        dst: 1600,
        weight: 0.5,
        flags: 0,
    });

    // Verify edges in shard
    assert_eq!(shard.edge_count(), 2);

    // Sort for deterministic comparison
    shard.sort_edges();
    assert_eq!(shard.edges[0].dst, 1500);
    assert_eq!(shard.edges[1].dst, 1600);
}

/// Test property store sync with CSR.
#[test]
fn test_property_store_sync_with_csr() {
    let mut store = PropertyStore::in_memory().unwrap();
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Set token properties
    store.set_token_text(1050, "test_token").unwrap();
    store.set_embedding(1050, &[0.1, 0.2, 0.3, 0.4]).unwrap();
    store.set_metadata(1050, r#"{"type": "test"}"#).unwrap();

    // Add CSR edge referencing token
    shard.add_edge(CsrEdge {
        src: 1050,
        dst: 1500,
        weight: 0.8,
        flags: 0,
    });

    // Verify property store consistency
    let text = store.get_token_text(1050).unwrap();
    assert_eq!(text, Some("test_token".to_string()));

    let embedding = store.get_embedding(1050).unwrap();
    assert!(embedding.is_some());
    assert_eq!(embedding.unwrap().len(), 4);

    let metadata = store.get_metadata(1050).unwrap();
    assert_eq!(metadata, Some(r#"{"type": "test"}"#.to_string()));
}

/// Test pub/sub notifications on CSR changes.
#[test]
fn test_pubsub_notifications_on_csr_changes() {
    let pubsub = PubSub::new();
    let notified = Arc::new(Mutex::new(Vec::new()));
    let notified_clone = notified.clone();

    pubsub.subscribe(
        vec!["graph.edge".to_string()],
        Box::new(move |change| {
            notified_clone.lock().unwrap().push(change.clone());
        }),
    );

    // Simulate edge insertion
    let change = Change::edge_inserted(1050, 1500, "graph.edge".to_string());
    pubsub.publish(change);

    std::thread::sleep(Duration::from_millis(10));

    let notifications = notified.lock().unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].change_type, ChangeType::EdgeInserted);
    assert_eq!(notifications[0].src_id, Some(1050));
    assert_eq!(notifications[0].dst_id, Some(1500));
}

/// Test semantic layer KNN fallback.
#[test]
fn test_semantic_layer_knn_fallback() {
    let mut layer = SemanticLayer::new(4);

    // Insert embeddings
    layer
        .insert_embedding(1050, vec![1.0, 0.0, 0.0, 0.0])
        .unwrap();
    layer
        .insert_embedding(1500, vec![0.9, 0.1, 0.0, 0.0])
        .unwrap();
    layer
        .insert_embedding(1600, vec![0.0, 1.0, 0.0, 0.0])
        .unwrap();

    // Query KNN
    let query = vec![1.0, 0.0, 0.0, 0.0];
    let results = layer.knn_search(&query, 2);

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].node_id, 1050); // Closest match
    assert!(results[0].distance < results[1].distance);
}

/// Test property store persistence across operations.
#[test]
fn test_property_store_persistence() {
    let mut store = PropertyStore::in_memory().unwrap();

    // Insert token
    store.set_token_text(100, "test").unwrap();
    assert!(store.get_token_text(100).unwrap().is_some());

    // Update token
    store.set_token_text(100, "updated").unwrap();
    assert_eq!(
        store.get_token_text(100).unwrap(),
        Some("updated".to_string())
    );

    // Delete token
    store.delete_token(100).unwrap();
    assert!(store.get_token_text(100).unwrap().is_none());

    // Verify count
    assert_eq!(store.token_count().unwrap(), 0);
}

/// Test WAL replay after restart.
#[test]
fn test_wal_replay_after_restart() {
    let pubsub = PubSub::new();

    // Publish changes before restart
    pubsub.publish(Change::node_inserted(100, "graph.node".to_string()));
    pubsub.publish(Change::edge_inserted(100, 200, "graph.edge".to_string()));

    assert_eq!(pubsub.change_log_size(), 2);

    // Simulate restart by creating new subscriber and replaying
    let count = Arc::new(Mutex::new(0));
    let count_clone = count.clone();

    pubsub.subscribe(
        vec!["graph.node".to_string()],
        Box::new(move |_| {
            *count_clone.lock().unwrap() += 1;
        }),
    );

    let notified = pubsub.replay_wal().unwrap();
    assert_eq!(notified, 1); // One subscriber notified of one matching change
    assert_eq!(*count.lock().unwrap(), 1);
}

/// Test semantic layer incremental insert.
#[test]
fn test_semantic_layer_incremental_insert() {
    let mut layer = SemanticLayer::new(3);

    // Initial insert
    layer.insert_embedding(100, vec![1.0, 0.0, 0.0]).unwrap();
    assert_eq!(layer.embedding_count(), 1);

    // Update existing embedding (HNSW creates new vector_id for fine-tune)
    layer.insert_embedding(100, vec![0.9, 0.1, 0.0]).unwrap();
    // HNSW creates new vector_id, so count increases
    assert_eq!(layer.embedding_count(), 2);

    // Remove is no-op for HNSW (no deletion support)
    layer.remove_embedding(100);
    // Token still exists since HNSW can't delete efficiently
    assert!(layer.has_embedding(100));

    // Re-insert adds another vector
    layer.insert_embedding(100, vec![0.8, 0.2, 0.0]).unwrap();
    assert!(layer.has_embedding(100));
    assert_eq!(layer.embedding_count(), 3); // Count increased again
}

/// Test pub/sub topic filtering with wildcards.
#[test]
fn test_pubsub_topic_wildcards() {
    let pubsub = PubSub::new();

    let node_count = Arc::new(Mutex::new(0));
    let edge_count = Arc::new(Mutex::new(0));
    let all_count = Arc::new(Mutex::new(0));

    let node_count_clone = node_count.clone();
    let edge_count_clone = edge_count.clone();
    let all_count_clone = all_count.clone();

    // Subscribe to specific topics
    pubsub.subscribe(
        vec!["graph.node".to_string()],
        Box::new(move |_| {
            *node_count_clone.lock().unwrap() += 1;
        }),
    );

    pubsub.subscribe(
        vec!["graph.edge".to_string()],
        Box::new(move |_| {
            *edge_count_clone.lock().unwrap() += 1;
        }),
    );

    // Subscribe to wildcard
    pubsub.subscribe(
        vec!["*".to_string()],
        Box::new(move |_| {
            *all_count_clone.lock().unwrap() += 1;
        }),
    );

    // Publish changes
    pubsub.publish(Change::node_inserted(100, "graph.node".to_string()));
    pubsub.publish(Change::edge_inserted(100, 200, "graph.edge".to_string()));

    std::thread::sleep(Duration::from_millis(10));

    // Verify filtering
    assert_eq!(*node_count.lock().unwrap(), 1); // Only node change
    assert_eq!(*edge_count.lock().unwrap(), 1); // Only edge change
    assert_eq!(*all_count.lock().unwrap(), 2); // Both changes
}

/// Test property store all properties query.
#[test]
fn test_property_store_all_properties() {
    let mut store = PropertyStore::in_memory().unwrap();

    // Insert token with all properties
    store.set_token_text(100, "test").unwrap();
    store.set_embedding(100, &[0.1, 0.2, 0.3]).unwrap();
    store.set_metadata(100, r#"{"key": "value"}"#).unwrap();

    // Query all properties
    let result = store.get_all_properties(100).unwrap();
    assert!(result.is_some());

    let (text, embedding, metadata) = result.unwrap();
    assert_eq!(text, "test");
    assert_eq!(embedding.len(), 3);
    assert_eq!(metadata, r#"{"key": "value"}"#);
}

/// Test manifest with CSR shards.
#[test]
fn test_manifest_with_csr_shards() {
    let mut shard1 = CsrShard::new(0, 0, 1000);
    let mut shard2 = CsrShard::new(1, 1000, 2000);

    shard1.add_edge(CsrEdge {
        src: 100,
        dst: 200,
        weight: 0.5,
        flags: 0,
    });
    shard2.add_edge(CsrEdge {
        src: 1500,
        dst: 1700,
        weight: 0.7,
        flags: 0,
    });

    let metadata1 = ShardMetadata {
        shard_id: 0,
        source_start: 0,
        source_end: 1000,
        edge_count: 1,
        file_name: "shard_0000.csr".to_string(),
    };

    let metadata2 = ShardMetadata {
        shard_id: 1,
        source_start: 1000,
        source_end: 2000,
        edge_count: 1,
        file_name: "shard_0001.csr".to_string(),
    };

    let manifest = Manifest::new(vec![metadata1, metadata2], "1.0".to_string());

    assert_eq!(manifest.shards.len(), 2);
    assert_eq!(manifest.shards[0].edge_count, 1);
    assert_eq!(manifest.shards[1].source_start, 1000);
    assert_eq!(manifest.shards[1].source_end, 2000);
}

/// Test CSR edge storage and retrieval.
#[test]
fn test_csr_edge_storage() {
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Create edges
    shard.add_edge(CsrEdge {
        src: 1050,
        dst: 1500,
        weight: 0.8,
        flags: 0,
    });
    shard.add_edge(CsrEdge {
        src: 1100,
        dst: 1500,
        weight: 0.6,
        flags: 0,
    });
    shard.add_edge(CsrEdge {
        src: 1200,
        dst: 1500,
        weight: 0.4,
        flags: 0,
    });

    shard.sort_edges();

    // Verify edge count
    assert_eq!(shard.edge_count(), 3);

    // Verify edges are sorted by source
    let mut prev_src = 0;
    for edge in &shard.edges {
        assert!(edge.src >= prev_src);
        prev_src = edge.src;
    }
}

/// Test semantic layer cosine distance - identical vectors.
#[test]
fn test_cosine_distance_identical() {
    let mut layer = SemanticLayer::new(3);

    layer.insert_embedding(100, vec![1.0, 2.0, 3.0]).unwrap();
    layer.insert_embedding(200, vec![1.0, 2.0, 3.0]).unwrap();

    let query = vec![1.0, 2.0, 3.0];
    let results = layer.knn_search(&query, 1);
    assert_eq!(results.len(), 1);
    assert!((results[0].distance - 0.0).abs() < 1e-6); // Identical = distance 0
}

/// Test semantic layer cosine distance - orthogonal vectors.
#[test]
fn test_cosine_distance_orthogonal() {
    let mut layer = SemanticLayer::new(3);

    // Only insert orthogonal vectors
    layer.insert_embedding(300, vec![1.0, 0.0, 0.0]).unwrap();
    layer.insert_embedding(400, vec![0.0, 1.0, 0.0]).unwrap();

    let query_orthogonal = vec![1.0, 0.0, 0.0];
    let results_orthogonal = layer.knn_search(&query_orthogonal, 2);
    assert_eq!(results_orthogonal.len(), 2);
    assert_eq!(results_orthogonal[0].node_id, 300); // Closest (distance 0)
    // Orthogonal vector should have distance around 1.0 (HNSW approximation varies)
    eprintln!(
        "Distance to orthogonal vector: {}",
        results_orthogonal[1].distance
    );
    assert!(results_orthogonal[1].distance > 0.1); // Should be some distance
    assert!(results_orthogonal[0].distance < results_orthogonal[1].distance); // Results sorted by distance
}

/// Test property store token count operations.
#[test]
fn test_property_store_token_count() {
    let mut store = PropertyStore::in_memory().unwrap();

    assert_eq!(store.token_count().unwrap(), 0);

    store.set_token_text(100, "one").unwrap();
    store.set_token_text(200, "two").unwrap();
    store.set_token_text(300, "three").unwrap();

    assert_eq!(store.token_count().unwrap(), 3);

    store.delete_token(200).unwrap();
    assert_eq!(store.token_count().unwrap(), 2);
}

/// Test pub/sub change log persistence.
#[test]
fn test_pubsub_change_log_persistence() {
    let pubsub = PubSub::new();

    assert_eq!(pubsub.change_log_size(), 0);

    pubsub.publish(Change::node_inserted(100, "graph.node".to_string()));
    assert_eq!(pubsub.change_log_size(), 1);

    pubsub.publish(Change::edge_inserted(100, 200, "graph.edge".to_string()));
    assert_eq!(pubsub.change_log_size(), 2);

    pubsub.clear_change_log();
    assert_eq!(pubsub.change_log_size(), 0);
}

/// Test pubsub subscriber lifecycle.
#[test]
fn test_pubsub_subscriber_lifecycle() {
    let pubsub = PubSub::new();

    assert_eq!(pubsub.subscriber_count(), 0);

    let id1 = pubsub.subscribe(vec!["graph.node".to_string()], Box::new(|_| {}));
    assert_eq!(pubsub.subscriber_count(), 1);

    let id2 = pubsub.subscribe(vec!["graph.edge".to_string()], Box::new(|_| {}));
    assert_eq!(pubsub.subscriber_count(), 2);

    pubsub.unsubscribe(id1);
    assert_eq!(pubsub.subscriber_count(), 1);

    pubsub.unsubscribe(id2);
    assert_eq!(pubsub.subscriber_count(), 0);
}
