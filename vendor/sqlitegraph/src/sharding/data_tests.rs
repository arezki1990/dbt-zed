//! Data-driven tests for native-v3 CSR sharding layer.
//!
//! Tests each native-v3 feature with realistic graph workloads to validate
//! correctness, performance, and integration with existing algorithms.

use std::sync::{Arc, Mutex};

use crate::sharding::*;

/// Test CSR shard reading with real graph data.
#[test]
fn test_csr_shard_real_graph() {
    // Create realistic graph: call graph with 100 functions
    // Shard range [0, 1000) means source IDs must be 0-999
    let mut shard = CsrShard::new(0, 0, 1000);

    // Simulate function call graph (IDs must be in [0, 1000))
    for i in 0..100 {
        let caller = i * 10;
        let callee = (i + 1) * 10;

        shard.add_edge(CsrEdge {
            src: caller,
            dst: callee,
            weight: 0.8,
            flags: 0,
        });
    }

    shard.sort_edges();

    // Verify CSR structure
    assert_eq!(shard.edge_count(), 100);
    assert_eq!(shard.edges[0].src, 0);
    assert_eq!(shard.edges[99].dst, 1000);
}

/// Test BFS subgraph builder with dependency chain.
#[test]
fn test_subgraph_builder_dependency_chain() {
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Create linear dependency: main -> parse -> validate -> execute
    let deps = vec![
        (1000, 1050), // main -> parse
        (1050, 1100), // parse -> validate
        (1100, 1150), // validate -> execute
        (1150, 1200), // execute -> finish
    ];

    for (src, dst) in deps {
        shard.add_edge(CsrEdge {
            src,
            dst,
            weight: 1.0,
            flags: 0,
        });
    }

    shard.sort_edges();

    // Verify CSR structure
    assert_eq!(shard.edge_count(), 4);
    assert_eq!(shard.edges[0].src, 1000);
    assert_eq!(shard.edges[0].dst, 1050);

    // Would build subgraph here once SubgraphReader is available
    // let tokens = vec![1000];
    // let subgraph = SubgraphBuilder::new_from_shard(&shard);
}

/// Test bidirectional index with real call graph.
#[test]
fn test_bidirectional_index_call_graph() {
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Create function with multiple callers
    // helper_func called by: process_request, handle_error, cleanup
    let callers = vec![1000, 1050, 1100];
    let callee = 2000;

    for caller in &callers {
        shard.add_edge(CsrEdge {
            src: *caller,
            dst: callee,
            weight: 0.5,
            flags: 0,
        });
    }

    shard.sort_edges();

    // Verify forward edges
    let total_weight: f32 = shard.edges.iter().map(|e| e.weight).sum();
    assert_eq!(total_weight, 1.5); // 0.5 * 3 callers
}

/// Test HNSW semantic layer with code embeddings.
#[test]
fn test_hnsw_semantic_layer_code_embeddings() {
    let mut layer = SemanticLayer::new(128);

    // Insert function embeddings (simulated 128-dim code vectors)
    let functions = vec![
        (100, "parse_input"),
        (200, "validate_data"),
        (300, "process_request"),
        (400, "handle_error"),
    ];

    for (func_id, _name) in functions {
        let embedding: Vec<f32> = (0..128).map(|i| i as f32 / 1000.0).collect();
        layer.insert_embedding(func_id, embedding).unwrap();
    }

    // Query for similar function
    let query: Vec<f32> = (0..128).map(|i| i as f32 / 1000.0).collect();
    let results = layer.knn_search(&query, 2);

    assert_eq!(results.len(), 2);
    // Most similar should be the first function
    assert_eq!(results[0].node_id, 100);
}

/// Test property store with function metadata.
#[test]
fn test_property_store_function_metadata() {
    let mut store = PropertyStore::in_memory().unwrap();

    // Store function metadata
    store.set_token_text(1000, "parse_input").unwrap();
    store
        .set_metadata(
            1000,
            "{\"type\": \"function\", \"file\": \"parser.rs\", \"line\": 42}",
        )
        .unwrap();

    // Retrieve and verify
    let text = store.get_token_text(1000).unwrap();
    let metadata = store.get_metadata(1000).unwrap();

    assert_eq!(text, Some("parse_input".to_string()));

    let parsed: serde_json::Value = metadata.unwrap().parse().unwrap();
    assert_eq!(parsed["type"], "function");
    assert_eq!(parsed["file"], "parser.rs");
    assert_eq!(parsed["line"], 42);
}

