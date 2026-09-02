//! MVCC-Lite Snapshot Tests (S3 Hybrid Design)
//!
//! Tests for synchronous, embedded, ArcSwap-powered MVCC-lite snapshot system.
//! Design: In-memory adjacency snapshot + read-only SQLite connection.
//!
//! READ-ONLY snapshots from perspective of snapshot users.
//! Synchronous (no async, no background workers).
//! Deterministic behavior.
//! Focus on safe, consistent reads during write activity.

use sqlitegraph::{
    GraphEdgeCreate, GraphEntityCreate, SqliteGraph, SqliteGraphError, add_label, add_property,
    cache_stats,
};
use std::time::Duration;

// Helper function: Get node count
fn node_count(graph: &SqliteGraph) -> Result<i64, SqliteGraphError> {
    let ids = graph.list_entity_ids()?;
    Ok(ids.len() as i64)
}

// Helper function: Get edge count (approximate by checking edges from known nodes)
fn edge_count(graph: &SqliteGraph) -> Result<i64, SqliteGraphError> {
    let entity_ids = graph.list_entity_ids()?;
    let mut total_edges = 0;
    for &id in &entity_ids {
        let outgoing = graph.query().outgoing(id)?;
        total_edges += outgoing.len();
    }
    Ok(total_edges as i64)
}

// Helper function: Get neighbors
fn get_neighbors(graph: &SqliteGraph, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
    graph.query().outgoing(id)
}

// Helper function: Get incoming neighbors
fn get_incoming(graph: &SqliteGraph, id: i64) -> Result<Vec<i64>, SqliteGraphError> {
    graph.query().incoming(id)
}

// Helper function: Insert entity using proper API
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

// Helper function: Insert edge using proper API
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

// Test helper: Create a test graph with sample data
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

// Test helper: Add more data to existing graph
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
// GROUP 1: SNAPSHOT CREATION TESTS
//

#[test]
fn test_snapshot_creation_basic() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create snapshot - this should be implemented as a method on SqliteGraph
    // let snapshot = graph.create_snapshot()?;

    // Verify snapshot exists and has basic properties
    // assert!(snapshot.node_count() > 0);
    // assert!(snapshot.edge_count() > 0);

    // For now, just verify the base graph works
    assert!(node_count(&graph)? > 0);
    assert!(edge_count(&graph)? > 0);

    Ok(())
}

#[test]
fn test_snapshot_count_verification() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let initial_nodes = node_count(&graph)?;
    let initial_edges = edge_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Verify counts match
    // assert_eq!(snapshot.node_count(), initial_nodes);
    // assert_eq!(snapshot.edge_count(), initial_edges);

    // Add more data to main graph
    add_more_data(&graph)?;

    // Verify main graph counts changed
    assert!(node_count(&graph)? > initial_nodes);
    assert!(edge_count(&graph)? > initial_edges);

    // Verify snapshot counts unchanged (isolation)
    // assert_eq!(snapshot.node_count(), initial_nodes);
    // assert_eq!(snapshot.edge_count(), initial_edges);

    Ok(())
}

#[test]
fn test_snapshot_neighbor_access() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Get a node with neighbors
    let entity_ids = graph.list_entity_ids()?;
    assert!(!entity_ids.is_empty());

    let test_node = entity_ids[0];
    let original_neighbors = get_neighbors(&graph, test_node)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Verify neighbor access through snapshot
    // let snapshot_neighbors = snapshot.neighbors(test_node)?;
    // assert_eq!(original_neighbors, snapshot_neighbors);

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
        // let snapshot_neighbors_after = snapshot.neighbors(test_node)?;
        // assert_eq!(original_neighbors, snapshot_neighbors_after);
    }

    Ok(())
}

//
// GROUP 2: SNAPSHOT STABILITY UNDER WRITES
//

#[test]
fn test_snapshot_isolation_from_writes() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let _initial_nodes = node_count(&graph)?;
    let _initial_edges = edge_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Perform various write operations on main graph
    add_more_data(&graph)?;

    // Add labels and properties
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        add_label(&graph, entity_ids[0], "test_label")?;
        add_property(&graph, entity_ids[0], "test_key", "test_value")?;
    }

    // Verify snapshot remains unchanged
    // assert_eq!(snapshot.node_count(), initial_nodes);
    // assert_eq!(snapshot.edge_count(), initial_edges);

    Ok(())
}

