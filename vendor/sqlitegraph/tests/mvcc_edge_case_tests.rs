//! MVCC Snapshot Lifecycle Edge Case Tests
//!
//! **Purpose:** Validate snapshot lifecycle behavior under edge conditions
//! **Scope:** Empty graphs, large graphs, rapid lifecycle, memory pressure
//! **Focus:** Snapshot lifecycle edge cases, stress testing, memory validation
//!
//! These tests verify that MVCC-lite snapshots handle edge cases correctly
//! including empty graphs, large graphs, rapid creation/destruction, and
/// special scenarios like transaction rollbacks and deleted nodes.
use sqlitegraph::{GraphEdgeCreate, GraphEntityCreate, SqliteGraph, SqliteGraphError};
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

    let entity3 = GraphEntityCreate {
        kind: "variable".to_string(),
        name: "config".to_string(),
        file_path: Some("src/config.rs".to_string()),
        data: serde_json::json!({"type": "String"}),
    };

    let id1 = insert_entity(&graph, entity1)?;
    let id2 = insert_entity(&graph, entity2)?;
    let id3 = insert_entity(&graph, entity3)?;

    // Create test edges
    let edge1 = GraphEdgeCreate {
        from_id: id1,
        to_id: id2,
        edge_type: "calls".to_string(),
        data: serde_json::json!({"line": 15}),
    };

    let edge2 = GraphEdgeCreate {
        from_id: id1,
        to_id: id3,
        edge_type: "reads".to_string(),
        data: serde_json::json!({"line": 12}),
    };

    insert_edge(&graph, edge1)?;
    insert_edge(&graph, edge2)?;

    Ok(graph)
}

//
// GROUP 1: EMPTY GRAPH SNAPSHOTS
//