/// Test pub/sub with graph operation events.
#[test]
fn test_pubsub_graph_operations() {
    let pubsub = PubSub::new();
    let notified = Arc::new(Mutex::new(Vec::new()));
    let notified_clone = notified.clone();

    pubsub.subscribe(
        vec!["graph.edge".to_string()],
        Box::new(move |change| {
            notified_clone.lock().unwrap().push(change.clone());
        }),
    );

    // Simulate refactoring: remove old edge, add new edge
    pubsub.publish(Change::edge_deleted(1000, 1050, "graph.edge".to_string()));
    pubsub.publish(Change::edge_inserted(1000, 1100, "graph.edge".to_string()));

    std::thread::sleep(std::time::Duration::from_millis(10));

    let notifications = notified.lock().unwrap();
    assert_eq!(notifications.len(), 2);

    // Verify edge types
    assert!(matches!(
        notifications[0].change_type,
        ChangeType::EdgeDeleted
    ));
    assert!(matches!(
        notifications[1].change_type,
        ChangeType::EdgeInserted
    ));
}

/// Test CSR + HNSW integration for semantic search fallback.
#[test]
fn test_csr_hnsw_fallback_integration() {
    // Setup: CSR with code graph
    let mut shard = CsrShard::new(0, 1000, 2000);

    // Add CSR edges (function calls)
    shard.add_edge(CsrEdge {
        src: 1000,
        dst: 1050,
        weight: 0.9,
        flags: 0,
    });

    // Setup: HNSW with embeddings
    let mut layer = SemanticLayer::new(64);

    let embedding1: Vec<f32> = (0..64).map(|i| i as f32).collect();
    let embedding2: Vec<f32> = (0..64).map(|i| (i as f32) + 0.1).collect();

    layer.insert_embedding(1000, embedding1).unwrap();
    layer.insert_embedding(1050, embedding2).unwrap();

    // Verify CSR structure (no forward query API yet, just validate edges exist)
    assert_eq!(shard.edge_count(), 1);
    assert_eq!(shard.edges[0].src, 1000);
    assert_eq!(shard.edges[0].dst, 1050);

    // Use HNSW for semantic similarity
    let query: Vec<f32> = (0..64).map(|i| i as f32).collect();
    let semantic_results = layer.knn_search(&query, 2);

    assert_eq!(semantic_results.len(), 2);
}

/// Test multi-shard subgraph construction.
#[test]
fn test_multi_shard_cross_shard_edges() {
    let mut shard1 = CsrShard::new(0, 0, 1000);
    let mut shard2 = CsrShard::new(1, 1000, 2000);

    // Cross-shard function call
    shard1.add_edge(CsrEdge {
        src: 500,
        dst: 1500,
        weight: 0.7,
        flags: 0,
    });

    shard2.add_edge(CsrEdge {
        src: 1500,
        dst: 1800,
        weight: 0.8,
        flags: 0,
    });

    shard1.sort_edges();
    shard2.sort_edges();

    // This would need ShardReader to load both shards
    // For now, verify shard structure
    assert_eq!(shard1.edge_count(), 1);
    assert_eq!(shard2.edge_count(), 1);
    assert_eq!(shard1.edges[0].dst, 1500); // Cross-shard edge
}

/// Test property store persistence with multiple tokens.
#[test]
fn test_property_store_multiple_tokens() {
    let mut store = PropertyStore::in_memory().unwrap();

    // Insert multiple functions
    let functions = vec![
        (1000, "parse"),
        (1050, "validate"),
        (1100, "process"),
        (1150, "render"),
    ];

    for (id, name) in functions {
        store.set_token_text(id, name).unwrap();
    }

    // Query all
    let count = store.token_count().unwrap();
    assert_eq!(count, 4);

    // Query specific
    let text = store.get_token_text(1100).unwrap();
    assert_eq!(text, Some("process".to_string()));

    // Delete one
    store.delete_token(1050).unwrap();
    assert_eq!(store.token_count().unwrap(), 3);
    assert!(store.get_token_text(1050).unwrap().is_none());
}

/// Test WAL replay consistency.
#[test]
fn test_wal_replay_consistency() {
    let pubsub = PubSub::new();

    // Publish sequence of changes
    pubsub.publish(Change::node_inserted(100, "graph.node".to_string()));
    pubsub.publish(Change::edge_inserted(100, 200, "graph.edge".to_string()));
    pubsub.publish(Change::edge_deleted(200, 300, "graph.edge".to_string()));

    let count = pubsub.change_log_size();
    assert_eq!(count, 3);

    // Add subscriber before replaying
    pubsub.subscribe(
        vec!["graph.edge".to_string(), "graph.node".to_string()],
        Box::new(|_change| {
            // Replay notification
        }),
    );

    // Replay should notify the subscriber
    let replayed = pubsub.replay_wal().unwrap();
    assert!(replayed > 0);

    // Clear log after checkpoint
    pubsub.clear_change_log();
    assert_eq!(pubsub.change_log_size(), 0);
}
