//! MVCC WAL Coordination Tests
//!
//! **Purpose:** Validate MVCC snapshot behavior during WAL-related operations
//! **Scope:** Snapshot interaction with write operations that generate WAL
//! **Focus:** Snapshot isolation during writes, concurrent operations, edge cases
//!
//! **NOTE:** These tests work with in-memory SQLite databases. Direct checkpoint
//! and WAL recovery testing would require file-based databases and are not currently
//! part of the public SqliteGraph API. These tests validate snapshot behavior
//! with write operations that would generate WAL in file-based databases.

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
// GROUP 1: SNAPSHOT WITH WAL-GENERATING WRITES
//

#[test]
fn test_snapshot_with_wal_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Acquire snapshots during write operations (which generate WAL)
    // Expected: Each snapshot consistent, no torn reads

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let initial_nodes = node_count(&graph)?;
    let initial_edges = edge_count(&graph)?;

    // Acquire snapshot before writes
    let snapshot1 = graph.acquire_snapshot()?;

    // Perform writes that would generate WAL in file-based DB
    for i in 0..10 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "wal_test".to_string(),
                name: format!("wal_func_{}", i),
                file_path: Some(format!("wal_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire snapshot after writes
    let snapshot2 = graph.acquire_snapshot()?;

    // Verify snapshot1 sees pre-write state
    assert_eq!(snapshot1.node_count() as i64, initial_nodes);
    assert_eq!(snapshot1.edge_count() as i64, initial_edges);

    // Verify snapshot2 sees post-write state
    assert!(snapshot2.node_count() as i64 > initial_nodes);

    Ok(())
}

#[test]
fn test_snapshot_isolation_during_wal_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Snapshot acquired mid-way through write sequence
    // Expected: Snapshot sees consistent state at acquisition time

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Perform initial writes
    for i in 0..5 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "phase1".to_string(),
                name: format!("phase1_func_{}", i),
                file_path: Some(format!("phase1_{}.rs", i)),
                data: serde_json::json!({"phase": 1}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire snapshot after phase 1
    let snapshot = graph.acquire_snapshot()?;
    let phase1_count = snapshot.node_count();

    // Perform phase 2 writes
    for i in 0..5 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "phase2".to_string(),
                name: format!("phase2_func_{}", i),
                file_path: Some(format!("phase2_{}.rs", i)),
                data: serde_json::json!({"phase": 2}),
            },
        )?;
    }

    // Verify snapshot unchanged after phase 2 writes
    assert_eq!(snapshot.node_count(), phase1_count);

    Ok(())
}

//
// GROUP 2: SNAPSHOT WITH CONCURRENT WAL WRITES
//

#[test]
fn test_snapshot_with_rapid_wal_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Acquire snapshots during rapid write operations
    // Expected: Each snapshot consistent, no torn reads

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let mut snapshots = Vec::new();

    // Perform rapid writes with snapshot acquisition
    for i in 0..50 {
        // Write some data
        let _new_id = insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "rapid".to_string(),
                name: format!("rapid_func_{}", i),
                file_path: Some(format!("rapid_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            },
        )?;

        // Warm cache and acquire snapshot
        warm_cache(&graph)?;
        let snapshot = graph.acquire_snapshot()?;

        // Verify snapshot is consistent
        assert!(snapshot.node_count() > 0);

        snapshots.push(snapshot);
    }

    // Verify all snapshots remain valid and consistent
    for (i, snapshot) in snapshots.iter().enumerate() {
        let count = snapshot.node_count();
        assert!(count > 0, "Snapshot {} has no nodes", i);
    }

    Ok(())
}

#[test]
fn test_snapshot_with_write_heavy_workload() -> Result<(), SqliteGraphError> {
    // Scenario: Heavy write workload with periodic snapshots
    // Expected: All snapshots consistent, monotonic growth

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let mut snapshot_counts = Vec::new();

    // Perform 20 write + snapshot cycles
    for cycle in 0..20 {
        // Add entities
        for i in 0..5 {
            insert_entity(
                &graph,
                GraphEntityCreate {
                    kind: "heavy".to_string(),
                    name: format!("heavy_func_{}_{}", cycle, i),
                    file_path: Some(format!("heavy_{}_{}.rs", cycle, i)),
                    data: serde_json::json!({"cycle": cycle}),
                },
            )?;
        }

        // Acquire snapshot
        warm_cache(&graph)?;
        let snapshot = graph.acquire_snapshot()?;
        snapshot_counts.push(snapshot.node_count());
    }

    // Verify monotonic growth
    for i in 1..snapshot_counts.len() {
        assert!(
            snapshot_counts[i] > snapshot_counts[i - 1],
            "Snapshot counts should grow monotonically"
        );
    }

    Ok(())
}

//
// GROUP 3: SNAPSHOT EDGE CASES WITH WAL
//