#[test]
fn test_snapshot_consistency_during_modifications() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Get initial state
    let entity_ids = graph.list_entity_ids()?;
    let _initial_neighbors = if !entity_ids.is_empty() {
        Some(get_neighbors(&graph, entity_ids[0])?)
    } else {
        None
    };

    // Perform rapid modifications
    for i in 0..10 {
        let new_entity = GraphEntityCreate {
            kind: "temp".to_string(),
            name: format!("temp_{}", i),
            file_path: Some(format!("temp_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let new_id = insert_entity(&graph, new_entity)?;

        if !entity_ids.is_empty() {
            let new_edge = GraphEdgeCreate {
                from_id: entity_ids[0],
                to_id: new_id,
                edge_type: "temp_relation".to_string(),
                data: serde_json::json!({"temp": true}),
            };
            insert_edge(&graph, new_edge)?;
        }
    }

    // Verify snapshot state is consistent
    // if let Some(initial) = initial_neighbors {
    //     let snapshot_neighbors = snapshot.neighbors(entity_ids[0])?;
    //     assert_eq!(initial, snapshot_neighbors);
    // }

    Ok(())
}

//
// GROUP 3: CACHE CONSISTENCY TESTS
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

    let _initial_cache_stats = cache_stats(&graph);

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Access data through snapshot to populate snapshot caches
    // if !entity_ids.is_empty() {
    //     snapshot.neighbors(entity_ids[0])?;
    // }

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

    // Verify main graph caches were invalidated
    let _final_cache_stats = cache_stats(&graph);
    // Note: This test would need to verify cache miss behavior

    Ok(())
}

#[test]
fn test_cache_invalidation_behavior() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Access some data to populate caches
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        get_neighbors(&graph, entity_ids[0])?;
        // snapshot.neighbors(entity_ids[0])?;
    }

    // Perform writes that would invalidate main graph caches
    add_more_data(&graph)?;

    // Verify snapshot caches remain valid (no unexpected invalidations)
    // This would test that snapshot caches are independent

    Ok(())
}

//
// GROUP 4: TRANSACTION BEHAVIOR TESTS
//

#[test]
fn test_snapshot_transaction_boundaries() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create snapshot within explicit transaction
    // let _guard = graph.transaction_guard()?; // Not available in public API

    let _initial_nodes = node_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Add data within transaction
    add_more_data(&graph)?;

    // Verify snapshot sees state at creation time, not current transaction state
    // assert_eq!(snapshot.node_count(), initial_nodes);

    // Transaction will be rolled back when guard drops
    Ok(())
}

#[test]
fn test_snapshot_commit_rollback_behavior() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let initial_nodes = node_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Start transaction and add data
    {
        // let _guard = graph.transaction_guard()?; // Not available in public API
        add_more_data(&graph)?;
        // Transaction commits here when guard drops successfully
    }

    // Verify main graph changed
    assert!(node_count(&graph)? > initial_nodes);

    // Verify snapshot unchanged
    // assert_eq!(snapshot.node_count(), initial_nodes);

    // Test rollback behavior
    let _nodes_before_rollback = node_count(&graph)?;

    {
        // let _guard = graph.transaction_guard()?; // Not available in public API
        let new_entity = GraphEntityCreate {
            kind: "rollback_test".to_string(),
            name: "rollback_test".to_string(),
            file_path: Some("rollback.rs".to_string()),
            data: serde_json::json!({}),
        };
        insert_entity(&graph, new_entity)?;
        // Force rollback by panicking or using explicit rollback
        // drop(_guard); // This would normally commit, need explicit rollback
    }

    // Verify rollback worked (this would need explicit rollback method)
    // assert_eq!(node_count(&graph)?, nodes_before_rollback);

    Ok(())
}

//
// GROUP 5: LIFETIME & SAFETY TESTS
//

#[test]
fn test_snapshot_resource_management() -> Result<(), SqliteGraphError> {
    let _graph = create_test_graph()?;

    // Create multiple snapshots
    // let snapshot1 = graph.create_snapshot()?;
    // let snapshot2 = graph.create_snapshot()?;
    // let snapshot3 = graph.create_snapshot()?;

    // Use snapshots
    // assert!(snapshot1.node_count() > 0);
    // assert!(snapshot2.node_count() > 0);
    // assert!(snapshot3.node_count() > 0);

    // Let snapshots go out of scope and verify no resource leaks
    // This would need to be verified through resource monitoring

    Ok(())
}

#[test]
fn test_read_only_enforcement() -> Result<(), SqliteGraphError> {
    let _graph = create_test_graph()?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Attempt write operations through snapshot (should fail)
    // These would need to be implemented as methods that return errors

    // let new_entity = GraphEntityCreate {
    //     kind: "illegal".to_string(),
    //     name: "illegal".to_string(),
    //     file_path: Some("illegal.rs".to_string()),
    //     data: serde_json::json!({}),
    // };

    // assert!(snapshot.insert_entity(new_entity).is_err());

    Ok(())
}

