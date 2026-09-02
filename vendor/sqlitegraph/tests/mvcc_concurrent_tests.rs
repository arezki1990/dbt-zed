//! MVCC Concurrent Tests - Multi-threaded stress testing
//!
//! **Purpose:** Validate MVCC-lite system under concurrent access
//! **Scope:** Multi-threaded tests of thread-safe components
//! **Focus:** SnapshotManager thread safety, stress testing, performance validation
//!
//! **NOTE:** SqliteGraph itself is NOT thread-safe (contains RefCell, non-sync types).
//! These tests focus on the thread-safe SnapshotManager component only.
//!
//! These tests verify that the snapshot system's thread-safe components provide
//! proper isolation under concurrent access patterns.

use sqlitegraph::algo;
use sqlitegraph::mvcc::SnapshotManager;
use sqlitegraph::{GraphEdgeCreate, GraphEntityCreate, SqliteGraph, SqliteGraphError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

//
// TEST HELPERS
//

/// Helper: Get node count from graph
fn node_count(graph: &SqliteGraph) -> Result<i64, SqliteGraphError> {
    let ids = graph.list_entity_ids()?;
    Ok(ids.len() as i64)
}

/// Helper: Warm the cache by reading all adjacency data
fn warm_cache(graph: &SqliteGraph) -> Result<(), SqliteGraphError> {
    let entity_ids = graph.list_entity_ids()?;
    for &id in &entity_ids {
        let _ = graph.query().outgoing(id);
        let _ = graph.query().incoming(id);
    }
    Ok(())
}

/// Helper: Insert entity using proper API
fn insert_entity(graph: &SqliteGraph, create: GraphEntityCreate) -> Result<i64, SqliteGraphError> {
    let entity = sqlitegraph::GraphEntity {
        id: 0, // Will be assigned by database
        kind: create.kind,
        name: create.name,
        file_path: create.file_path,
        data: create.data,
    };
    graph.insert_entity(&entity)
}

/// Helper: Insert edge using proper API
fn insert_edge(graph: &SqliteGraph, create: GraphEdgeCreate) -> Result<i64, SqliteGraphError> {
    let edge = sqlitegraph::GraphEdge {
        id: 0, // Will be assigned by database
        from_id: create.from_id,
        to_id: create.to_id,
        edge_type: create.edge_type,
        data: create.data,
    };
    graph.insert_edge(&edge)
}

/// Helper: Create test graph with sample data
fn create_test_graph() -> Result<SqliteGraph, SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create test entities
    let entity1 = GraphEntityCreate {
        kind: "function".to_string(),
        name: "main".to_string(),
        file_path: Some("src/main.rs".to_string()),
        data: serde_json::json!({"line": 10}),
    };

    let entity2 = GraphEntityCreate {
        kind: "function".to_string(),
        name: "helper".to_string(),
        file_path: Some("src/helper.rs".to_string()),
        data: serde_json::json!({"line": 5}),
    };

    let id1 = insert_entity(&graph, entity1)?;
    let id2 = insert_entity(&graph, entity2)?;

    // Create edge
    let edge = GraphEdgeCreate {
        from_id: id1,
        to_id: id2,
        edge_type: "calls".to_string(),
        data: serde_json::json!({}),
    };
    insert_edge(&graph, edge)?;

    Ok(graph)
}

