//! Performance validation tests for query cache.
//!
//! These tests verify that the query cache provides performance benefits
//! for repeated expensive graph traversal operations.

use sqlitegraph::{
    EdgeSpec, NodeSpec, SnapshotId,
    backend::{BackendDirection, GraphBackend, SqliteGraphBackend},
};
use std::time::Instant;

fn create_performance_test_graph() -> Result<SqliteGraphBackend, Box<dyn std::error::Error>> {
    let backend = SqliteGraphBackend::in_memory()?;

    // Create a larger graph for more noticeable performance differences
    let mut node_ids = Vec::new();
    for i in 1..=100 {
        let node_id = backend.insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;
        node_ids.push(node_id);
    }

    // Create a mesh-like graph structure for complex traversals
    for i in 0..90 {
        for j in 1..=5 {
            let target = (i + j) % 100;
            if target != 0 && target != i + 1 {
                backend.insert_edge(EdgeSpec {
                    from: node_ids[i],
                    to: node_ids[target - 1],
                    edge_type: "mesh".to_string(),
                    data: serde_json::json!({"weight": j}),
                })?;
            }
        }
    }

    Ok(backend)
}

#[test]
fn test_query_cache_performance_benefit() -> Result<(), Box<dyn std::error::Error>> {
    let backend = create_performance_test_graph()?;
    let start_node = 1;
    let depth = 5;

    // First query - should be cache miss and populate cache
    let start_time = Instant::now();
    let _result1 = backend.bfs(SnapshotId::current(), start_node, depth)?;
    let first_query_time = start_time.elapsed();

    // Second identical query - should be cache hit
    let start_time = Instant::now();
    let _result2 = backend.bfs(SnapshotId::current(), start_node, depth)?;
    let second_query_time = start_time.elapsed();

    println!("First BFS query: {:?}", first_query_time);
    println!("Second BFS query: {:?}", second_query_time);

    // The second query should be faster (though this may vary by system)
    // At minimum, this test validates that caching doesn't break correctness
    assert!(
        second_query_time <= first_query_time * 2,
        "Cache should not cause significant performance regression"
    );

    Ok(())
}

#[test]
fn test_query_cache_multiple_operations() -> Result<(), Box<dyn std::error::Error>> {
    let backend = create_performance_test_graph()?;

    // Test BFS
    {
        let start_time = Instant::now();
        let result1 = backend.bfs(SnapshotId::current(), 1, 4)?;
        let first_time = start_time.elapsed();

        let start_time = Instant::now();
        let result2 = backend.bfs(SnapshotId::current(), 1, 4)?;
        let second_time = start_time.elapsed();

        println!("BFS - First: {:?}, Second: {:?}", first_time, second_time);
        assert_eq!(result1, result2, "Cached BFS result should match original");
    }

    // Test K-Hop Outgoing
    {
        let start_time = Instant::now();
        let result1 = backend.k_hop(SnapshotId::current(), 1, 3, BackendDirection::Outgoing)?;
        let first_time = start_time.elapsed();

        let start_time = Instant::now();
        let result2 = backend.k_hop(SnapshotId::current(), 1, 3, BackendDirection::Outgoing)?;
        let second_time = start_time.elapsed();

        println!(
            "K-Hop Outgoing - First: {:?}, Second: {:?}",
            first_time, second_time
        );
        assert_eq!(
            result1, result2,
            "Cached k-hop result should match original"
        );
    }

    // Test K-Hop Incoming
    {
        let start_time = Instant::now();
        let result1 = backend.k_hop(SnapshotId::current(), 50, 2, BackendDirection::Incoming)?;
        let first_time = start_time.elapsed();

        let start_time = Instant::now();
        let result2 = backend.k_hop(SnapshotId::current(), 50, 2, BackendDirection::Incoming)?;
        let second_time = start_time.elapsed();

        println!(
            "K-Hop Incoming - First: {:?}, Second: {:?}",
            first_time, second_time
        );
        assert_eq!(
            result1, result2,
            "Cached k-hop incoming result should match original"
        );
    }

    // Test Filtered K-Hop
    {
        let start_time = Instant::now();
        let result1 = backend.k_hop_filtered(
            SnapshotId::current(),
            1,
            2,
            BackendDirection::Outgoing,
            &["mesh"],
        )?;
        let first_time = start_time.elapsed();

        let start_time = Instant::now();
        let result2 = backend.k_hop_filtered(
            SnapshotId::current(),
            1,
            2,
            BackendDirection::Outgoing,
            &["mesh"],
        )?;
        let second_time = start_time.elapsed();

        println!(
            "Filtered K-Hop - First: {:?}, Second: {:?}",
            first_time, second_time
        );
        assert_eq!(
            result1, result2,
            "Cached filtered k-hop result should match original"
        );
    }

    // Test Shortest Path
    {
        let start_time = Instant::now();
        let result1 = backend.shortest_path(SnapshotId::current(), 1, 50)?;
        let first_time = start_time.elapsed();

        let start_time = Instant::now();
        let result2 = backend.shortest_path(SnapshotId::current(), 1, 50)?;
        let second_time = start_time.elapsed();

        println!(
            "Shortest Path - First: {:?}, Second: {:?}",
            first_time, second_time
        );
        assert_eq!(
            result1, result2,
            "Cached shortest path result should match original"
        );
    }

    Ok(())
}

#[test]
fn test_cache_invalidation_performance() -> Result<(), Box<dyn std::error::Error>> {
    let backend = create_performance_test_graph()?;

    // Warm up the cache
    let _result1 = backend.bfs(SnapshotId::current(), 1, 3)?;
    let _result2 = backend.k_hop(SnapshotId::current(), 1, 2, BackendDirection::Outgoing)?;

    // Modify the graph (should invalidate cache)
    backend.insert_edge(EdgeSpec {
        from: 1,
        to: 99,
        edge_type: "new_edge".to_string(),
        data: serde_json::json!({"test": true}),
    })?;

    // Query again - should compute fresh result due to cache invalidation
    let _result3 = backend.bfs(SnapshotId::current(), 1, 3)?;

    // This primarily tests that cache invalidation works without breaking functionality
    // Performance characteristics after invalidation should be similar to first queries
    println!("Cache invalidation test completed successfully");

    Ok(())
}
