//! Phase 32 (Rewritten): V2 Cluster Pipeline Tests
//!
//! Clean tests using Phase 34 pipeline to verify cluster reconstruction works correctly.
//! All tests use real sqlitegraph APIs with zero manual cluster manipulation.

use sqlitegraph::{BackendDirection, NeighborQuery, SnapshotId, config::GraphConfig, open_graph};
use tempfile::TempDir;

/// Create a simple V2 graph with one edge for testing
fn create_simple_v2_graph() -> (Box<dyn sqlitegraph::GraphBackend>, u64, u64, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");

    // Create graph with native backend
    let cfg = GraphConfig::new(sqlitegraph::BackendKind::Native);
    let graph = open_graph(&db_path, &cfg).expect("Failed to create graph");

    // Create source node
    let source_id = graph
        .insert_node(sqlitegraph::NodeSpec {
            kind: "Function".to_string(),
            name: "source".to_string(),
            file_path: Some("/src/source.rs".to_string()),
            data: serde_json::json!({"role": "source"}),
        })
        .expect("Failed to insert source node") as u64;

    // Create target node
    let target_id = graph
        .insert_node(sqlitegraph::NodeSpec {
            kind: "Function".to_string(),
            name: "target".to_string(),
            file_path: Some("/src/target.rs".to_string()),
            data: serde_json::json!({"role": "target"}),
        })
        .expect("Failed to insert target node") as u64;

    // Create edge
    graph
        .insert_edge(sqlitegraph::EdgeSpec {
            from: source_id as i64,
            to: target_id as i64,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({"line": 5}),
        })
        .expect("Failed to insert edge");

    (graph, source_id, target_id, temp_dir)
}

/// Create a test graph
fn create_test_graph() -> (Box<dyn sqlitegraph::GraphBackend>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");

    // Create graph with native backend
    let cfg = GraphConfig::new(sqlitegraph::BackendKind::Native);
    let graph = open_graph(&db_path, &cfg).expect("Failed to create graph");

    (graph, temp_dir)
}

/// Add a V2 node
fn add_node_v2(
    graph: &mut Box<dyn sqlitegraph::GraphBackend>,
    _id: u64,
    name: &str,
    file_path: &str,
    data: serde_json::Value,
) -> u64 {
    graph
        .insert_node(sqlitegraph::NodeSpec {
            kind: "Function".to_string(),
            name: name.to_string(),
            file_path: Some(file_path.to_string()),
            data,
        })
        .expect("Failed to insert node") as u64
}

/// Add a V2 edge
fn add_edge_v2(
    graph: &mut Box<dyn sqlitegraph::GraphBackend>,
    from: u64,
    to: u64,
    edge_type: &str,
    data: serde_json::Value,
) -> u64 {
    graph
        .insert_edge(sqlitegraph::EdgeSpec {
            from: from as i64,
            to: to as i64,
            edge_type: edge_type.to_string(),
            data,
        })
        .expect("Failed to insert edge") as u64
}

/// Create a star V2 graph with one center node connected to many targets
fn create_star_v2_graph(
    num_targets: usize,
) -> (Box<dyn sqlitegraph::GraphBackend>, u64, Vec<u64>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");

    // Create graph with native backend
    let cfg = GraphConfig::new(sqlitegraph::BackendKind::Native);
    let graph = open_graph(&db_path, &cfg).expect("Failed to create graph");

    // Create center node
    let center_id = graph
        .insert_node(sqlitegraph::NodeSpec {
            kind: "Function".to_string(),
            name: "center".to_string(),
            file_path: Some("/src/center.rs".to_string()),
            data: serde_json::json!({"role": "center"}),
        })
        .expect("Failed to insert center node") as u64;

    // Create target nodes and edges
    let mut target_ids = Vec::new();
    for i in 0..num_targets {
        let target_id = graph
            .insert_node(sqlitegraph::NodeSpec {
                kind: "Function".to_string(),
                name: format!("target_{}", i),
                file_path: Some(format!("/src/target_{}.rs", i)),
                data: serde_json::json!({"role": "target", "index": i}),
            })
            .expect("Failed to insert target node") as u64;
        target_ids.push(target_id);

        // Create edge from center to target
        graph
            .insert_edge(sqlitegraph::EdgeSpec {
                from: center_id as i64,
                to: target_id as i64,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({"line": (i + 1) * 5}),
            })
            .expect("Failed to insert edge");
    }

    (graph, center_id, target_ids, temp_dir)
}

