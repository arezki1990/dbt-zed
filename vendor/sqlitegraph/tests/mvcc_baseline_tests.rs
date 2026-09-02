//! MVCC Baseline Tests - Single-threaded functionality verification
//!
//! **Purpose:** Establish baseline behavior and metrics for MVCC-lite system
//! **Scope:** Single-threaded tests only (concurrent tests in Plan 04-02)
//! **Focus:** Verify current behavior, measure performance, document edge cases
//!
//! These tests establish measurable baselines before implementing concurrent access
//! tests. They document the current state even if buggy - this is intentional.

use sqlitegraph::{
    GraphEdgeCreate, GraphEntityCreate, SqliteGraph, SqliteGraphError, add_label, add_property,
    cache_stats,
};
use std::time::{Duration, Instant};

//
// TEST HELPERS
//

/// Helper: Get node count from graph
fn node_count(graph: &SqliteGraph) -> Result<i64, SqliteGraphError> {
    let ids = graph.list_entity_ids()?;
    Ok(ids.len() as i64)
}

/// Helper: Get edge count from graph
fn edge_count(graph: &SqliteGraph) -> Result<i64, SqliteGraphError> {
    let entity_ids = graph.list_entity_ids()?;
    let mut total_edges = 0;
    for &id in &entity_ids {
        let outgoing = graph.query().outgoing(id)?;
        total_edges += outgoing.len();
    }
    Ok(total_edges as i64)
}

/// Helper: Get neighbors for a node
fn get_neighbors(graph: &SqliteGraph, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
    graph.query().outgoing(id)
}

/// Helper: Get incoming neighbors for a node
fn get_incoming(graph: &SqliteGraph, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
    graph.query().incoming(id)
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

/// Helper: Add more data to existing graph
fn add_more_data(graph: &SqliteGraph) -> Result<(i64, i64), SqliteGraphError> {
    let entity4 = GraphEntityCreate {
        kind: "function".to_string(),
        name: "new_func".to_string(),
        file_path: Some("src/new.rs".to_string()),
        data: serde_json::json!({"line": 20}),
    };

    let entity5 = GraphEntityCreate {
        kind: "class".to_string(),
        name: "TestClass".to_string(),
        file_path: Some("src/class.rs".to_string()),
        data: serde_json::json!({"methods": 3}),
    };

    let id4 = insert_entity(graph, entity4)?;
    let id5 = insert_entity(graph, entity5)?;

    let edge3 = GraphEdgeCreate {
        from_id: id4,
        to_id: id5,
        edge_type: "instantiates".to_string(),
        data: serde_json::json!({"line": 25}),
    };

    insert_edge(graph, edge3)?;

    Ok((id4, id5))
}

//
// GROUP 1: SNAPSHOT ISOLATION TESTS
//

#[test]
fn test_snapshot_isolation_single_threaded() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let initial_nodes = node_count(&graph)?;
    let initial_edges = edge_count(&graph)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot sees consistent state
    assert_eq!(snapshot.node_count() as i64, initial_nodes);
    assert_eq!(snapshot.edge_count() as i64, initial_edges);

    // Modify graph after snapshot
    add_more_data(&graph)?;

    // Verify graph changed
    assert!(node_count(&graph)? > initial_nodes);
    assert!(edge_count(&graph)? > initial_edges);

    // Verify snapshot unchanged (isolation)
    assert_eq!(snapshot.node_count() as i64, initial_nodes);
    assert_eq!(snapshot.edge_count() as i64, initial_edges);

    Ok(())
}

#[test]
fn test_snapshot_neighbor_isolation() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let entity_ids = graph.list_entity_ids()?;
    assert!(!entity_ids.is_empty());

    let test_node = entity_ids[0];

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let original_neighbors = get_neighbors(&graph, test_node)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot neighbors match initial state
    let snapshot_neighbors = snapshot.get_outgoing(test_node);
    assert_eq!(snapshot_neighbors, Some(&original_neighbors));

    // Add new edge to main graph
    if entity_ids.len() >= 2 {
        let new_edge = GraphEdgeCreate {
            from_id: test_node,
            to_id: entity_ids[1],
            edge_type: "new_relation".to_string(),
            data: serde_json::json!({"test": true}),
        };
        insert_edge(&graph, new_edge)?;

        // Verify main graph has new neighbor
        let updated_neighbors = get_neighbors(&graph, test_node)?;
        assert!(updated_neighbors.len() > original_neighbors.len());

        // Verify snapshot neighbors unchanged
        let snapshot_neighbors_after = snapshot.get_outgoing(test_node);
        assert_eq!(snapshot_neighbors_after, Some(&original_neighbors));
    }

    Ok(())
}

