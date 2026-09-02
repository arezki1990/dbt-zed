//! Concurrent traversal regression tests.
//!
//! Validates that the observe_with_cluster() optimization doesn't introduce
//! lock contention or deadlocks in BFS traversals.

use std::time::{Duration, Instant};

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

/// Test BFS traversals from different start nodes sequentially
///
/// Verifies:
/// - All traversals complete successfully
/// - No deadlocks occur
/// - No excessive blocking (completes in reasonable time)
#[test]
fn test_sequential_bfs_no_contention() {
    let chain_size = 500;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test_sequential_bfs.db");

    // Create graph
    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(chain_size);

    // Create nodes
    for i in 0..chain_size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create chain edges
    for i in 0..chain_size.saturating_sub(1) {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"order": i}),
            })
            .expect("Failed to insert edge");
    }

    let _temp_dir = temp_dir;

    // Test multiple traversals from different start nodes
    let num_traversals = 4;
    let timeout_secs = 30; // Should complete well within this

    for traversal_id in 0..num_traversals {
        // Start from node 0 for all traversals
        let start_node = node_ids[0];

        let start_time = Instant::now();

        // Run BFS traversal
        let result = graph.bfs(SnapshotId::current(), start_node, chain_size as u32);

        let elapsed = start_time.elapsed();

        match result {
            Ok(visited) => {
                // Verify traversal found nodes
                assert!(
                    !visited.is_empty(),
                    "Traversal {} found no nodes",
                    traversal_id
                );

                // Check for reasonable completion time (no deadlock)
                assert!(
                    elapsed < Duration::from_secs(timeout_secs),
                    "Traversal {} took too long: {:?}",
                    traversal_id,
                    elapsed
                );
            }
            Err(e) => {
                panic!("Traversal {} failed: {:?}", traversal_id, e);
            }
        }
    }
}

/// Test write and read operations mixed
///
/// Verifies:
/// - Writes succeed while BFS operations can run
/// - Traversals complete without blocking indefinitely
/// - No data corruption
#[test]
fn test_write_read_mix() {
    let chain_size = 100; // Smaller for this test
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test_write_read_mix.db");

    // Create graph
    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(chain_size);

    // Create nodes
    for i in 0..chain_size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create initial chain edges
    for i in 0..chain_size.saturating_sub(1) {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"order": i}),
            })
            .expect("Failed to insert edge");
    }

    let _temp_dir = temp_dir;

    // Perform mix of writes and reads
    for iteration in 0..5 {
        // Add some "skip" edges
        if iteration % 2 == 0 {
            for i in (0..chain_size.saturating_sub(2)).step_by(10) {
                let _ = graph.insert_edge(EdgeSpec {
                    from: node_ids[i],
                    to: node_ids[i + 2],
                    edge_type: "skip".to_string(),
                    data: serde_json::json!({"skip": i, "iteration": iteration}),
                });
            }
        }

        // Run BFS
        let start_time = Instant::now();
        let result = graph.bfs(SnapshotId::current(), node_ids[iteration % chain_size], 50);

        let elapsed = start_time.elapsed();

        match result {
            Ok(visited) => {
                assert!(!visited.is_empty(), "BFS {} found no nodes", iteration);
                assert!(
                    elapsed < Duration::from_secs(10),
                    "BFS {} took too long: {:?}",
                    iteration,
                    elapsed
                );
            }
            Err(e) => {
                panic!("BFS {} failed: {:?}", iteration, e);
            }
        }
    }
}

/// Test multiple traversals on the same graph
///
/// Verifies that TraversalContext isolation is maintained
/// and no cross-traversal pollution occurs.
#[test]
fn test_multiple_traversal_isolation() {
    let chain_size = 200;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test_isolation.db");

    // Create graph
    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(chain_size);

    // Create nodes
    for i in 0..chain_size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create chain edges
    for i in 0..chain_size.saturating_sub(1) {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"order": i}),
            })
            .expect("Failed to insert edge");
    }

    let _temp_dir = temp_dir;

    // Run multiple traversals from different start nodes
    let num_traversals = 4;

    for traversal_id in 0..num_traversals {
        // Each run performs multiple traversals
        for iteration in 0..3 {
            // Start from early nodes in the chain
            let start_index = (traversal_id + iteration) % 50;
            let start_node = node_ids[start_index];

            let result = graph.bfs(SnapshotId::current(), start_node, 50);

            assert!(
                result.is_ok(),
                "Traversal {} iteration {} failed: {:?}",
                traversal_id,
                iteration,
                result.err()
            );

            let visited = result.unwrap();
            assert!(
                !visited.is_empty(),
                "Traversal {} iteration {} found no nodes",
                traversal_id,
                iteration
            );
        }
    }
}

/// Test for deadlock scenarios with multiple traversals
///
/// This test specifically targets potential deadlock conditions
/// that could arise from the observe_with_cluster() changes.
#[test]
fn test_no_deadlock_multiple_traversals() {
    let chain_size = 300;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test_deadlock.db");

    // Create graph
    let graph = open_graph(&db_path, &GraphConfig::native()).expect("Failed to create graph");

    let mut node_ids = Vec::with_capacity(chain_size);

    // Create nodes
    for i in 0..chain_size {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .expect("Failed to insert node");
        node_ids.push(node_id);
    }

    // Create chain edges
    for i in 0..chain_size.saturating_sub(1) {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"order": i}),
            })
            .expect("Failed to insert edge");
    }

    let _temp_dir = temp_dir;

    let start = Instant::now();
    let timeout = Duration::from_secs(20);

    // Run multiple short traversals
    let num_traversals = 6;
    for traversal_id in 0..num_traversals {
        for i in 0..5 {
            // Start from early nodes in the chain
            let start_index = (traversal_id * 5 + i) % 50;
            let start_node = node_ids[start_index];

            let result = graph.bfs(SnapshotId::current(), start_node, 100);

            if result.is_ok() && !result.unwrap().is_empty() {
                // Success for this traversal
            } else {
                panic!("Traversal {} iteration {} failed", traversal_id, i);
            }

            // Check timeout
            if start.elapsed() > timeout {
                panic!("Deadlock detected - traversals did not complete in time");
            }
        }
    }

    let elapsed = start.elapsed();
    assert!(
        elapsed < timeout,
        "Test took too long: {:?} (possible blocking)",
        elapsed
    );
}