/// Helper: Create larger test graph for stress tests
fn create_large_test_graph(size: usize) -> Result<SqliteGraph, SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create entities
    for i in 0..size {
        let entity = GraphEntityCreate {
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("file_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        insert_entity(&graph, entity)?;
    }

    // Create edges (each node connects to next 2 nodes)
    let ids: Vec<i64> = graph.list_entity_ids()?;
    for (i, &id) in ids.iter().enumerate() {
        for j in 1..=2 {
            let target_idx = (i + j) % ids.len();
            let edge = GraphEdgeCreate {
                from_id: id,
                to_id: ids[target_idx],
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            };
            insert_edge(&graph, edge)?;
        }
    }

    Ok(graph)
}

/// Helper: Create a SnapshotManager with test data
fn create_test_snapshot_manager(size: usize) -> SnapshotManager {
    let mut outgoing = HashMap::new();
    let mut incoming = HashMap::new();

    for i in 0..size {
        outgoing.insert(i as i64, vec![]);
        incoming.insert(i as i64, vec![]);
    }

    SnapshotManager::with_state(&outgoing, &incoming)
}

//
// GROUP 1: SNAPSHOT MANAGER CONCURRENCY TESTS
//

#[test]
fn test_concurrent_snapshot_acquisition() {
    // Scenario: 100 threads simultaneously acquire snapshots
    // Expected: All threads succeed, no deadlocks, all snapshots valid
    let manager = Arc::new(create_test_snapshot_manager(100));
    let barrier = Arc::new(Barrier::new(100));
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let success_count = success_count.clone();

            thread::spawn(move || {
                barrier.wait();
                let snapshot = manager.acquire_snapshot();
                if snapshot.node_count() > 0 {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    // Wait for all threads
    for h in handles {
        h.join().expect("Thread panicked");
    }

    // Verify all threads succeeded
    let success = success_count.load(Ordering::Relaxed);
    assert_eq!(success, 100, "Not all threads acquired valid snapshots");
}

#[test]
fn test_snapshot_during_state_update() {
    // Scenario: Concurrent snapshot acquisition during state updates
    // Expected: No torn reads, all snapshots consistent
    let manager = Arc::new(create_test_snapshot_manager(100));

    let mut outgoing = HashMap::new();
    let mut incoming = HashMap::new();

    // Initialize with some data
    for i in 0..100 {
        outgoing.insert(i, vec![]);
        incoming.insert(i, vec![]);
    }

    let manager1 = manager.clone();
    let manager2 = manager.clone();

    // Thread A: Update state continuously
    let handle1 = thread::spawn(move || {
        for i in 0..1000 {
            let mut new_outgoing = outgoing.clone();
            new_outgoing.insert(i % 100, vec![]);
            manager1.update_snapshot(&new_outgoing, &incoming);
        }
    });

    // Thread B: Acquire snapshots continuously
    let handle2 = thread::spawn(move || {
        for _ in 0..1000 {
            let snapshot = manager2.acquire_snapshot();
            // Verify snapshot is valid
            assert!(snapshot.node_count() <= 100);
        }
    });

    handle1.join().expect("Thread A panicked");
    handle2.join().expect("Thread B panicked");
}

#[test]
fn test_rapid_snapshot_creation() {
    // Scenario: Create 1000 snapshots in rapid succession
    // Expected: No memory leaks, all snapshots valid
    let manager = create_test_snapshot_manager(50);

    // Create many snapshots
    for _ in 0..1000 {
        let snapshot = manager.acquire_snapshot();
        assert!(snapshot.node_count() > 0, "Snapshot has no nodes");
    }

    // Verify final snapshot is still valid
    let final_snapshot = manager.acquire_snapshot();
    assert!(final_snapshot.node_count() > 0);
}

#[test]
fn test_100_simultaneous_snapshots() {
    // Scenario: 100 threads acquire snapshots simultaneously with barrier
    // Expected: All succeed, no contention issues
    let manager = Arc::new(create_test_snapshot_manager(100));
    let barrier = Arc::new(Barrier::new(100));
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let success_count = success_count.clone();

            thread::spawn(move || {
                barrier.wait();
                let snapshot = manager.acquire_snapshot();
                let count = snapshot.node_count();

                if count > 0 && count <= 100 {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("Thread panicked");
    }

    let success = success_count.load(Ordering::Relaxed);
    assert_eq!(success, 100, "Not all threads succeeded");
}

#[test]
fn test_sustained_concurrent_access() {
    // Scenario: Sustained concurrent access for 2 seconds
    // Expected: No deadlocks, continuous progress
    let manager = Arc::new(create_test_snapshot_manager(50));
    let running = Arc::new(AtomicU64::new(1));
    let duration = Duration::from_secs(2);

    // Spawn 10 threads
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let manager = manager.clone();
            let running = running.clone();
            let start = Instant::now();

            thread::spawn(move || {
                let mut count = 0;
                while running.load(Ordering::Relaxed) == 1 && start.elapsed() < duration {
                    let _ = manager.acquire_snapshot();
                    count += 1;
                }
                count
            })
        })
        .collect();

    // Let them run for 2 seconds
    thread::sleep(duration);
    running.store(0, Ordering::Relaxed);

    // Collect results
    let mut total = 0;
    for h in handles {
        let count = h.join().expect("Thread panicked");
        total += count;
    }

    // Verify reasonable throughput (at least 1000 snapshots total)
    assert!(
        total >= 1000,
        "Low throughput: {} snapshots in 2 seconds",
        total
    );
    println!(
        "Sustained concurrent access: {} snapshots in 2 seconds",
        total
    );
}

//
// GROUP 2: CORRECTNESS TESTS
//

#[test]
fn test_snapshot_state_immutability() {
    // Scenario: Verify SnapshotState is truly immutable
    // Expected: Snapshots never change after creation
    let manager = Arc::new(create_test_snapshot_manager(50));

    let snapshot1 = manager.acquire_snapshot();
    let original_count = snapshot1.node_count();

    // Update state
    let mut new_outgoing = HashMap::new();
    for i in 0..100 {
        new_outgoing.insert(i, vec![]);
    }
    manager.update_snapshot(&new_outgoing, &new_outgoing);

    // Original snapshot should be unchanged
    assert_eq!(snapshot1.node_count(), original_count);

    // New snapshot should reflect changes
    let snapshot2 = manager.acquire_snapshot();
    assert_eq!(snapshot2.node_count(), 100);
}

#[test]
fn test_arc_swap_atomic_guarantees() {
    // Scenario: Verify ArcSwap provides atomic pointer swaps
    // Expected: No torn reads, consistent state
    let manager = Arc::new(create_test_snapshot_manager(50));

    let mut state1 = HashMap::new();
    for i in 0..50 {
        state1.insert(i, vec![]);
    }

    let mut state2 = HashMap::new();
    for i in 0..100 {
        state2.insert(i, vec![]);
    }

    let manager1 = manager.clone();
    let manager2 = manager.clone();

    // Thread A: Rapid state updates
    let handle_a = thread::spawn(move || {
        for _ in 0..1000 {
            manager1.update_snapshot(&state1, &state1);
            manager1.update_snapshot(&state2, &state2);
        }
    });

    // Thread B: Rapid snapshot acquisition
    let handle_b = thread::spawn(move || {
        for _ in 0..1000 {
            let snapshot = manager2.acquire_snapshot();

            // Verify snapshot is consistent (either 50 or 100 nodes)
            let count = snapshot.node_count();
            assert!(
                count == 50 || count == 100,
                "Inconsistent snapshot state: {} nodes",
                count
            );
        }
    });

    handle_a.join().expect("Thread A panicked");
    handle_b.join().expect("Thread B panicked");
}

#[test]
fn test_concurrent_snapshot_ordering() {
    // Scenario: Multiple concurrent snapshots should see consistent ordering
    // Expected: Snapshots maintain happens-before relationship (or same timestamp if very fast)
    let manager = create_test_snapshot_manager(50);

    // Create snapshot 1
    let snapshot1 = manager.acquire_snapshot();
    let time1 = snapshot1.created_at;

    // Small delay to ensure different timestamp
    thread::sleep(Duration::from_millis(20));

    // Create snapshot 2
    let snapshot2 = manager.acquire_snapshot();
    let time2 = snapshot2.created_at;

    // Verify ordering (or equal if system is very fast)
    assert!(
        time2 >= time1,
        "Snapshot ordering violated: {:?} >= {:?}",
        time2,
        time1
    );
}

#[test]
fn test_snapshot_isolation_with_clones() {
    // Scenario: Multiple clones of same snapshot
    // Expected: All clones see same state
    let manager = create_test_snapshot_manager(50);

    let snapshot1 = manager.acquire_snapshot();
    let snapshot2 = Arc::clone(&snapshot1);

    assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    assert_eq!(snapshot1.created_at, snapshot2.created_at);
}

//
// GROUP 3: MEMORY AND PERFORMANCE
//

#[test]
fn test_memory_no_leaks() {
    // Scenario: Create and drop many snapshots
    // Expected: No memory leaks
    let manager = create_test_snapshot_manager(50);

    // Create many snapshots in a loop
    for _ in 0..10_000 {
        let _snapshot = manager.acquire_snapshot();
        // Snapshot dropped here
    }

    // Final snapshot should still work
    let final_snapshot = manager.acquire_snapshot();
    assert!(final_snapshot.node_count() > 0);
}

#[test]
fn test_snapshot_clone_performance() {
    // Scenario: Clone Arc<SnapshotState> many times
    // Expected: Cloning is cheap (just atomic refcount increment)
    let manager = create_test_snapshot_manager(50);

    let snapshot = manager.acquire_snapshot();

    let start = Instant::now();

    // Clone 1000 times
    for _ in 0..1000 {
        let _ = Arc::clone(&snapshot);
    }

    let elapsed = start.elapsed();

    // Should be very fast (< 10ms)
    assert!(
        elapsed < Duration::from_millis(10),
        "Arc::clone too slow: {:?}",
        elapsed
    );

    println!("1000 Arc::clone operations in {:?}", elapsed);
}

#[test]
fn test_high_contention_snapshot_acquisition() {
    // Scenario: High contention with many threads
    // Expected: Still performs reasonably
    let manager = Arc::new(create_test_snapshot_manager(50));
    let barrier = Arc::new(Barrier::new(50));
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..50)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let success_count = success_count.clone();

            thread::spawn(move || {
                barrier.wait();

                // Each thread acquires 100 snapshots
                for _ in 0..100 {
                    let snapshot = manager.acquire_snapshot();
                    if snapshot.node_count() > 0 {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("Thread panicked");
    }

    // Expected: 50 threads * 100 snapshots = 5000 successful acquisitions
    let success = success_count.load(Ordering::Relaxed);
    assert_eq!(
        success, 5000,
        "Expected all snapshot acquisitions to succeed"
    );
}

#[test]
fn test_snapshot_independence() {
    // Scenario: Multiple independent snapshots
    // Expected: Each snapshot is independent
    let manager = create_test_snapshot_manager(50);

    let snapshot1 = manager.acquire_snapshot();
    let snapshot2 = manager.acquire_snapshot();
    let snapshot3 = manager.acquire_snapshot();

    // All should have same state
    assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    assert_eq!(snapshot2.node_count(), snapshot3.node_count());

    // Update state
    let mut new_outgoing = HashMap::new();
    for i in 0..100 {
        new_outgoing.insert(i, vec![]);
    }
    manager.update_snapshot(&new_outgoing, &new_outgoing);

    // Old snapshots unchanged
    assert_eq!(snapshot1.node_count(), 50);
    assert_eq!(snapshot2.node_count(), 50);
    assert_eq!(snapshot3.node_count(), 50);

    // New snapshot sees changes
    let snapshot4 = manager.acquire_snapshot();
    assert_eq!(snapshot4.node_count(), 100);
}

//
// GROUP 4: CONCURRENT ALGORITHM EXECUTION
//
// NOTE: SqliteGraph is NOT thread-safe (contains RefCell, non-Sync types).
// These tests focus on thread-safe SnapshotManager and verify algorithms
// work correctly with snapshots, not concurrent graph access.
//

#[test]
fn test_concurrent_snapshot_creation_with_algorithms() {
    // Scenario: Multiple threads create snapshots concurrently
    // Expected: All threads succeed, all snapshots valid
    let manager = Arc::new(create_test_snapshot_manager(100));
    let barrier = Arc::new(Barrier::new(10));
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let success_count = success_count.clone();

            thread::spawn(move || {
                barrier.wait();

                // Each thread creates a snapshot
                let snapshot = manager.acquire_snapshot();

                if snapshot.node_count() > 0 {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("Thread panicked");
    }

    let success = success_count.load(Ordering::Relaxed);
    assert_eq!(success, 10, "Not all threads acquired valid snapshots");
}

#[test]
fn test_snapshot_state_with_algorithm_preparation() {
    // Scenario: Verify snapshot state is suitable for algorithm execution
    // Expected: Snapshot has all required data for algorithms
    let graph = create_test_graph().expect("Failed to create test graph");
    warm_cache(&graph).expect("Failed to warm cache");

    // Acquire snapshot
    let snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");

    // Verify snapshot has nodes
    assert!(snapshot.node_count() > 0, "Snapshot should have nodes");

    // Verify snapshot data structure is consistent
    let entity_ids = graph.list_entity_ids().expect("Failed to get entity IDs");
    for &id in &entity_ids {
        assert!(
            snapshot.contains_node(id),
            "Snapshot should contain node {}",
            id
        );
    }

    // Verify algorithms can run on the graph (not snapshot directly)
    let components = algo::connected_components(&graph);
    assert!(components.is_ok(), "Algorithm should run on graph");
}

//
// GROUP 5: ALGORITHM CONSISTENCY
//

#[test]
fn test_algorithm_determinism_multiple_runs() {
    // Scenario: Run same algorithm multiple times on same graph
    // Expected: Results are deterministic
    let graph = create_large_test_graph(30).expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    // Run PageRank twice
    let result1 = algo::pagerank(&graph, 0.85, 20);
    let result2 = algo::pagerank(&graph, 0.85, 20);

    assert!(result1.is_ok(), "First PageRank failed");
    assert!(result2.is_ok(), "Second PageRank failed");

    let scores1 = result1.unwrap();
    let scores2 = result2.unwrap();

    // Verify same number of scores
    assert_eq!(scores1.len(), scores2.len(), "Different number of scores");

    // Verify scores are approximately equal (floating point tolerance)
    for (s1, s2) in scores1.iter().zip(scores2.iter()) {
        assert_eq!(s1.0, s2.0, "Different node IDs");
        assert!(
            (s1.1 - s2.1).abs() < 1e-10,
            "Scores differ significantly: {} vs {}",
            s1.1,
            s2.1
        );
    }
}

#[test]
fn test_multiple_algorithms_same_graph() {
    // Scenario: Run multiple different algorithms on same graph
    // Expected: All algorithms succeed
    let graph = create_large_test_graph(40).expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    // Run multiple algorithms
    let components_result = algo::connected_components(&graph);
    let degrees_result = algo::nodes_by_degree(&graph, true);
    let pagerank_result = algo::pagerank(&graph, 0.85, 10);
    let cycles_result = algo::find_cycles_limited(&graph, 10);

    assert!(components_result.is_ok(), "Connected components failed");
    assert!(degrees_result.is_ok(), "Nodes by degree failed");
    assert!(pagerank_result.is_ok(), "PageRank failed");
    assert!(cycles_result.is_ok(), "Find cycles failed");

    // Verify results are non-empty for non-empty graph
    let components = components_result.unwrap();
    let degrees = degrees_result.unwrap();
    let pagerank = pagerank_result.unwrap();

    assert!(!components.is_empty() || graph.list_entity_ids().unwrap().is_empty());
    assert!(!degrees.is_empty() || graph.list_entity_ids().unwrap().is_empty());
    assert!(!pagerank.is_empty() || graph.list_entity_ids().unwrap().is_empty());
}

#[test]
fn test_algorithm_with_empty_graph() {
    // Scenario: Run algorithms on empty graph
    // Expected: All handle empty graph gracefully
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Run algorithms on empty graph
    let components = algo::connected_components(&graph);
    let degrees = algo::nodes_by_degree(&graph, true);
    let pagerank = algo::pagerank(&graph, 0.85, 10);
    let cycles = algo::find_cycles_limited(&graph, 10);

    assert!(
        components.is_ok(),
        "Connected components should handle empty graph"
    );
    assert!(degrees.is_ok(), "Nodes by degree should handle empty graph");
    assert!(pagerank.is_ok(), "PageRank should handle empty graph");
    assert!(cycles.is_ok(), "Find cycles should handle empty graph");

    // Verify results are empty
    assert!(components.unwrap().is_empty());
    assert!(degrees.unwrap().is_empty());
    assert!(pagerank.unwrap().is_empty());
    assert!(cycles.unwrap().is_empty());
}

#[test]
fn test_algorithm_snapshot_consistency() {
    // Scenario: Acquire snapshot, run algorithm, verify consistency
    // Expected: Algorithm sees data consistent with snapshot
    let graph = create_test_graph().expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    // Acquire snapshot
    let snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let snapshot_count = snapshot.node_count();

    // Run algorithm
    let components = algo::connected_components(&graph).expect("Components failed");

    // Verify consistency
    let graph_ids = graph.list_entity_ids().expect("Failed to get graph IDs");

    // Algorithm should see same number of nodes as snapshot
    assert_eq!(
        graph_ids.len(),
        snapshot_count,
        "Algorithm and snapshot disagree on node count"
    );

    // All nodes in snapshot should be in graph
    assert!(!components.is_empty() || snapshot_count == 0);
}

//
// GROUP 6: STRESS TEST PATTERNS
//

#[test]
fn test_rapid_algorithm_execution() {
    // Scenario: Run algorithms rapidly in sequence
    // Expected: All operations succeed
    let graph = create_large_test_graph(30).expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    let start = Instant::now();

    // Run 100 algorithm executions
    for i in 0..100 {
        let result = if i % 4 == 0 {
            algo::connected_components(&graph).map(|_| ())
        } else if i % 4 == 1 {
            algo::nodes_by_degree(&graph, true).map(|_| ())
        } else if i % 4 == 2 {
            algo::pagerank(&graph, 0.85, 5).map(|_| ())
        } else {
            algo::find_cycles_limited(&graph, 5).map(|_| ())
        };

        assert!(result.is_ok(), "Algorithm {} failed", i);
    }

    let elapsed = start.elapsed();
    println!("100 algorithm executions in {:?}", elapsed);
}

#[test]
fn test_mixed_operations_sequence() {
    // Scenario: Alternate between reads, writes, and algorithms
    // Expected: All operations succeed
    let graph = create_large_test_graph(20).expect("Failed to create graph");

    for i in 0..50 {
        if i % 3 == 0 {
            // Read operation
            let _ = graph.list_entity_ids().expect("List IDs failed");
        } else if i % 3 == 1 {
            // Write operation
            let _ = insert_entity(
                &graph,
                GraphEntityCreate {
                    kind: "mixed".to_string(),
                    name: format!("mixed_node_{}", i),
                    file_path: Some(format!("mixed_{}.rs", i)),
                    data: serde_json::json!({}),
                },
            );
        } else {
            // Algorithm operation
            let _ = algo::nodes_by_degree(&graph, false);
        }
    }

    // Verify final state
    let final_count = node_count(&graph).expect("Failed to get final count");
    assert!(final_count > 20, "Graph should have more nodes");
}

#[test]
fn test_rapid_snapshot_creation_destruction_10k() {
    // Scenario: Rapid snapshot creation and destruction (10K iterations)
    // Expected: All operations succeed, no memory leaks
    let graph = create_large_test_graph(50).expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    let start = Instant::now();

    for i in 0..10_000 {
        let snapshot = graph
            .acquire_snapshot()
            .expect("Failed to acquire snapshot");

        // Verify snapshot is valid
        assert!(snapshot.node_count() > 0, "Snapshot {} invalid", i);

        // Snapshot dropped here
    }

    let elapsed = start.elapsed();
    println!("10K snapshot creations in {:?}", elapsed);

    // Final snapshot should still work
    let final_snapshot = graph
        .acquire_snapshot()
        .expect("Failed to acquire final snapshot");
    assert!(final_snapshot.node_count() > 0, "Final snapshot invalid");
}

#[test]
fn test_snapshot_during_algorithm_execution() {
    // Scenario: Acquire snapshots while algorithm is running
    // Expected: All snapshots are independent and consistent
    let graph = create_large_test_graph(30).expect("Failed to create graph");
    warm_cache(&graph).expect("Failed to warm cache");

    // Acquire initial snapshot
    let snapshot1 = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let count1 = snapshot1.node_count();

    // Run algorithm
    let _ = algo::label_propagation(&graph, 5).expect("Label propagation failed");

    // Acquire second snapshot
    let snapshot2 = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let count2 = snapshot2.node_count();

    // Verify snapshots are independent but same (no writes occurred)
    assert_eq!(snapshot1.node_count(), count1);
    assert_eq!(snapshot2.node_count(), count2);
    assert_eq!(count1, count2);
}

//
// GROUP 7: INTEGRATION WITH SQLITEGRAPH (SINGLE-THREADED)
//

#[test]
fn test_graph_snapshot_creation() {
    // Scenario: Create snapshot from SqliteGraph
    // Expected: Snapshot captures current state
    let graph = create_test_graph().expect("Failed to create test graph");
    warm_cache(&graph).expect("Failed to warm cache");

    let snapshot1 = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let original_count = snapshot1.node_count();

    assert!(original_count > 0, "Snapshot should have nodes");

    // Modify graph
    let _ = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "new".to_string(),
            name: "new_node".to_string(),
            file_path: Some("new.rs".to_string()),
            data: serde_json::json!({}),
        },
    );

    // Original snapshot unchanged
    assert_eq!(snapshot1.node_count(), original_count);

    // New snapshot reflects changes
    warm_cache(&graph).expect("Failed to warm cache");
    let snapshot2 = graph
        .acquire_snapshot()
        .expect("Failed to acquire second snapshot");
    assert!(snapshot2.node_count() > original_count);
}

#[test]
fn test_graph_snapshot_isolation() {
    // Scenario: Verify snapshot isolation in graph context
    // Expected: Multiple snapshots are independent
    let graph = create_large_test_graph(100).expect("Failed to create large graph");
    warm_cache(&graph).expect("Failed to warm cache");

    let snapshot1 = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let count1 = snapshot1.node_count();

    // Modify graph
    let _ = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "test".to_string(),
            name: "new_node".to_string(),
            file_path: Some("new.rs".to_string()),
            data: serde_json::json!({}),
        },
    );

    warm_cache(&graph).expect("Failed to warm cache");
    let snapshot2 = graph
        .acquire_snapshot()
        .expect("Failed to acquire snapshot");
    let count2 = snapshot2.node_count();

    // Verify isolation
    assert_eq!(snapshot1.node_count(), count1, "Snapshot 1 changed");
    assert_eq!(snapshot2.node_count(), count2, "Snapshot 2 has wrong count");
    assert!(count2 > count1, "Snapshot 2 should have more nodes");
}

#[test]
fn test_graph_snapshot_performance() {
    // Scenario: Measure snapshot acquisition performance
    // Expected: Fast snapshot creation
    let graph = create_test_graph().expect("Failed to create test graph");
    warm_cache(&graph).expect("Failed to warm cache");

    let start = Instant::now();

    for _ in 0..100 {
        let _ = graph
            .acquire_snapshot()
            .expect("Failed to acquire snapshot");
    }

    let elapsed = start.elapsed();

    println!("100 snapshot acquisitions in {:?}", elapsed);

    // Should be reasonably fast (< 100ms)
    assert!(
        elapsed < Duration::from_millis(100),
        "Snapshot acquisition too slow: {:?}",
        elapsed
    );
}