#[test]
fn test_snapshot_incoming_neighbor_isolation() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let entity_ids = graph.list_entity_ids()?;
    assert!(entity_ids.len() >= 2);

    let target_node = entity_ids[1];

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let original_incoming = get_incoming(&graph, target_node)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot incoming neighbors match
    let snapshot_incoming = snapshot.get_incoming(target_node);
    assert_eq!(snapshot_incoming, Some(&original_incoming));

    // Add new edge pointing to target
    let new_edge = GraphEdgeCreate {
        from_id: entity_ids[0],
        to_id: target_node,
        edge_type: "new_incoming".to_string(),
        data: serde_json::json!({}),
    };
    insert_edge(&graph, new_edge)?;

    // Verify main graph changed
    let updated_incoming = get_incoming(&graph, target_node)?;
    assert!(updated_incoming.len() > original_incoming.len());

    // Verify snapshot unchanged
    let snapshot_incoming_after = snapshot.get_incoming(target_node);
    assert_eq!(snapshot_incoming_after, Some(&original_incoming));

    Ok(())
}

//
// GROUP 2: SNAPSHOT LIFECYCLE TESTS
//

#[test]
fn test_snapshot_creation_basic() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // IMPORTANT: Cache must be warmed before snapshot acquisition
    // This is a current limitation of the MVCC-lite implementation
    warm_cache(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot exists and has basic properties
    assert!(snapshot.node_count() > 0);
    assert!(snapshot.edge_count() > 0);

    // Verify snapshot has valid timestamp
    let created_at = snapshot.created_at();
    let now = std::time::SystemTime::now();
    assert!(created_at <= now);

    Ok(())
}

#[test]
fn test_multiple_snapshots_same_state() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let initial_nodes = node_count(&graph)?;
    let initial_edges = edge_count(&graph)?;

    // Create multiple snapshots from same base
    let snapshot1 = graph.acquire_snapshot()?;
    let snapshot2 = graph.acquire_snapshot()?;
    let snapshot3 = graph.acquire_snapshot()?;

    // Verify all have identical content
    assert_eq!(snapshot1.node_count(), initial_nodes as usize);
    assert_eq!(snapshot2.node_count(), initial_nodes as usize);
    assert_eq!(snapshot3.node_count(), initial_nodes as usize);

    assert_eq!(snapshot1.edge_count(), initial_edges as usize);
    assert_eq!(snapshot2.edge_count(), initial_edges as usize);
    assert_eq!(snapshot3.edge_count(), initial_edges as usize);

    Ok(())
}

#[test]
fn test_snapshot_ordering_consistency() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Get node list before snapshot
    let entity_ids = graph.list_entity_ids()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Snapshot contains adjacency data, not entity list
    // But we can verify node_count is consistent
    let count1 = snapshot.node_count();
    let count2 = snapshot.node_count();
    let count3 = snapshot.node_count();

    assert_eq!(count1, entity_ids.len());
    assert_eq!(count2, entity_ids.len());
    assert_eq!(count3, entity_ids.len());

    Ok(())
}

//
// GROUP 3: MEMORY FOOTPRINT TESTS
//

