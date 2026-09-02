//! V3 Backend Algorithm Integration Tests
//!
//! These tests verify that V3 Backend GraphBackend operations work correctly
//! for the primitive operations that graph algorithms depend on:
//! - entity_ids() - enumerate all nodes
//! - neighbors() - traverse edges with direction filtering
//! - insert_node() / insert_edge() - build test graphs

use crate::backend::native::v3::{
    PersistentHeaderV3, V3_FORMAT_VERSION, V3_HEADER_SIZE, V3Backend,
};

/// Verify that key V3 module exports are available
#[test]
fn test_v3_module_exports() {
    // Verify key exports are available
    let _header = PersistentHeaderV3::new_v3();
    assert_eq!(V3_HEADER_SIZE, 112);
    assert_eq!(V3_FORMAT_VERSION, 4);
}
use crate::backend::GraphBackend;
use crate::backend::sqlite::types::{BackendDirection, EdgeSpec, NeighborQuery, NodeSpec};
use tempfile::TempDir;

/// Test that V3 backend can create nodes and enumerate them
#[test]
fn test_v3_entity_ids_basic() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Initially empty
    let ids = backend.entity_ids().unwrap();
    assert!(ids.is_empty(), "New database should have no entities");

    // Insert nodes
    let node1 = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node2 = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Verify enumeration
    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), 2, "Should have 2 entities");
    assert!(ids.contains(&node1), "Should contain node1");
    assert!(ids.contains(&node2), "Should contain node2");
}