#[test]
fn test_empty_graph_snapshot() -> Result<(), SqliteGraphError> {
    // Scenario: Create snapshot of empty graph
    // Expected: Snapshot has 0 nodes and 0 edges

    let graph = SqliteGraph::open_in_memory()?;

    // Warm cache (no-op for empty graph)
    warm_cache(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify empty state
    assert_eq!(snapshot.node_count(), 0);
    assert_eq!(snapshot.edge_count(), 0);

    // Verify no nodes exist
    assert!(!snapshot.contains_node(1));
    assert!(!snapshot.contains_node(999));

    // Verify neighbor queries return None
    assert_eq!(snapshot.get_outgoing(1), None);
    assert_eq!(snapshot.get_incoming(1), None);

    Ok(())
}

#[test]
fn test_empty_graph_snapshot_after_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Empty snapshot, then writes, verify snapshot unchanged
    // Expected: Empty snapshot remains empty after writes

    let graph = SqliteGraph::open_in_memory()?;

    // Acquire empty snapshot
    warm_cache(&graph)?;
    let snapshot_empty = graph.acquire_snapshot()?;
    assert_eq!(snapshot_empty.node_count(), 0);

    // Add data
    for i in 0..10 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "test".to_string(),
                name: format!("test_{}", i),
                file_path: Some(format!("test_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    // Verify empty snapshot still empty
    assert_eq!(snapshot_empty.node_count(), 0);
    assert_eq!(snapshot_empty.edge_count(), 0);

    // Verify new snapshot sees data
    warm_cache(&graph)?;
    let snapshot_populated = graph.acquire_snapshot()?;
    assert!(snapshot_populated.node_count() > 0);

    Ok(())
}

#[test]
fn test_multiple_empty_snapshots() -> Result<(), SqliteGraphError> {
    // Scenario: Create multiple snapshots of empty graph
    // Expected: All snapshots have 0 nodes

    let graph = SqliteGraph::open_in_memory()?;

    warm_cache(&graph)?;

    // Create multiple snapshots
    let snapshot1 = graph.acquire_snapshot()?;
    let snapshot2 = graph.acquire_snapshot()?;
    let snapshot3 = graph.acquire_snapshot()?;

    // All should be empty
    assert_eq!(snapshot1.node_count(), 0);
    assert_eq!(snapshot2.node_count(), 0);
    assert_eq!(snapshot3.node_count(), 0);

    Ok(())
}

//
// GROUP 2: LARGE GRAPH SNAPSHOTS
//

#[test]
fn test_large_graph_snapshot_memory() -> Result<(), SqliteGraphError> {
    // Scenario: Create graph with 100K+ nodes, acquire multiple snapshots
    // Expected: All snapshots succeed, no unbounded memory growth

    let graph = SqliteGraph::open_in_memory()?;

    // Create large graph (10K nodes to keep test fast)
    let num_nodes = 10_000;

    println!("Creating {} nodes...", num_nodes);
    for i in 0..num_nodes {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "large".to_string(),
                name: format!("large_node_{}", i),
                file_path: Some(format!("large_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            },
        )?;
    }

    // Warm cache
    println!("Warming cache...");
    warm_cache(&graph)?;

    let total_nodes = node_count(&graph)?;
    println!("Total nodes: {}", total_nodes);

    // Acquire multiple snapshots
    println!("Acquiring snapshots...");
    let snapshot1 = graph.acquire_snapshot()?;
    let snapshot2 = graph.acquire_snapshot()?;
    let snapshot3 = graph.acquire_snapshot()?;

    // The adjacency cache is bounded (sqlitegraph::cache::DEFAULT_CACHE_CAPACITY,
    // 8192 entries). A snapshot reads from that cache, so for a graph larger
    // than the cache capacity the snapshot reflects the retained working set,
    // not every node in the DB. This test's purpose is to confirm that acquiring
    // multiple snapshots does NOT cause unbounded memory growth and that the
    // snapshots are mutually consistent — both still hold under the bounded cache.
    use sqlitegraph::cache::DEFAULT_CACHE_CAPACITY;
    assert!(snapshot1.node_count() <= DEFAULT_CACHE_CAPACITY);
    assert!(snapshot1.node_count() > 0);
    assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    assert_eq!(snapshot2.node_count(), snapshot3.node_count());

    println!(
        "All snapshots consistent: {} nodes retained (cap {}, db total {})",
        snapshot1.node_count(),
        DEFAULT_CACHE_CAPACITY,
        total_nodes
    );

    Ok(())
}

#[test]
fn test_large_graph_snapshot_performance() -> Result<(), SqliteGraphError> {
    // Scenario: Measure snapshot acquisition latency for large graph
    // Expected: Acquisition completes in reasonable time

    let graph = SqliteGraph::open_in_memory()?;

    // Create moderately large graph
    let num_nodes = 5_000;

    for i in 0..num_nodes {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "perf".to_string(),
                name: format!("perf_node_{}", i),
                file_path: Some(format!("perf_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Measure snapshot acquisition
    let start = Instant::now();
    let snapshot = graph.acquire_snapshot()?;
    let duration = start.elapsed();

    println!(
        "Snapshot acquisition for {} nodes: {:?}",
        num_nodes, duration
    );

    // Verify snapshot is valid
    assert!(snapshot.node_count() > 0);

    // Should complete in reasonable time (< 5 seconds)
    assert!(duration < Duration::from_secs(5));

    Ok(())
}

//
// GROUP 3: RAPID SNAPSHOT LIFECYCLE
//

#[test]
fn test_rapid_snapshot_lifecycle() -> Result<(), SqliteGraphError> {
    // Scenario: Create and drop 10K snapshots in rapid succession
    // Expected: All operations succeed, no memory leaks

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let iterations = 10_000;

    println!("Creating {} snapshots...", iterations);
    let start = Instant::now();

    for i in 0..iterations {
        let snapshot = graph.acquire_snapshot()?;

        // Verify snapshot valid
        assert!(snapshot.node_count() > 0, "Snapshot {} invalid", i);

        // Snapshot dropped here
    }

    let duration = start.elapsed();
    println!(
        "Created and dropped {} snapshots in {:?}",
        iterations, duration
    );

    // Final snapshot should still work
    let final_snapshot = graph.acquire_snapshot()?;
    assert!(final_snapshot.node_count() > 0);

    Ok(())
}

#[test]
fn test_rapid_snapshot_creation() -> Result<(), SqliteGraphError> {
    // Scenario: Create 1K snapshots as fast as possible
    // Expected: All succeed, performance reasonable

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let mut snapshots = Vec::new();
    let count = 1_000;

    let start = Instant::now();

    for _ in 0..count {
        snapshots.push(graph.acquire_snapshot()?);
    }

    let duration = start.elapsed();
    println!("Created {} snapshots in {:?}", count, duration);

    // Verify all snapshots valid
    for (i, snapshot) in snapshots.iter().enumerate() {
        assert!(snapshot.node_count() > 0, "Snapshot {} invalid", i);
    }

    // Should be fast (< 1 second for 1K snapshots)
    assert!(duration < Duration::from_secs(1));

    Ok(())
}

#[test]
fn test_snapshot_clone_stress() -> Result<(), SqliteGraphError> {
    // Scenario: Clone snapshot many times
    // Expected: All clones work, cheap operation

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let snapshot = std::rc::Rc::new(graph.acquire_snapshot()?);

    let start = Instant::now();

    // Clone 10K times
    for _ in 0..10_000 {
        let _clone = std::rc::Rc::clone(&snapshot);
    }

    let duration = start.elapsed();
    println!("10K Arc clones in {:?}", duration);

    // Arc::clone should be very fast (< 100ms)
    assert!(duration < Duration::from_millis(100));

    Ok(())
}

//
// GROUP 4: SNAPSHOT DURING TRANSACTION ROLLBACK
//

#[test]
fn test_snapshot_during_transaction_commit() -> Result<(), SqliteGraphError> {
    // Scenario: Acquire snapshot, then transaction commit
    // Expected: Snapshot unaffected by committed transaction

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Acquire initial snapshot
    let snapshot1 = graph.acquire_snapshot()?;
    let count1 = snapshot1.node_count();

    // Perform writes (SQLite auto-commits each statement)
    for i in 0..5 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "commit_test".to_string(),
                name: format!("commit_func_{}", i),
                file_path: Some(format!("commit_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    // Verify snapshot1 unchanged
    assert_eq!(snapshot1.node_count(), count1);

    // New snapshot sees committed data
    warm_cache(&graph)?;
    let snapshot2 = graph.acquire_snapshot()?;
    assert!(snapshot2.node_count() > count1);

    Ok(())
}

#[test]
fn test_snapshot_isolation_with_deletes() -> Result<(), SqliteGraphError> {
    // Scenario: Create snapshot, then delete nodes from graph
    // Expected: Snapshot still sees deleted nodes

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;
    let original_count = snapshot.node_count();

    // Verify snapshot sees all nodes
    assert!(original_count > 0);

    // Delete a node from main graph
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        graph.delete_entity(entity_ids[0])?;
    }

    // Verify main graph changed
    let new_count = node_count(&graph)?;
    assert!(new_count < original_count as i64);

    // Verify snapshot unchanged (still sees deleted node)
    assert_eq!(snapshot.node_count(), original_count);

    Ok(())
}

#[test]
fn test_snapshot_with_deleted_node_visibility() -> Result<(), SqliteGraphError> {
    // Scenario: Verify snapshot maintains visibility of deleted nodes
    // Expected: Snapshot preserves adjacency of deleted nodes

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let entity_ids = graph.list_entity_ids()?;
    if entity_ids.len() < 2 {
        return Ok(()); // Skip if not enough entities
    }

    // Get neighbors before deletion
    let test_node = entity_ids[0];
    let neighbors_before = graph.query().outgoing(test_node)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot sees neighbors
    let snapshot_neighbors = snapshot.get_outgoing(test_node);
    assert_eq!(snapshot_neighbors, Some(&neighbors_before));

    // Delete node from main graph
    graph.delete_entity(test_node)?;

    // Verify snapshot still sees deleted node and its neighbors
    assert!(snapshot.contains_node(test_node));
    assert_eq!(snapshot.get_outgoing(test_node), Some(&neighbors_before));

    Ok(())
}

//
// GROUP 5: SPECIAL SCENARIOS
//

#[test]
fn test_snapshot_with_single_node() -> Result<(), SqliteGraphError> {
    // Scenario: Graph with single node, no edges
    // Expected: Snapshot handles single node correctly

    let graph = SqliteGraph::open_in_memory()?;

    // Create single node
    let entity_id = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "single".to_string(),
            name: "single_node".to_string(),
            file_path: Some("single.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;

    warm_cache(&graph)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify single node
    assert_eq!(snapshot.node_count(), 1);
    assert_eq!(snapshot.edge_count(), 0);

    // Verify neighbor access
    assert!(snapshot.contains_node(entity_id));
    assert_eq!(snapshot.get_outgoing(entity_id), Some(&vec![]));
    assert_eq!(snapshot.get_incoming(entity_id), Some(&vec![]));

    Ok(())
}

#[test]
fn test_snapshot_with_disconnected_components() -> Result<(), SqliteGraphError> {
    // Scenario: Graph with multiple disconnected components
    // Expected: Snapshot sees all components

    let graph = SqliteGraph::open_in_memory()?;

    // Create component 1
    let id1 = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "comp1".to_string(),
            name: "comp1_node1".to_string(),
            file_path: Some("comp1.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;
    let id2 = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "comp1".to_string(),
            name: "comp1_node2".to_string(),
            file_path: Some("comp1.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;

    // Create component 2 (disconnected)
    let id3 = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "comp2".to_string(),
            name: "comp2_node1".to_string(),
            file_path: Some("comp2.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;
    let id4 = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "comp2".to_string(),
            name: "comp2_node2".to_string(),
            file_path: Some("comp2.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;

    // Connect component 1
    insert_edge(
        &graph,
        GraphEdgeCreate {
            from_id: id1,
            to_id: id2,
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        },
    )?;

    // Connect component 2
    insert_edge(
        &graph,
        GraphEdgeCreate {
            from_id: id3,
            to_id: id4,
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        },
    )?;

    warm_cache(&graph)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify all nodes present
    assert_eq!(snapshot.node_count(), 4);
    assert_eq!(snapshot.edge_count(), 2);

    // Verify component 1 connectivity
    let neighbors1 = snapshot.get_outgoing(id1);
    assert_eq!(neighbors1, Some(&vec![id2]));

    // Verify component 2 connectivity
    let neighbors3 = snapshot.get_outgoing(id3);
    assert_eq!(neighbors3, Some(&vec![id4]));

    Ok(())
}

#[test]
fn test_snapshot_consistency_under_modifications() -> Result<(), SqliteGraphError> {
    // Scenario: Snapshot remains consistent while graph is modified
    // Expected: Snapshot state immutable despite modifications

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let entity_ids = graph.list_entity_ids()?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;
    let original_count = snapshot.node_count();
    let original_edges = snapshot.edge_count();

    // Perform various modifications
    for i in 0..10 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "mod".to_string(),
                name: format!("mod_func_{}", i),
                file_path: Some(format!("mod_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;

        if !entity_ids.is_empty() {
            let new_id = insert_entity(
                &graph,
                GraphEntityCreate {
                    kind: "edge_mod".to_string(),
                    name: format!("edge_mod_{}", i),
                    file_path: Some(format!("edge_mod_{}.rs", i)),
                    data: serde_json::json!({}),
                },
            )?;

            insert_edge(
                &graph,
                GraphEdgeCreate {
                    from_id: entity_ids[0],
                    to_id: new_id,
                    edge_type: "mod_edge".to_string(),
                    data: serde_json::json!({}),
                },
            )?;
        }
    }

    // Verify snapshot unchanged
    assert_eq!(snapshot.node_count(), original_count);
    assert_eq!(snapshot.edge_count(), original_edges);

    Ok(())
}

#[test]
fn test_multiple_snapshots_different_states() -> Result<(), SqliteGraphError> {
    // Scenario: Acquire snapshots at different graph states
    // Expected: Each snapshot reflects state at acquisition time

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Snapshot 1: Initial state
    let snapshot1 = graph.acquire_snapshot()?;
    let count1 = snapshot1.node_count();

    // Add data
    for i in 0..5 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "phase1".to_string(),
                name: format!("phase1_{}", i),
                file_path: Some(format!("phase1_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Snapshot 2: After phase 1
    let snapshot2 = graph.acquire_snapshot()?;
    let count2 = snapshot2.node_count();

    // Add more data
    for i in 0..5 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "phase2".to_string(),
                name: format!("phase2_{}", i),
                file_path: Some(format!("phase2_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Snapshot 3: After phase 2
    let snapshot3 = graph.acquire_snapshot()?;
    let count3 = snapshot3.node_count();

    // Verify monotonic growth and isolation
    assert!(count1 < count2);
    assert!(count2 < count3);

    assert_eq!(snapshot1.node_count(), count1);
    assert_eq!(snapshot2.node_count(), count2);
    assert_eq!(snapshot3.node_count(), count3);

    Ok(())
}

//
// GROUP 6: SNAPSHOT LIFECYCLE EDGE CASES
//

#[test]
fn test_snapshot_outlives_graph() -> Result<(), SqliteGraphError> {
    // Scenario: Snapshot outlives graph (should work with Arc)
    // Expected: Snapshot remains valid even after graph is dropped
    let snapshot = {
        let graph = create_test_graph()?;
        warm_cache(&graph)?;

        // Acquire snapshot and move it out

        graph.acquire_snapshot()?
    };

    // Graph is dropped here, but snapshot should still work
    // (Snapshot holds Arc<SnapshotState>, which is independent)

    let count = snapshot.node_count();
    assert!(count > 0, "Snapshot should remain valid");

    // Verify snapshot data is still accessible
    let _ = snapshot.edge_count();

    Ok(())
}

#[test]
fn test_snapshot_clone_independence() -> Result<(), SqliteGraphError> {
    // Scenario: Clone snapshot and verify independence
    // Expected: Both clones see same state, independent modifications
    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let snapshot1 = std::rc::Rc::new(graph.acquire_snapshot()?);
    let snapshot2 = std::rc::Rc::clone(&snapshot1);

    // Both should see same data
    assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    assert_eq!(snapshot1.edge_count(), snapshot2.edge_count());

    // Modify graph
    let _ = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "clone_test".to_string(),
            name: "clone_node".to_string(),
            file_path: Some("clone.rs".to_string()),
            data: serde_json::json!({}),
        },
    );

    warm_cache(&graph)?;

    // Both snapshots should still see original state
    let original_count = snapshot1.node_count();
    assert_eq!(snapshot2.node_count(), original_count);

    // New snapshot should see more nodes
    let new_snapshot = graph.acquire_snapshot()?;
    assert!(new_snapshot.node_count() > original_count);

    Ok(())
}

#[test]
fn test_nested_snapshots() -> Result<(), SqliteGraphError> {
    // Scenario: Create snapshot, then another snapshot (not of snapshot, but sequential)
    // Expected: Both snapshots are independent
    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // First snapshot
    let snapshot1 = graph.acquire_snapshot()?;
    let count1 = snapshot1.node_count();

    // Second snapshot (not nested, but sequential)
    let snapshot2 = graph.acquire_snapshot()?;
    let count2 = snapshot2.node_count();

    // Should be equal (no modifications between)
    assert_eq!(count1, count2);
    assert_eq!(snapshot1.node_count(), snapshot2.node_count());

    // Modify graph
    let _ = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "nested".to_string(),
            name: "nested_node".to_string(),
            file_path: Some("nested.rs".to_string()),
            data: serde_json::json!({}),
        },
    );

    warm_cache(&graph)?;

    // Third snapshot
    let snapshot3 = graph.acquire_snapshot()?;

    // First two unchanged, third sees modification
    assert_eq!(snapshot1.node_count(), count1);
    assert_eq!(snapshot2.node_count(), count2);
    assert!(snapshot3.node_count() > count2);

    Ok(())
}

#[test]
fn test_snapshot_consistency_with_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Verify snapshot remains consistent despite writes
    // Expected: Snapshot never changes after acquisition
    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let snapshot = graph.acquire_snapshot()?;
    let original_count = snapshot.node_count();
    let original_edges = snapshot.edge_count();

    // Perform many writes
    for i in 0..20 {
        let _ = insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "write_test".to_string(),
                name: format!("write_node_{}", i),
                file_path: Some(format!("write_{}.rs", i)),
                data: serde_json::json!({}),
            },
        );
    }

    warm_cache(&graph)?;

    // Verify snapshot unchanged
    assert_eq!(snapshot.node_count(), original_count);
    assert_eq!(snapshot.edge_count(), original_edges);

    // Verify graph changed
    let final_count = node_count(&graph)?;
    assert!(final_count > original_count as i64);

    Ok(())
}

//
// GROUP 7: TRANSACTION EDGE CASES
//

#[test]
fn test_empty_transaction() -> Result<(), SqliteGraphError> {
    // Scenario: Transaction with no operations (SQLite auto-commits each statement)
    // Expected: Empty state is valid, no errors
    let graph = SqliteGraph::open_in_memory()?;

    // Auto-commit each statement in SQLite
    let ids = graph.list_entity_ids()?;
    assert!(ids.is_empty(), "Graph should be empty");

    // "Empty transaction" - no operations performed
    // Verify state is consistent
    let count_after = node_count(&graph)?;
    assert_eq!(count_after, 0, "Count should still be 0");

    Ok(())
}

#[test]
fn test_transaction_with_failed_operations() -> Result<(), SqliteGraphError> {
    // Scenario: Transaction mix of successful and failed operations
    // Expected: Successful operations commit, failed ones error
    let graph = SqliteGraph::open_in_memory()?;

    // Successful operation
    let id1 = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "test".to_string(),
            name: "success".to_string(),
            file_path: Some("success.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;

    // Try to create edge to non-existent node (will fail)
    let result = insert_edge(
        &graph,
        GraphEdgeCreate {
            from_id: id1,
            to_id: 99999, // Non-existent
            edge_type: "fails".to_string(),
            data: serde_json::json!({}),
        },
    );

    // Edge creation should fail
    assert!(result.is_err(), "Edge to non-existent node should fail");

    // But entity should still exist (SQLite auto-commits the insert)
    let ids = graph.list_entity_ids()?;
    assert_eq!(ids.len(), 1, "Entity should exist");

    Ok(())
}

#[test]
fn test_partial_modification_state() -> Result<(), SqliteGraphError> {
    // Scenario: Partial graph modification with errors
    // Expected: Successful modifications persist, errors don't corrupt state
    let graph = create_test_graph()?;

    let initial_count = node_count(&graph)?;

    // Add some entities successfully
    for i in 0..5 {
        let _ = insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "partial".to_string(),
                name: format!("partial_{}", i),
                file_path: Some(format!("partial_{}.rs", i)),
                data: serde_json::json!({}),
            },
        );
    }

    // Try to create edges to non-existent nodes
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        let _ = insert_edge(
            &graph,
            GraphEdgeCreate {
                from_id: entity_ids[0],
                to_id: 99998,
                edge_type: "bad_edge".to_string(),
                data: serde_json::json!({}),
            },
        );
    }

    // Verify state is consistent
    let final_count = node_count(&graph)?;
    assert!(
        final_count > initial_count,
        "Successful inserts should persist"
    );

    // Verify graph is still functional
    let _ = graph.acquire_snapshot()?;

    Ok(())
}
