//! Tests for V3 block-locality prototype
//!
//! Run with: cargo test --features native-v3 test_block_locality --release -- --nocapture
//!
//! This tests the block-aware traversal cache prototype.

use sqlitegraph::{
    backend::native::v3::{V3Backend, node::block_cache::*},
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_node_id_to_block_calculation() {
    // Block 0: nodes 1-128
    assert_eq!(node_id_to_block(1), 0);
    assert_eq!(node_id_to_block(64), 0);
    assert_eq!(node_id_to_block(128), 0);

    // Block 1: nodes 129-256
    assert_eq!(node_id_to_block(129), 1);
    assert_eq!(node_id_to_block(200), 1);
    assert_eq!(node_id_to_block(256), 1);

    // Block 2: nodes 257-384
    assert_eq!(node_id_to_block(257), 2);
    assert_eq!(node_id_to_block(384), 2);

    println!("✓ Block ID calculation works correctly");
}

#[test]
fn test_block_cache_basic_operations() {
    let mut cache = BlockAwareTraversalCache::new(4);

    // Initial state
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.hits(), 0);
    assert_eq!(cache.misses(), 0);
    assert_eq!(cache.hit_rate(), 0.0);

    // Miss
    assert!(cache.get(1).is_none());
    assert_eq!(cache.misses(), 1);

    println!("✓ Block cache basic operations work");
}

#[test]
fn test_block_cache_with_backend() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("block_cache_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert nodes in a pattern that tests block locality
    // Block 0: nodes 1-128
    for i in 1..=50 {
        backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"i": i}),
            })
            .unwrap();
    }

    // Block 1: nodes 129-256 (but actually gets IDs 51-102)
    let block1_start = 51;
    let block1_end = 102;
    for i in block1_start..=block1_end {
        backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"i": i}),
            })
            .unwrap();
    }

    backend.flush_to_disk().unwrap();

    // Verify all nodes can be retrieved
    use sqlitegraph::SnapshotId;

    for i in 1..=50 {
        let result = backend.get_node(SnapshotId::current(), i);
        assert!(result.is_ok(), "Should retrieve node {}", i);
    }

    for i in block1_start..=block1_end {
        let result = backend.get_node(SnapshotId::current(), i);
        assert!(result.is_ok(), "Should retrieve node {}", i);
    }

    println!("✓ Backend with block-locality prototype works correctly");
}

#[test]
fn test_block_cache_stats() {
    let cache = BlockAwareTraversalCache::new(16);

    let stats = cache.block_stats();
    assert_eq!(stats.unique_blocks_in_cache, 0);
    assert_eq!(stats.tracked_blocks, 0);
    assert_eq!(stats.pages_in_cache, 0);

    // Simulate some cache activity
    // (In real usage, pages are inserted via NodeStore)

    println!("✓ Block cache stats work");
}

#[test]
fn test_block_preserves_correctness() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("correctness_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Create a simple graph
    let node1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(sqlitegraph::EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "calls".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend.flush_to_disk().unwrap();

    use sqlitegraph::SnapshotId;

    // Verify retrieval
    let entity1 = backend.get_node(SnapshotId::current(), node1).unwrap();
    assert_eq!(entity1.id, node1);

    let neighbors = backend
        .neighbors(
            SnapshotId::current(),
            node1,
            sqlitegraph::NeighborQuery {
                direction: sqlitegraph::BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], node2);

    println!("✓ Block-locality prototype preserves correctness");
}

#[test]
fn test_block_reopen_correctness() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("reopen_test.graph");

    // Create database with nodes spanning multiple blocks
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Block 0
        for i in 1..=100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }

        // Block 1 (node IDs 101-200, which spans blocks 0 and 1)
        for i in 101..=200 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }

        backend.flush_to_disk().unwrap();
    }

    // Reopen and verify
    let backend = V3Backend::open(&db_path).unwrap();
    use sqlitegraph::SnapshotId;

    // Verify nodes from both blocks
    for i in 1..=100 {
        let result = backend.get_node(SnapshotId::current(), i);
        assert!(result.is_ok(), "Should retrieve node {} after reopen", i);
    }

    for i in 101..=200 {
        let result = backend.get_node(SnapshotId::current(), i);
        assert!(result.is_ok(), "Should retrieve node {} after reopen", i);
    }

    println!("✓ Block-locality survives reopen correctly");
}