/// Test that V3 backend can create edges and traverse them
#[test]
fn test_v3_outgoing_edges() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Create a chain: A -> B -> C
    let node_a = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_b = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_c = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "C".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Insert edges
    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_b,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node_b,
            to: node_c,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Test outgoing edges
    let snapshot = crate::snapshot::SnapshotId::current();
    let outgoing_a = backend
        .neighbors(
            snapshot,
            node_a,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(outgoing_a.len(), 1, "A should have 1 outgoing edge");
    assert!(outgoing_a.contains(&node_b), "A should point to B");

    let outgoing_b = backend
        .neighbors(
            snapshot,
            node_b,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(outgoing_b.len(), 1, "B should have 1 outgoing edge");
    assert!(outgoing_b.contains(&node_c), "B should point to C");

    let outgoing_c = backend
        .neighbors(
            snapshot,
            node_c,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert!(outgoing_c.is_empty(), "C should have no outgoing edges");
}

/// Test incoming edges (reverse traversal)
#[test]
fn test_v3_incoming_edges() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Create a star: A -> B, A -> C, A -> D
    let node_a = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_b = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_c = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "C".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_d = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "D".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Insert edges from A to B, C, D
    for target in [node_b, node_c, node_d] {
        backend
            .insert_edge(EdgeSpec {
                from: node_a,
                to: target,
                edge_type: "links_to".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    // Test incoming edges
    let snapshot = crate::snapshot::SnapshotId::current();

    let incoming_b = backend
        .neighbors(
            snapshot,
            node_b,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(incoming_b.len(), 1, "B should have 1 incoming edge");
    assert!(incoming_b.contains(&node_a), "B should be pointed to by A");

    let incoming_c = backend
        .neighbors(
            snapshot,
            node_c,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(incoming_c.len(), 1, "C should have 1 incoming edge");
    assert!(incoming_c.contains(&node_a), "C should be pointed to by A");

    let incoming_a = backend
        .neighbors(
            snapshot,
            node_a,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert!(incoming_a.is_empty(), "A should have no incoming edges");
}

/// Test k-hop traversal (used by many algorithms)
#[test]
fn test_v3_k_hop_traversal() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Create a binary tree: A -> B, A -> C, B -> D, B -> E
    let node_a = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_b = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_c = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "C".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_d = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "D".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_e = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "E".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Insert edges
    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_b,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_c,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node_b,
            to: node_d,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node_b,
            to: node_e,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Test k-hop from A
    let snapshot = crate::snapshot::SnapshotId::current();

    // k_hop returns all nodes within k hops (not exactly at k hops)
    // 1-hop: B, C (nodes within 1 hop)
    let hop1 = backend
        .k_hop(snapshot, node_a, 1, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop1.len(), 2, "A should have 2 nodes within 1 hop");
    assert!(hop1.contains(&node_b), "B should be within 1 hop from A");
    assert!(hop1.contains(&node_c), "C should be within 1 hop from A");

    // 2-hop: B, C, D, E (nodes within 2 hops, includes 1-hop neighbors)
    let hop2 = backend
        .k_hop(snapshot, node_a, 2, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop2.len(), 4, "A should have 4 nodes within 2 hops");
    assert!(hop2.contains(&node_b), "B should be within 2 hops from A");
    assert!(hop2.contains(&node_c), "C should be within 2 hops from A");
    assert!(hop2.contains(&node_d), "D should be within 2 hops from A");
    assert!(hop2.contains(&node_e), "E should be within 2 hops from A");
}

/// Test node degree (incoming + outgoing counts)
#[test]
fn test_v3_node_degree() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Create a bidirectional link: A <-> B
    let node_a = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let node_b = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // A -> B
    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_b,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // B -> A (making it bidirectional)
    backend
        .insert_edge(EdgeSpec {
            from: node_b,
            to: node_a,
            edge_type: "links_to".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Check degrees
    let snapshot = crate::snapshot::SnapshotId::current();

    let (out_a, in_a) = backend.node_degree(snapshot, node_a).unwrap();
    assert_eq!(out_a, 1, "A should have 1 outgoing edge");
    assert_eq!(in_a, 1, "A should have 1 incoming edge");

    let (out_b, in_b) = backend.node_degree(snapshot, node_b).unwrap();
    assert_eq!(out_b, 1, "B should have 1 outgoing edge");
    assert_eq!(in_b, 1, "B should have 1 incoming edge");
}

/// Test persistence - database file is created and can be reopened
///
/// NOTE: Full persistence (loading nodes after reopen) requires NodeStore
/// disk loading which is not yet implemented. This test verifies basic
/// file creation and header persistence.
#[test]
fn test_v3_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();
        let id = backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "Persistent".to_string(),
                file_path: None,
                data: serde_json::json!({"key": "value"}),
            })
            .unwrap();

        // Verify before close - node exists with correct ID
        let snapshot = crate::snapshot::SnapshotId::current();
        let node = backend.get_node(snapshot, id).unwrap();
        assert_eq!(node.id, id, "Node ID should match");
    }

    // Verify file was created
    assert!(db_path.exists(), "Database file should exist after create");

    // Reopen database - header should be valid
    let backend = V3Backend::open(&db_path).unwrap();
    let header = backend.header();
    assert_eq!(
        header.magic,
        crate::backend::native::v3::V3_MAGIC,
        "Header magic should be valid"
    );
    assert_eq!(
        header.version,
        crate::backend::native::v3::V3_FORMAT_VERSION,
        "Header version should be valid"
    );
    // NOTE: NodeStore disk loading not yet implemented - cannot verify node data after reopen
}

// ============================================================================
// Algorithm Integration Tests
// ============================================================================

/// Helper: Create a chain graph and return node IDs
fn create_chain_graph(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("chain_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap(),
        );
    }
    for i in 0..n - 1 {
        backend
            .insert_edge(EdgeSpec {
                from: ids[i],
                to: ids[i + 1],
                edge_type: "NEXT".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    ids
}

/// Test: BFS (via k_hop) reaches all nodes in a chain
#[test]
fn test_v3_bfs_chain_reaches_all() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let ids = create_chain_graph(&backend, 10);
    let snapshot = crate::snapshot::SnapshotId::current();

    // k_hop with depth 20 should reach all 9 edges (9 reachable nodes)
    let reachable = backend
        .k_hop(snapshot, ids[0], 20, BackendDirection::Outgoing)
        .unwrap();

    assert_eq!(
        reachable.len(),
        9,
        "BFS from first node should reach all 9 other nodes in chain"
    );
    // Verify they're in order
    for (i, id) in ids.iter().enumerate().skip(1).take(9) {
        assert!(reachable.contains(id), "Should reach node at index {}", i);
    }
}

/// Test: BFS stays within one component for disconnected chains
#[test]
fn test_v3_bfs_disconnected_stays_in_component() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    // Create two disconnected chains: A->B->C and D->E->F
    let chain1 = create_chain_graph(&backend, 3);
    let chain2 = create_chain_graph(&backend, 3);
    let snapshot = crate::snapshot::SnapshotId::current();

    // BFS from chain1 start should only reach chain1
    let reachable = backend
        .k_hop(snapshot, chain1[0], 10, BackendDirection::Outgoing)
        .unwrap();

    assert_eq!(
        reachable.len(),
        2,
        "BFS should only reach 2 nodes in same component"
    );
    assert!(reachable.contains(&chain1[1]), "Should reach chain1[1]");
    assert!(reachable.contains(&chain1[2]), "Should reach chain1[2]");
    assert!(
        !reachable.contains(&chain2[0]),
        "Should NOT reach chain2[0]"
    );
}

/// Test: Star topology - center reaches all leaves
#[test]
fn test_v3_star_outgoing() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let center = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut leaves = Vec::new();
    for i in 0..5 {
        let leaf = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("leaf_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: leaf,
                edge_type: "CONNECTS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
        leaves.push(leaf);
    }

    let snapshot = crate::snapshot::SnapshotId::current();

    // Outgoing from center should reach all leaves
    let outgoing = backend
        .neighbors(
            snapshot,
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(outgoing.len(), 5, "Center should have 5 outgoing edges");

    // Each leaf should have 1 incoming from center
    for leaf in &leaves {
        let incoming = backend
            .neighbors(
                snapshot,
                *leaf,
                NeighborQuery {
                    direction: BackendDirection::Incoming,
                    edge_type: None,
                },
            )
            .unwrap();
        assert_eq!(incoming.len(), 1, "Each leaf should have 1 incoming edge");
        assert!(incoming.contains(&center), "Incoming should be from center");
    }
}

/// Test: Binary tree k-hop at different depths
#[test]
fn test_v3_binary_tree_k_hop() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    // Create binary tree of depth 3: 7 nodes (1 + 2 + 4)
    let mut ids = Vec::new();
    for i in 0..7 {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("bt_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap(),
        );
    }
    // Edges: 0->1, 0->2, 1->3, 1->4, 2->5, 2->6
    let edges = [(0, 1), (0, 2), (1, 3), (1, 4), (2, 5), (2, 6)];
    for (from, to) in edges {
        backend
            .insert_edge(EdgeSpec {
                from: ids[from],
                to: ids[to],
                edge_type: "CHILD".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    let snapshot = crate::snapshot::SnapshotId::current();

    // k=1 from root: nodes 1, 2 (2 children)
    let hop1 = backend
        .k_hop(snapshot, ids[0], 1, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop1.len(), 2, "k=1 should reach 2 children");

    // k=2 from root: nodes 1, 2, 3, 4, 5, 6 (2 + 4 = 6)
    let hop2 = backend
        .k_hop(snapshot, ids[0], 2, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop2.len(), 6, "k=2 should reach all 6 descendants");

    // k=3 from root: same as k=2 (no more nodes)
    let hop3 = backend
        .k_hop(snapshot, ids[0], 3, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop3.len(), 6, "k=3 should still reach 6 (no deeper nodes)");
}

/// Test: Node degree correctness
#[test]
fn test_v3_node_degree_complex() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    // Create a diamond: A -> B, A -> C, B -> D, C -> D
    let a = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let b = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let c = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "C".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let d = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "D".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: a,
            to: b,
            edge_type: "E".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: a,
            to: c,
            edge_type: "E".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: b,
            to: d,
            edge_type: "E".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: c,
            to: d,
            edge_type: "E".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let snapshot = crate::snapshot::SnapshotId::current();

    // A: out=2, in=0
    let (out_a, in_a) = backend.node_degree(snapshot, a).unwrap();
    assert_eq!(out_a, 2, "A should have 2 outgoing");
    assert_eq!(in_a, 0, "A should have 0 incoming");

    // D: out=0, in=2
    let (out_d, in_d) = backend.node_degree(snapshot, d).unwrap();
    assert_eq!(out_d, 0, "D should have 0 outgoing");
    assert_eq!(in_d, 2, "D should have 2 incoming");

    // B: out=1, in=1
    let (out_b, in_b) = backend.node_degree(snapshot, b).unwrap();
    assert_eq!(out_b, 1, "B should have 1 outgoing");
    assert_eq!(in_b, 1, "B should have 1 incoming");
}

/// Test: Edge type filtering
#[test]
fn test_v3_edge_type_filtering() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let node_a = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "A".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let node_b = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "B".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let node_c = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "C".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_b,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: node_a,
            to: node_c,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let snapshot = crate::snapshot::SnapshotId::current();

    // All outgoing from A
    let all = backend
        .neighbors(
            snapshot,
            node_a,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(all.len(), 2, "A should have 2 outgoing edges total");

    // Filter by CALLS
    let calls = backend
        .neighbors(
            snapshot,
            node_a,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(calls.len(), 1, "A should have 1 CALLS edge");
    assert!(calls.contains(&node_b), "CALLS edge should point to B");

    // Filter by USES
    let uses = backend
        .neighbors(
            snapshot,
            node_a,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();
    assert_eq!(uses.len(), 1, "A should have 1 USES edge");
    assert!(uses.contains(&node_c), "USES edge should point to C");
}

/// Test: Large graph operations (stress test)
#[test]
fn test_v3_large_graph_insert_and_query() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let n = 200;
    let mut ids = Vec::with_capacity(n);

    // Insert 200 nodes
    for i in 0..n {
        ids.push(
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("n_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap(),
        );
    }

    // Create chain edges
    for i in 0..n - 1 {
        backend
            .insert_edge(EdgeSpec {
                from: ids[i],
                to: ids[i + 1],
                edge_type: "NEXT".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    let snapshot = crate::snapshot::SnapshotId::current();

    // Verify entity_ids returns all 200
    let all_ids = backend.entity_ids().unwrap();
    assert_eq!(all_ids.len(), n, "Should have {} entities", n);

    // Verify first node reaches far in the chain
    let reachable = backend
        .k_hop(snapshot, ids[0], 100, BackendDirection::Outgoing)
        .unwrap();
    assert!(
        reachable.len() >= 100,
        "Should reach at least 100 nodes from start"
    );

    // Verify last node has no outgoing
    let last_outgoing = backend
        .neighbors(
            snapshot,
            ids[n - 1],
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert!(
        last_outgoing.is_empty(),
        "Last node should have no outgoing edges"
    );

    // Verify first node has no incoming
    let first_incoming = backend
        .neighbors(
            snapshot,
            ids[0],
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert!(
        first_incoming.is_empty(),
        "First node should have no incoming edges"
    );
}