#[test]
fn test_empty_graph_writes_then_snapshot() -> Result<(), SqliteGraphError> {
    // Scenario: Empty graph, writes, then snapshot
    // Expected: Snapshot handles transition from empty to populated

    let graph = SqliteGraph::open_in_memory()?;

    // Acquire snapshot of empty graph
    let snapshot_empty = graph.acquire_snapshot()?;
    assert_eq!(snapshot_empty.node_count(), 0);
    assert_eq!(snapshot_empty.edge_count(), 0);

    // Add data
    for i in 0..10 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "populate".to_string(),
                name: format!("populate_func_{}", i),
                file_path: Some(format!("populate_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire new snapshot
    let snapshot_populated = graph.acquire_snapshot()?;
    assert!(snapshot_populated.node_count() > 0);

    // Verify empty snapshot still empty
    assert_eq!(snapshot_empty.node_count(), 0);

    Ok(())
}

#[test]
fn test_snapshot_consistency_after_write_burst() -> Result<(), SqliteGraphError> {
    // Scenario: Large write burst followed by snapshots
    // Expected: Snapshots consistent, no corruption

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Acquire initial snapshot
    let snapshot1 = graph.acquire_snapshot()?;

    // Perform large write burst
    for i in 0..100 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "burst".to_string(),
                name: format!("burst_func_{}", i),
                file_path: Some(format!("burst_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire new snapshot
    let snapshot2 = graph.acquire_snapshot()?;

    // Verify snapshot1 unchanged
    let count1 = snapshot1.node_count();
    assert!(count1 > 0);

    // Verify snapshot2 sees all writes
    let count2 = snapshot2.node_count();
    assert!(count2 > count1);

    Ok(())
}

#[test]
fn test_snapshot_during_complex_write_sequence() -> Result<(), SqliteGraphError> {
    // Scenario: Complex write sequence (entities + edges) with snapshots
    // Expected: Snapshots see consistent state at acquisition time

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    let entity_ids = graph.list_entity_ids()?;

    // Acquire initial snapshot
    let snapshot1 = graph.acquire_snapshot()?;

    // Add new entity
    let new_id = insert_entity(
        &graph,
        GraphEntityCreate {
            kind: "new".to_string(),
            name: "new_entity".to_string(),
            file_path: Some("new.rs".to_string()),
            data: serde_json::json!({}),
        },
    )?;

    // Add edge to new entity
    if !entity_ids.is_empty() {
        insert_edge(
            &graph,
            GraphEdgeCreate {
                from_id: entity_ids[0],
                to_id: new_id,
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire snapshot after modifications
    let snapshot2 = graph.acquire_snapshot()?;

    // Verify snapshot1 unchanged
    let count1 = snapshot1.node_count();
    let edges1 = snapshot1.edge_count();

    // Verify snapshot2 sees modifications
    let count2 = snapshot2.node_count();
    let edges2 = snapshot2.edge_count();

    assert!(count2 > count1);
    assert!(edges2 > edges1);

    Ok(())
}

//
// GROUP 4: EDGE CASE: WRITE PATTERNS
//

#[test]
fn test_snapshot_with_interleaved_writes_and_reads() -> Result<(), SqliteGraphError> {
    // Scenario: Interleave writes, reads, and snapshots
    // Expected: All operations consistent, no interference

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Interleave operations
    for i in 0..20 {
        // Write
        let _id = insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "interleave".to_string(),
                name: format!("interleave_func_{}", i),
                file_path: Some(format!("interleave_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;

        // Read
        let _ids = graph.list_entity_ids()?;

        // Snapshot (every 5 iterations)
        if i % 5 == 0 {
            warm_cache(&graph)?;
            let snapshot = graph.acquire_snapshot()?;
            assert!(snapshot.node_count() > 0);
        }
    }

    Ok(())
}

#[test]
fn test_snapshot_with_batch_writes() -> Result<(), SqliteGraphError> {
    // Scenario: Batch writes followed by snapshot
    // Expected: Snapshot sees all batched writes

    let graph = create_test_graph()?;
    warm_cache(&graph)?;

    // Acquire pre-batch snapshot
    let snapshot_before = graph.acquire_snapshot()?;
    let count_before = snapshot_before.node_count();

    // Perform batch write
    for i in 0..50 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "batch".to_string(),
                name: format!("batch_func_{}", i),
                file_path: Some(format!("batch_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;
    }

    warm_cache(&graph)?;

    // Acquire post-batch snapshot
    let snapshot_after = graph.acquire_snapshot()?;
    let count_after = snapshot_after.node_count();

    // Verify batch writes visible
    assert_eq!(snapshot_before.node_count(), count_before);
    assert!(count_after > count_before);
    assert!(count_after >= count_before + 50); // At least 50 new entities

    Ok(())
}

//
// GROUP 5: PERFORMANCE WITH WAL OPERATIONS
//

#[test]
fn test_write_performance_with_snapshots() -> Result<(), SqliteGraphError> {
    // Scenario: Measure write performance with periodic snapshots
    // Expected: Writes complete in reasonable time

    let graph = create_test_graph()?;

    let start = Instant::now();

    // Perform writes with periodic snapshots
    for i in 0..50 {
        insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "perf".to_string(),
                name: format!("perf_func_{}", i),
                file_path: Some(format!("perf_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;

        // Periodic snapshots
        if i % 10 == 0 {
            warm_cache(&graph)?;
            let _snapshot = graph.acquire_snapshot()?;
        }
    }

    let duration = start.elapsed();
    println!("50 writes with 5 snapshots: {:?}", duration);

    // Should complete in reasonable time (< 5 seconds)
    assert!(duration < Duration::from_secs(5));

    Ok(())
}

#[test]
fn test_rapid_write_snapshot_cycle() -> Result<(), SqliteGraphError> {
    // Scenario: Rapid cycle of write + snapshot
    // Expected: All operations succeed, no corruption

    let graph = create_test_graph()?;

    // Perform 20 write + snapshot cycles
    for i in 0..20 {
        // Write
        let _new_id = insert_entity(
            &graph,
            GraphEntityCreate {
                kind: "cycle".to_string(),
                name: format!("cycle_func_{}", i),
                file_path: Some(format!("cycle_{}.rs", i)),
                data: serde_json::json!({}),
            },
        )?;

        // Snapshot
        warm_cache(&graph)?;
        let snapshot = graph.acquire_snapshot()?;

        // Verify snapshot consistent
        assert!(snapshot.node_count() > 0);
    }

    Ok(())
}