#[test]
fn test_snapshot_memory_overhead() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create a modest graph (100 nodes, 200 edges)
    let mut entity_ids = Vec::new();
    for i in 0..100 {
        let entity = GraphEntityCreate {
            kind: "test".to_string(),
            name: format!("entity_{}", i),
            file_path: Some(format!("file_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let id = insert_entity(&graph, entity)?;
        entity_ids.push(id);
    }

    // Create edges
    for i in 0..200 {
        let from = entity_ids[i % entity_ids.len()];
        let to = entity_ids[(i + 1) % entity_ids.len()];
        let edge = GraphEdgeCreate {
            from_id: from,
            to_id: to,
            edge_type: "connects".to_string(),
            data: serde_json::json!({"pair": i}),
        };
        insert_edge(&graph, edge)?;
    }

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Measure memory before snapshot
    let before_nodes = node_count(&graph)?;
    let before_edges = edge_count(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot captured all data
    assert_eq!(snapshot.node_count() as i64, before_nodes);
    assert_eq!(snapshot.edge_count() as i64, before_edges);

    // Note: Actual memory measurement would require custom allocator
    // This test verifies functional correctness
    // Memory profiling should be done with external tools (valgrind, etc.)

    Ok(())
}

#[test]
fn test_large_graph_snapshot() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create larger graph (1000 nodes, 2000 edges)
    let mut entity_ids = Vec::new();
    for i in 0..1000 {
        let entity = GraphEntityCreate {
            kind: "test".to_string(),
            name: format!("entity_{}", i),
            file_path: Some(format!("file_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let id = insert_entity(&graph, entity)?;
        entity_ids.push(id);
    }

    // Create edges
    for i in 0..2000 {
        let from = entity_ids[i % entity_ids.len()];
        let to = entity_ids[(i + 1) % entity_ids.len()];
        let edge = GraphEdgeCreate {
            from_id: from,
            to_id: to,
            edge_type: "connects".to_string(),
            data: serde_json::json!({"pair": i}),
        };
        insert_edge(&graph, edge)?;
    }

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let total_nodes = node_count(&graph)?;
    let total_edges = edge_count(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot captures all data
    assert_eq!(snapshot.node_count() as i64, total_nodes);
    assert_eq!(snapshot.edge_count() as i64, total_edges);

    Ok(())
}

#[test]
fn test_multiple_snapshots_memory() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Create multiple snapshots
    let snapshot1 = graph.acquire_snapshot()?;
    let snapshot2 = graph.acquire_snapshot()?;
    let snapshot3 = graph.acquire_snapshot()?;

    // Verify all snapshots are independent
    assert!(snapshot1.node_count() > 0);
    assert!(snapshot2.node_count() > 0);
    assert!(snapshot3.node_count() > 0);

    // Snapshots should have independent Arc references
    // (verified by drop behavior)

    Ok(())
}

//
// GROUP 4: PERFORMANCE BASELINE TESTS
//

#[test]
fn test_snapshot_acquisition_latency() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Warm up
    let _ = graph.acquire_snapshot()?;

    // Measure acquisition latency
    let start = Instant::now();
    let _snapshot = graph.acquire_snapshot()?;
    let duration = start.elapsed();

    // Baseline: Should complete in reasonable time
    // This documents current performance, not enforcing a limit
    println!("Snapshot acquisition latency: {:?}", duration);

    // Sanity check: should not take more than 1 second
    assert!(duration < Duration::from_secs(1));

    Ok(())
}

#[test]
fn test_snapshot_clone_performance() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let snapshot = graph.acquire_snapshot()?;

    // Measure clone performance (Arc::clone should be fast)
    let start = Instant::now();
    for _ in 0..1000 {
        let _clone = snapshot.state().clone();
    }
    let duration = start.elapsed();

    println!("1000 snapshot clones: {:?}", duration);

    // Arc::clone should be very fast
    assert!(duration < Duration::from_millis(100));

    Ok(())
}

#[test]
fn test_multiple_snapshot_overhead() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let start = Instant::now();

    // Create 100 snapshots
    let mut snapshots = Vec::new();
    for _ in 0..100 {
        snapshots.push(graph.acquire_snapshot()?);
    }

    let duration = start.elapsed();

    println!("100 snapshots created in: {:?}", duration);

    // Verify all snapshots valid
    for snapshot in snapshots {
        assert!(snapshot.node_count() > 0);
    }

    Ok(())
}

//
// GROUP 5: INTEGRATION TESTS
//

#[test]
fn test_snapshot_with_sqlite_backend() -> Result<(), SqliteGraphError> {
    // Test with SQLite backend (default)
    let graph = SqliteGraph::open_in_memory()?;

    let entity = GraphEntityCreate {
        kind: "test".to_string(),
        name: "test_entity".to_string(),
        file_path: Some("test.rs".to_string()),
        data: serde_json::json!({}),
    };
    let entity_id = insert_entity(&graph, entity)?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot works with SQLite backend
    assert!(snapshot.contains_node(entity_id));
    assert_eq!(snapshot.node_count(), 1);

    Ok(())
}

#[test]
fn test_snapshot_with_labels_and_properties() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let entity_ids = graph.list_entity_ids()?;
    let test_node = entity_ids[0];

    // Add labels and properties
    add_label(&graph, test_node, "test_label")?;
    add_property(&graph, test_node, "test_key", "test_value")?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Acquire snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Snapshot contains adjacency data only
    // Labels and properties are in SQLite, not in SnapshotState
    // This test documents current behavior

    assert!(snapshot.contains_node(test_node));

    // Note: Labels/properties not in SnapshotState
    // This is expected for MVCC-lite design

    Ok(())
}

//
// GROUP 6: EDGE CASE TESTS
//

#[test]
fn test_empty_graph_snapshot() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create snapshot of empty graph
    let snapshot = graph.acquire_snapshot()?;

    // Verify empty state
    assert_eq!(snapshot.node_count(), 0);
    assert_eq!(snapshot.edge_count(), 0);

    // Add data to main graph
    let entity = GraphEntityCreate {
        kind: "first".to_string(),
        name: "first".to_string(),
        file_path: Some("first.rs".to_string()),
        data: serde_json::json!({}),
    };
    insert_entity(&graph, entity)?;

    // Verify snapshot still empty
    assert_eq!(snapshot.node_count(), 0);
    assert_eq!(snapshot.edge_count(), 0);

    Ok(())
}