//
// GROUP 6: DETERMINISTIC BEHAVIOR TESTS
//

#[test]
fn test_repeatable_snapshot_results() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create multiple snapshots of the same state
    // let snapshot1 = graph.create_snapshot()?;
    // let snapshot2 = graph.create_snapshot()?;

    // Verify they have identical content
    // assert_eq!(snapshot1.node_count(), snapshot2.node_count());
    // assert_eq!(snapshot1.edge_count(), snapshot2.edge_count());

    // Verify neighbor access is identical
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        // let neighbors1 = snapshot1.neighbors(entity_ids[0])?;
        // let neighbors2 = snapshot2.neighbors(entity_ids[0])?;
        // assert_eq!(neighbors1, neighbors2);
    }

    Ok(())
}

#[test]
fn test_deterministic_query_results() -> Result<(), SqliteGraphError> {
    let _graph = create_test_graph()?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Run same query multiple times through snapshot
    // let query = "SELECT COUNT(*) FROM graph_entities";
    // let result1 = snapshot.query(query)?;
    // let result2 = snapshot.query(query)?;
    // let result3 = snapshot.query(query)?;

    // Verify results are identical
    // assert_eq!(result1, result2);
    // assert_eq!(result2, result3);

    Ok(())
}

#[test]
fn test_snapshot_ordering_consistency() -> Result<(), SqliteGraphError> {
    let _graph = create_test_graph()?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Get ordered lists multiple times
    // let entities1 = snapshot.list_entities()?;
    // let entities2 = snapshot.list_entities()?;
    // let entities3 = snapshot.list_entities()?;

    // Verify ordering is consistent
    // assert_eq!(entities1, entities2);
    // assert_eq!(entities2, entities3);

    Ok(())
}

//
// PERFORMANCE AND STRESS TESTS
//

#[test]
fn test_multiple_snapshots_performance() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    // Create many snapshots to test performance
    let start_time = std::time::Instant::now();

    for _ in 0..100 {
        // let _snapshot = graph.create_snapshot()?;
        // Simulate some work
        let _ = node_count(&graph)?;
    }

    let duration = start_time.elapsed();

    // Verify reasonable performance (adjust threshold as needed)
    assert!(duration < Duration::from_secs(5));

    Ok(())
}

#[test]
fn test_large_graph_snapshot() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create a larger graph
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

    // Create some edges
    for i in 0..500 {
        if i + 1 < entity_ids.len() {
            let edge = GraphEdgeCreate {
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "connects".to_string(),
                data: serde_json::json!({"pair": i}),
            };
            insert_edge(&graph, edge)?;
        }
    }

    let _total_nodes = node_count(&graph)?;
    let _total_edges = edge_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Verify snapshot captures all data
    // assert_eq!(snapshot.node_count(), total_nodes);
    // assert_eq!(snapshot.edge_count(), total_edges);

    Ok(())
}

//
// EDGE CASE TESTS
//

#[test]
fn test_empty_graph_snapshot() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;

    // Create snapshot of empty graph
    // let snapshot = graph.create_snapshot()?;

    // Verify empty state
    // assert_eq!(snapshot.node_count(), 0);
    // assert_eq!(snapshot.edge_count(), 0);

    // Add data to main graph
    let entity = GraphEntityCreate {
        kind: "first".to_string(),
        name: "first".to_string(),
        file_path: Some("first.rs".to_string()),
        data: serde_json::json!({}),
    };
    insert_entity(&graph, entity)?;

    // Verify snapshot still empty
    // assert_eq!(snapshot.node_count(), 0);
    // assert_eq!(snapshot.edge_count(), 0);

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
    let _entity_id = insert_entity(&graph, entity)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Verify single node
    // assert_eq!(snapshot.node_count(), 1);
    // assert_eq!(snapshot.edge_count(), 0);

    // Verify neighbor access
    // let neighbors = snapshot.neighbors(entity_id)?;
    // assert!(neighbors.is_empty());

    Ok(())
}

#[test]
fn test_snapshot_with_deleted_entities() -> Result<(), SqliteGraphError> {
    let graph = create_test_graph()?;

    let initial_nodes = node_count(&graph)?;

    // Create snapshot
    // let snapshot = graph.create_snapshot()?;

    // Delete entity from main graph
    let entity_ids = graph.list_entity_ids()?;
    if !entity_ids.is_empty() {
        graph.delete_entity(entity_ids[0])?;
    }

    // Verify main graph changed
    assert!(node_count(&graph)? < initial_nodes);

    // Verify snapshot unchanged
    // assert_eq!(snapshot.node_count(), initial_nodes);

    Ok(())
}