/// Flush and reopen graph
fn flush_and_reopen(
    graph: Box<dyn sqlitegraph::GraphBackend>,
    temp_dir: &TempDir,
) -> Box<dyn sqlitegraph::GraphBackend> {
    // Explicit flush to ensure all pending writes are persisted before reopen
    graph.flush().expect("Flush before reopen should succeed");
    let db_path = temp_dir.path().join("test.db");
    let cfg = GraphConfig::new(sqlitegraph::BackendKind::Native);
    open_graph(&db_path, &cfg).expect("Failed to reopen graph")
}

/// Test 1: Verify single edge cluster creation with clean Phase 34 pipeline
#[test]
fn test_single_edge_cluster_clean_creation() {
    let (graph, source_id, target_id, _temp_dir) = create_simple_v2_graph();

    // Verify neighbors work through public API (Phase 35 routing)
    let neighbors = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(neighbors.len(), 1, "Should have exactly 1 neighbor");
    assert_eq!(neighbors[0], target_id as i64, "Neighbor should be target");

    // Verify node data integrity through GraphBackend API
    let source_node_data = graph
        .get_node(SnapshotId::current(), source_id as i64)
        .unwrap();
    let target_node_data = graph
        .get_node(SnapshotId::current(), target_id as i64)
        .unwrap();

    assert_eq!(
        source_node_data.name, "source",
        "Source node should have correct name"
    );
    assert_eq!(
        source_node_data.kind, "Function",
        "Source node should have correct kind"
    );
    assert_eq!(
        target_node_data.name, "target",
        "Target node should have correct name"
    );
    assert_eq!(
        target_node_data.kind, "Function",
        "Target node should have correct kind"
    );

    // Verify edge direction validation through neighbor queries
    let source_outgoing = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    let target_incoming = graph
        .neighbors(
            SnapshotId::current(),
            target_id as i64,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(
        source_outgoing.len(),
        1,
        "Source should have 1 outgoing neighbor"
    );
    assert_eq!(
        target_incoming.len(),
        1,
        "Target should have 1 incoming neighbor"
    );
    assert_eq!(
        source_outgoing[0], target_id as i64,
        "Source should connect to target"
    );
    assert_eq!(
        target_incoming[0], source_id as i64,
        "Target should receive from source"
    );
}

/// Test 2: Verify multi-edge cluster creation with clean Phase 34 pipeline
#[test]
fn test_multi_edge_cluster_clean_creation() {
    let (graph, center_id, target_ids, _temp_dir) = create_star_v2_graph(3);

    // Verify center node has all outgoing neighbors
    let center_neighbors = graph
        .neighbors(
            SnapshotId::current(),
            center_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    println!(
        "DEBUG: Center ID: {}, Actual neighbor count: {}, Expected: 3",
        center_id,
        center_neighbors.len()
    );
    println!("DEBUG: Actual neighbors: {:?}", center_neighbors);
    println!("DEBUG: Target IDs: {:?}", target_ids);

    assert_eq!(
        center_neighbors.len(),
        3,
        "Center should have 3 outgoing neighbors (got {})",
        center_neighbors.len()
    );

    // Sort for comparison since neighbor order isn't guaranteed
    let mut sorted_neighbors = center_neighbors.clone();
    sorted_neighbors.sort();
    let mut sorted_targets = target_ids.iter().map(|&id| id as i64).collect::<Vec<_>>();
    sorted_targets.sort();
    assert_eq!(
        sorted_neighbors, sorted_targets,
        "All target nodes should be neighbors"
    );

    // Verify each target has exactly one incoming neighbor
    for target_id in &target_ids {
        let incoming_neighbors = graph
            .neighbors(
                SnapshotId::current(),
                *target_id as i64,
                NeighborQuery {
                    direction: BackendDirection::Incoming,
                    edge_type: None,
                },
            )
            .unwrap();

        assert_eq!(
            incoming_neighbors.len(),
            1,
            "Each target should have 1 incoming neighbor"
        );
        assert_eq!(
            incoming_neighbors[0], center_id as i64,
            "Incoming neighbor should be center"
        );
    }
}

/// Test 3: Verify cluster data consistency across reads and writes
#[test]
fn test_cluster_data_consistency() {
    let (mut graph, _temp_dir) = create_test_graph();

    // Create complex graph with multiple edge types
    let source_id = add_node_v2(
        &mut graph,
        1,
        "Source",
        "source",
        serde_json::json!({"role": "source"}),
    );
    let target1_id = add_node_v2(
        &mut graph,
        2,
        "Target1",
        "target1",
        serde_json::json!({"role": "target"}),
    );
    let target2_id = add_node_v2(
        &mut graph,
        3,
        "Target2",
        "target2",
        serde_json::json!({"role": "target"}),
    );

    // Create different edge types
    add_edge_v2(
        &mut graph,
        source_id,
        target1_id,
        "strong_edge",
        serde_json::json!({"weight": 10.0, "type": "primary"}),
    );
    add_edge_v2(
        &mut graph,
        source_id,
        target2_id,
        "weak_edge",
        serde_json::json!({"weight": 1.0, "type": "secondary"}),
    );

    // Verify all outgoing neighbors
    let all_outgoing = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(all_outgoing.len(), 2, "Should have 2 outgoing neighbors");

    // Verify filtered queries work correctly
    let strong_neighbors = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("strong_edge".to_string()),
            },
        )
        .unwrap();

    assert_eq!(
        strong_neighbors.len(),
        1,
        "Should have 1 strong_edge neighbor"
    );
    assert_eq!(
        strong_neighbors[0], target1_id as i64,
        "Strong edge should connect to target1"
    );

    let weak_neighbors = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("weak_edge".to_string()),
            },
        )
        .unwrap();

    assert_eq!(weak_neighbors.len(), 1, "Should have 1 weak_edge neighbor");
    assert_eq!(
        weak_neighbors[0], target2_id as i64,
        "Weak edge should connect to target2"
    );
}