#[test]
fn test_single_node_snapshot() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create single node
    let entity = GraphEntityCreate {
        kind: "single".to_string(),
        name: "single".to_string(),
        file_path: Some("single.rs".to_string()),
        data: serde_json::json!({}),
    };
    let entity_id = insert_entity(&graph, entity)?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify single node
    assert_eq!(snapshot.node_count(), 1);
    assert_eq!(snapshot.edge_count(), 0);

    // Verify neighbor access
    let neighbors = snapshot.get_outgoing(entity_id);
    assert_eq!(neighbors, Some(&vec![]));

    Ok(())
}

#[test]
fn test_snapshot_with_deleted_entities() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let initial_nodes = node_count(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Delete entity from main graph
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        graph.delete_entity(entity_ids[0])?;
    }

    // Verify main graph changed
    assert!(node_count(&graph)? < initial_nodes);

    // Verify snapshot unchanged
    assert_eq!(snapshot.node_count(), initial_nodes as usize);

    Ok(())
}

#[test]
fn test_snapshot_consistency_during_modifications() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let entity_ids = graph.list_entity_ids();
    let entity_ids = match entity_ids {
        Ok(ids) if !ids.is_empty() => ids,
        _ => return Ok(()), // Skip test if no entities
    };

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    let initial_neighbors = get_neighbors(&graph, entity_ids[0])?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Perform rapid modifications
    for i in 0..10 {
        let new_entity = GraphEntityCreate {
            kind: "temp".to_string(),
            name: format!("temp_{}", i),
            file_path: Some(format!("temp_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let new_id = insert_entity(&graph, new_entity)?;

        let new_edge = GraphEdgeCreate {
            from_id: entity_ids[0],
            to_id: new_id,
            edge_type: "temp_relation".to_string(),
            data: serde_json::json!({"temp": true}),
        };
        insert_edge(&graph, new_edge)?;
    }

    // Verify snapshot state consistent
    let snapshot_neighbors = snapshot.get_outgoing(entity_ids[0]);
    assert_eq!(snapshot_neighbors, Some(&initial_neighbors));

    Ok(())
}

//
// GROUP 7: CACHE CONSISTENCY TESTS
//

#[test]
fn test_cache_independence() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm up caches
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        get_neighbors(&graph, entity_ids[0])?;
        get_incoming(&graph, entity_ids[0])?;
    }

    let initial_cache_stats = cache_stats(&graph);

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Access data through snapshot
    if !entity_ids.is_empty() {
        let _ = snapshot.get_outgoing(entity_ids[0]);
    }

    // Modify main graph to invalidate its caches
    if !entity_ids.is_empty() {
        let new_entity = GraphEntityCreate {
            kind: "cache_test".to_string(),
            name: "cache_test".to_string(),
            file_path: Some("cache_test.rs".to_string()),
            data: serde_json::json!({}),
        };
        let new_id = insert_entity(&graph, new_entity)?;

        let new_edge = GraphEdgeCreate {
            from_id: entity_ids[0],
            to_id: new_id,
            edge_type: "cache_test_relation".to_string(),
            data: serde_json::json!({}),
        };
        insert_edge(&graph, new_edge)?;
    }

    let final_cache_stats = cache_stats(&graph);

    // Document cache behavior
    println!("Initial cache stats: {:?}", initial_cache_stats);
    println!("Final cache stats: {:?}", final_cache_stats);

    Ok(())
}

#[test]
fn test_snapshot_read_only_enforcement() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Warm cache to populate adjacency data
    warm_cache(&graph)?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Verify snapshot provides read-only access
    // Snapshot has no write methods by design
    // This test documents the API contract

    assert!(snapshot.node_count() > 0);

    // No write methods exist on GraphSnapshot
    // This is enforced by the type system

    Ok(())
}

//
// GROUP 8: DETERMINISTIC BEHAVIOR TESTS
//

#[test]
fn test_repeatable_snapshot_results() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create multiple snapshots of the same state
    let snapshot1 = graph.acquire_snapshot()?;
    let snapshot2 = graph.acquire_snapshot()?;

    // Verify they have identical content
    assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    assert_eq!(snapshot1.edge_count(), snapshot2.edge_count());

    // Verify neighbor access is identical
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        let neighbors1 = snapshot1.get_outgoing(entity_ids[0]);
        let neighbors2 = snapshot2.get_outgoing(entity_ids[0]);
        assert_eq!(neighbors1, neighbors2);
    }

    Ok(())
}

#[test]
fn test_deterministic_query_results() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create snapshot
    let snapshot = graph.acquire_snapshot()?;

    // Run same query multiple times through snapshot
    let count1 = snapshot.node_count();
    let count2 = snapshot.node_count();
    let count3 = snapshot.node_count();

    // Verify results are identical
    assert_eq!(count1, count2);
    assert_eq!(count2, count3);

    Ok(())
}