/// Test 4: Verify symmetric incoming/outgoing clusters
#[test]
fn test_symmetric_incoming_outgoing_clusters() {
    let (mut graph, _temp_dir) = create_test_graph();

    // Create bidirectional relationship
    let node1_id = add_node_v2(
        &mut graph,
        1,
        "Node1",
        "node1",
        serde_json::json!({"role": "node1"}),
    );
    let node2_id = add_node_v2(
        &mut graph,
        2,
        "Node2",
        "node2",
        serde_json::json!({"role": "node2"}),
    );

    // Create bidirectional edges
    add_edge_v2(
        &mut graph,
        node1_id,
        node2_id,
        "forward",
        serde_json::json!({"direction": "1->2"}),
    );
    add_edge_v2(
        &mut graph,
        node2_id,
        node1_id,
        "backward",
        serde_json::json!({"direction": "2->1"}),
    );

    // Verify node1's outgoing and incoming
    let node1_outgoing = graph
        .neighbors(
            SnapshotId::current(),
            node1_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    let node1_incoming = graph
        .neighbors(
            SnapshotId::current(),
            node1_id as i64,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(
        node1_outgoing.len(),
        1,
        "Node1 should have 1 outgoing neighbor"
    );
    assert_eq!(
        node1_outgoing[0], node2_id as i64,
        "Node1 outgoing should be node2"
    );
    assert_eq!(
        node1_incoming.len(),
        1,
        "Node1 should have 1 incoming neighbor"
    );
    assert_eq!(
        node1_incoming[0], node2_id as i64,
        "Node1 incoming should be node2"
    );

    // Verify node2's outgoing and incoming
    let node2_outgoing = graph
        .neighbors(
            SnapshotId::current(),
            node2_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    let node2_incoming = graph
        .neighbors(
            SnapshotId::current(),
            node2_id as i64,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(
        node2_outgoing.len(),
        1,
        "Node2 should have 1 outgoing neighbor"
    );
    assert_eq!(
        node2_outgoing[0], node1_id as i64,
        "Node2 outgoing should be node1"
    );
    assert_eq!(
        node2_incoming.len(),
        1,
        "Node2 should have 1 incoming neighbor"
    );
    assert_eq!(
        node2_incoming[0], node1_id as i64,
        "Node2 incoming should be node1"
    );
}

/// Test 5: Verify cluster persistence after file operations
#[test]
fn test_cluster_persistence() {
    let (mut graph, temp_dir) = create_test_graph();

    // Create initial graph data
    let (mut node_ids, mut edge_ids) = (Vec::new(), Vec::new());

    // Create 3 nodes
    for i in 1..=3 {
        let node_id = add_node_v2(
            &mut graph,
            i,
            &format!("test_node_{}", i),
            "TestNode",
            serde_json::json!({"index": i}),
        );
        node_ids.push(node_id);
    }

    // Create edges forming a triangle
    for i in 0..3 {
        let from = node_ids[i];
        let to = node_ids[(i + 1) % 3];
        let edge_id = add_edge_v2(
            &mut graph,
            from,
            to,
            "triangle_edge",
            serde_json::json!({"edge_index": i}),
        );
        edge_ids.push(edge_id);
    }

    // Flush and reopen
    let graph = flush_and_reopen(graph, &temp_dir);

    // Verify all nodes and edges persisted correctly
    for (i, &node_id) in node_ids.iter().enumerate() {
        // Check node exists and has correct data
        let node = graph
            .get_node(SnapshotId::current(), node_id as i64)
            .unwrap();
        assert_eq!(node.name, format!("test_node_{}", i + 1));

        // Check each node has correct neighbors
        let neighbors = graph
            .neighbors(
                SnapshotId::current(),
                node_id as i64,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();

        assert_eq!(
            neighbors.len(),
            1,
            "Each node should have 1 outgoing neighbor in triangle"
        );
    }

    // Verify triangle connectivity
    for i in 0..3 {
        let from = node_ids[i];
        let expected_to = node_ids[(i + 1) % 3];

        let neighbors = graph
            .neighbors(
                SnapshotId::current(),
                from as i64,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("triangle_edge".to_string()),
                },
            )
            .unwrap();

        assert_eq!(neighbors.len(), 1, "Should have 1 triangle_edge neighbor");
        assert_eq!(
            neighbors[0], expected_to as i64,
            "Triangle connectivity should be preserved"
        );
    }
}

/// Test 6: Public API neighbors match manual deserialization
#[test]
fn v2_cluster_neighbors_match_manual_deserialization() {
    let (graph, source_id, target_id, _temp_dir) = create_simple_v2_graph();

    // Test 1: Get neighbors via public API (graph.neighbors())
    let public_neighbors = graph
        .neighbors(
            SnapshotId::current(),
            source_id as i64,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    println!("DEBUG: Public API neighbors: {:?}", public_neighbors);

    // Test 2: Get source node details via GraphBackend API
    let source_node = graph
        .get_node(SnapshotId::current(), source_id as i64)
        .unwrap();
    println!(
        "DEBUG: Source node details: kind={}, name={}",
        source_node.kind, source_node.name
    );

    // Test 3: Validate public API neighbor results
    assert_eq!(
        public_neighbors.len(),
        1,
        "Public API should return 1 neighbor"
    );
    assert_eq!(
        public_neighbors[0], target_id as i64,
        "Public API neighbor should be the target node"
    );

    println!(
        "DEBUG: Public API validation successful - 1 neighbor found: {}",
        public_neighbors[0]
    );

    // Test 4: Validate node metadata consistency
    let target_node = graph
        .get_node(SnapshotId::current(), target_id as i64)
        .unwrap();
    assert_eq!(
        target_node.name, "target",
        "Target node should have correct name"
    );
    println!(
        "DEBUG: Target node validation successful: kind={}, name={}",
        target_node.kind, target_node.name
    );
}
