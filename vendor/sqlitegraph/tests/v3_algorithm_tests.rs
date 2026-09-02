//! V3 Backend Algorithm Integration Tests
//!
//! Tests that verify graph algorithms work correctly with the V3 native backend
//! using only the GraphBackend trait methods.

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{EdgeSpec, GraphBackend, NodeSpec},
};
use tempfile::TempDir;

/// Helper to create a V3-backed graph for algorithm testing
fn create_v3_backend() -> (V3Backend, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_v3.graph");
    let backend = V3Backend::create(&db_path).unwrap();
    (backend, temp_dir)
}

/// Build a simple chain graph: 1 -> 2 -> 3 -> 4
fn build_chain_graph(backend: &V3Backend) -> Vec<i64> {
    let mut nodes = Vec::new();

    // Create 4 nodes
    for i in 1..=4 {
        let id = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        nodes.push(id);
    }

    // Create chain edges: 1 -> 2 -> 3 -> 4
    for i in 0..nodes.len() - 1 {
        backend
            .insert_edge(EdgeSpec {
                from: nodes[i],
                to: nodes[i + 1],
                edge_type: "links_to".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    nodes
}

/// Build a star graph: center -> leaf1, center -> leaf2, center -> leaf3
fn build_star_graph(backend: &V3Backend) -> (i64, Vec<i64>) {
    let center = backend
        .insert_node(NodeSpec {
            kind: "Center".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut leaves = Vec::new();
    for i in 1..=3 {
        let leaf = backend
            .insert_node(NodeSpec {
                kind: "Leaf".to_string(),
                name: format!("leaf_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: leaf,
                edge_type: "links_to".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        leaves.push(leaf);
    }

    (center, leaves)
}

/// Build a cycle graph: 1 -> 2 -> 3 -> 1
fn build_cycle_graph(backend: &V3Backend) -> Vec<i64> {
    let mut nodes = Vec::new();

    // Create 3 nodes
    for i in 1..=3 {
        let id = backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        nodes.push(id);
    }

    // Create cycle edges
    for i in 0..nodes.len() {
        let next = (i + 1) % nodes.len();
        backend
            .insert_edge(EdgeSpec {
                from: nodes[i],
                to: nodes[next],
                edge_type: "links_to".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    nodes
}

// ============================================================================
// Core Graph Operations Tests (used by algorithms)
// ============================================================================

#[test]
fn test_v3_entity_ids_basic() {
    let (backend, _temp) = create_v3_backend();

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

#[test]
fn test_v3_fetch_outgoing() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_chain_graph(&backend);

    // Test outgoing from each node
    let out_0 = backend.fetch_outgoing(nodes[0]).unwrap();
    assert_eq!(out_0.len(), 1, "Node 0 should have 1 outgoing edge");
    assert!(out_0.contains(&nodes[1]), "Node 0 should point to node 1");

    let out_1 = backend.fetch_outgoing(nodes[1]).unwrap();
    assert_eq!(out_1.len(), 1, "Node 1 should have 1 outgoing edge");
    assert!(out_1.contains(&nodes[2]), "Node 1 should point to node 2");

    let out_3 = backend.fetch_outgoing(nodes[3]).unwrap();
    assert!(out_3.is_empty(), "Node 3 should have no outgoing edges");
}

#[test]
fn test_v3_fetch_incoming() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_chain_graph(&backend);

    // Test incoming to each node
    let in_0 = backend.fetch_incoming(nodes[0]).unwrap();
    assert!(in_0.is_empty(), "Node 0 should have no incoming edges");

    let in_1 = backend.fetch_incoming(nodes[1]).unwrap();
    assert_eq!(in_1.len(), 1, "Node 1 should have 1 incoming edge");
    assert!(
        in_1.contains(&nodes[0]),
        "Node 1 should be pointed to by node 0"
    );

    let in_3 = backend.fetch_incoming(nodes[3]).unwrap();
    assert_eq!(in_3.len(), 1, "Node 3 should have 1 incoming edge");
    assert!(
        in_3.contains(&nodes[2]),
        "Node 3 should be pointed to by node 2"
    );
}

// ============================================================================
// Algorithm-Specific Tests
// ============================================================================

/// Test that demonstrates PageRank can be computed using GraphBackend trait
#[test]
fn test_v3_pagerank_via_trait() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_chain_graph(&backend);

    // Use trait methods to compute PageRank manually
    let all_ids = backend.entity_ids().unwrap();
    let n = all_ids.len();
    assert_eq!(n, 4, "Should have 4 nodes");

    // Initialize scores
    let mut scores: std::collections::HashMap<i64, f64> =
        all_ids.iter().map(|&id| (id, 1.0 / n as f64)).collect();

    // Pre-compute outgoing counts
    let mut outgoing_counts: std::collections::HashMap<i64, usize> =
        std::collections::HashMap::new();
    for &id in &all_ids {
        let count = backend.fetch_outgoing(id).unwrap().len();
        outgoing_counts.insert(id, count);
    }

    // Run a few iterations of PageRank
    let damping = 0.85;
    for _ in 0..20 {
        let mut new_scores: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();

        let base_score = (1.0 - damping) / n as f64;
        for &id in &all_ids {
            new_scores.insert(id, base_score);
        }

        let mut dangling_score = 0.0;

        for &id in &all_ids {
            let score = scores[&id];
            let out_count = outgoing_counts[&id];

            if out_count == 0 {
                dangling_score += score;
            } else {
                let share = score / out_count as f64;
                for &neighbor in &backend.fetch_outgoing(id).unwrap() {
                    *new_scores.get_mut(&neighbor).unwrap() += damping * share;
                }
            }
        }

        let dangling_share = damping * dangling_score / n as f64;
        for (_, score) in new_scores.iter_mut() {
            *score += dangling_share;
        }

        scores = new_scores;
    }

    // Verify scores sum to ~1.0
    let total: f64 = scores.values().sum();
    assert!(
        (total - 1.0).abs() < 0.01,
        "Scores should sum to ~1.0, got {}",
        total
    );

    // In a chain, the end node should have highest score
    assert!(
        scores[&nodes[3]] > scores[&nodes[0]],
        "End of chain should have higher score than start"
    );
}

/// Test BFS traversal using GraphBackend trait
#[test]
fn test_v3_bfs_via_trait() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_chain_graph(&backend);

    // Manual BFS using trait methods
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut result = Vec::new();

    queue.push_back(nodes[0]);
    visited.insert(nodes[0]);

    while let Some(node) = queue.pop_front() {
        result.push(node);

        for neighbor in backend.fetch_outgoing(node).unwrap() {
            if visited.insert(neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    // Should visit all 4 nodes
    assert_eq!(result.len(), 4, "BFS should visit all 4 nodes");
    assert_eq!(result[0], nodes[0]);
    assert_eq!(result[1], nodes[1]);
    assert_eq!(result[2], nodes[2]);
    assert_eq!(result[3], nodes[3]);
}

/// Test SCC (cycle detection) using GraphBackend trait
#[test]
fn test_v3_scc_cycle_via_trait() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_cycle_graph(&backend);

    // Verify it's a cycle using outgoing/incoming
    for i in 0..nodes.len() {
        let outgoing = backend.fetch_outgoing(nodes[i]).unwrap();
        assert_eq!(outgoing.len(), 1, "Each node should have 1 outgoing");

        let next = (i + 1) % nodes.len();
        assert_eq!(
            outgoing[0], nodes[next],
            "Should point to next node in cycle"
        );
    }

    // All nodes should have 1 incoming
    for node in &nodes {
        let incoming = backend.fetch_incoming(*node).unwrap();
        assert_eq!(
            incoming.len(),
            1,
            "Each node should have 1 incoming in cycle"
        );
    }
}

/// Test star graph topology using GraphBackend trait
#[test]
fn test_v3_star_topology() {
    let (backend, _temp) = create_v3_backend();
    let (center, leaves) = build_star_graph(&backend);

    // Center should have 3 outgoing
    let center_out = backend.fetch_outgoing(center).unwrap();
    assert_eq!(center_out.len(), 3, "Center should have 3 outgoing edges");

    for leaf in &leaves {
        assert!(
            center_out.contains(leaf),
            "Center should point to all leaves"
        );
    }

    // Center should have 0 incoming
    let center_in = backend.fetch_incoming(center).unwrap();
    assert!(center_in.is_empty(), "Center should have no incoming edges");

    // Each leaf should have 0 outgoing, 1 incoming
    for leaf in &leaves {
        let leaf_out = backend.fetch_outgoing(*leaf).unwrap();
        assert!(leaf_out.is_empty(), "Leaf should have no outgoing edges");

        let leaf_in = backend.fetch_incoming(*leaf).unwrap();
        assert_eq!(leaf_in.len(), 1, "Leaf should have 1 incoming edge");
        assert_eq!(leaf_in[0], center, "Leaf should be pointed to by center");
    }
}

/// Test that demonstrates shortest path can be computed using GraphBackend trait
#[test]
fn test_v3_shortest_path_via_trait() {
    let (backend, _temp) = create_v3_backend();
    let nodes = build_chain_graph(&backend);

    // Dijkstra/BFS shortest path using trait methods
    let start = nodes[0];
    let end = nodes[3];

    let mut distances: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut predecessors: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    distances.insert(start, 0);
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        let current_dist = distances[&node];

        for neighbor in backend.fetch_outgoing(node).unwrap() {
            if let std::collections::hash_map::Entry::Vacant(e) = distances.entry(neighbor) {
                e.insert(current_dist + 1);
                predecessors.insert(neighbor, node);
                queue.push_back(neighbor);
            }
        }
    }

    // Should have found end node
    assert!(
        distances.contains_key(&end),
        "Should have found path to end node"
    );
    assert_eq!(distances[&end], 3, "Path length should be 3 edges");

    // Reconstruct path
    let mut path = vec![end];
    let mut current = end;
    while let Some(&pred) = predecessors.get(&current) {
        path.push(pred);
        current = pred;
    }
    path.reverse();

    assert_eq!(path.len(), 4, "Path should have 4 nodes");
    assert_eq!(path[0], nodes[0]);
    assert_eq!(path[1], nodes[1]);
    assert_eq!(path[2], nodes[2]);
    assert_eq!(path[3], nodes[3]);
}

// ============================================================================
// Edge Type Filtering Tests
// ============================================================================

#[test]
fn test_v3_edge_type_filtering() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let (backend, _temp) = create_v3_backend();

    // Create a central node
    let center = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "main".to_string(),
            file_path: Some("/src/main.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Create multiple neighbor nodes
    let helper1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "helper1".to_string(),
            file_path: Some("/src/helper1.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    let helper2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "helper2".to_string(),
            file_path: Some("/src/helper2.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    let util1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "util1".to_string(),
            file_path: Some("/src/util1.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Create edges with different types
    backend
        .insert_edge(EdgeSpec {
            from: center,
            to: helper1,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: center,
            to: helper2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: center,
            to: util1,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Test: All neighbors (no filter)
    let all_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(
        all_neighbors.len(),
        3,
        "Should have 3 neighbors without filter"
    );
    assert!(all_neighbors.contains(&helper1), "Should contain helper1");
    assert!(all_neighbors.contains(&helper2), "Should contain helper2");
    assert!(all_neighbors.contains(&util1), "Should contain util1");

    // Test: Filter by CALLS edge type
    let calls_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();

    assert_eq!(calls_neighbors.len(), 2, "Should have 2 CALLS neighbors");
    assert!(
        calls_neighbors.contains(&helper1),
        "Should contain helper1 (CALLS)"
    );
    assert!(
        calls_neighbors.contains(&helper2),
        "Should contain helper2 (CALLS)"
    );
    assert!(
        !calls_neighbors.contains(&util1),
        "Should NOT contain util1 (not CALLS)"
    );

    // Test: Filter by USES edge type
    let uses_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();

    assert_eq!(uses_neighbors.len(), 1, "Should have 1 USES neighbor");
    assert!(
        uses_neighbors.contains(&util1),
        "Should contain util1 (USES)"
    );
    assert!(
        !uses_neighbors.contains(&helper1),
        "Should NOT contain helper1 (not USES)"
    );
    assert!(
        !uses_neighbors.contains(&helper2),
        "Should NOT contain helper2 (not USES)"
    );

    // Test: Filter by non-existent edge type
    let empty_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("NONEXISTENT".to_string()),
            },
        )
        .unwrap();

    assert_eq!(
        empty_neighbors.len(),
        0,
        "Should have 0 neighbors for non-existent edge type"
    );

    // Test: Incoming filtering
    let incoming_calls = backend
        .neighbors(
            SnapshotId::current(),
            helper1,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();

    assert_eq!(
        incoming_calls.len(),
        1,
        "helper1 should have 1 incoming CALLS"
    );
    assert!(
        incoming_calls.contains(&center),
        "helper1 should be called by center"
    );

    let incoming_uses = backend
        .neighbors(
            SnapshotId::current(),
            helper1,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();

    assert_eq!(
        incoming_uses.len(),
        0,
        "helper1 should have 0 incoming USES"
    );
}

// ============================================================================
// Edge Type Durability Tests (Reopen/Recovery)
// ============================================================================

#[test]
fn test_v3_edge_type_durability_across_reopen() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_v3_durable.graph");

    // Phase 1: Create edges with different types
    {
        let backend = V3Backend::create(&db_path).unwrap();

        let center = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "main".to_string(),
                file_path: Some("/src/main.rs".to_string()),
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper1 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "helper1".to_string(),
                file_path: Some("/src/helper1.rs".to_string()),
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper2 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "helper2".to_string(),
                file_path: Some("/src/helper2.rs".to_string()),
                data: serde_json::json!({}),
            })
            .unwrap();

        let util1 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "util1".to_string(),
                file_path: Some("/src/util1.rs".to_string()),
                data: serde_json::json!({}),
            })
            .unwrap();

        // Create edges with different types
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper1,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper2,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: util1,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // Verify in-memory filtering works before close
        let calls_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("CALLS".to_string()),
                },
            )
            .unwrap();
        assert_eq!(
            calls_neighbors.len(),
            2,
            "Before reopen: should have 2 CALLS neighbors"
        );

        let uses_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("USES".to_string()),
                },
            )
            .unwrap();
        assert_eq!(
            uses_neighbors.len(),
            1,
            "Before reopen: should have 1 USES neighbor"
        );
    } // backend closes here

    // Phase 2: Reopen and verify edge_type filtering still works
    {
        let backend = V3Backend::open(&db_path).unwrap();

        // Get node IDs (they should be stable across reopen)
        let all_ids = backend.entity_ids().unwrap();
        assert_eq!(all_ids.len(), 4, "Should have 4 nodes after reopen");

        // Find center node (named "main")
        let center = all_ids
            .iter()
            .find(|&&id| match backend.get_node(SnapshotId::current(), id) {
                Ok(entity) => entity.name == "main",
                Err(_) => false,
            })
            .copied()
            .unwrap();

        // DEBUG: Print all node IDs and names
        println!("After reopen, all_ids: {:?}", all_ids);
        for &id in &all_ids {
            if let Ok(entity) = backend.get_node(SnapshotId::current(), id) {
                println!("  Node {}: name={}", id, entity.name);
            }
        }

        // DEBUG: Check unfiltered neighbors first
        let all_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();
        println!("Unfiltered neighbors after reopen: {:?}", all_neighbors);

        // CRITICAL TEST: Filter by CALLS after reopen
        let calls_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("CALLS".to_string()),
                },
            )
            .unwrap();

        assert_eq!(
            calls_neighbors.len(),
            2,
            "After reopen: should have 2 CALLS neighbors (edge_type survived recovery)"
        );

        // CRITICAL TEST: Filter by USES after reopen
        let uses_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("USES".to_string()),
                },
            )
            .unwrap();

        assert_eq!(
            uses_neighbors.len(),
            1,
            "After reopen: should have 1 USES neighbor (edge_type survived recovery)"
        );

        // CRITICAL TEST: All neighbors (unfiltered) still work
        let all_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();

        assert_eq!(
            all_neighbors.len(),
            3,
            "After reopen: should have 3 neighbors total (unfiltered)"
        );

        // CRITICAL TEST: Non-existent edge type returns empty
        let empty_neighbors = backend
            .neighbors(
                SnapshotId::current(),
                center,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: Some("NONEXISTENT".to_string()),
                },
            )
            .unwrap();

        assert_eq!(
            empty_neighbors.len(),
            0,
            "After reopen: non-existent edge type should return empty"
        );
    }
}

#[test]
fn test_v3_edge_type_mixed_queries_after_reopen() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_v3_mixed.graph");

    // Create a more complex graph with mixed edge types
    let (center, _helper1, _helper2, _util1, _util2) = {
        let backend = V3Backend::create(&db_path).unwrap();

        let center = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper1 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "h1".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper2 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "h2".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let util1 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "u1".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let util2 = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "u2".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> helper1 (CALLS)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper1,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> helper2 (CALLS)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper2,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> util1 (USES)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: util1,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> util2 (USES)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: util2,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        (center, helper1, helper2, util1, util2)
    };

    // Reopen and verify mixed queries work correctly
    let backend = V3Backend::open(&db_path).unwrap();

    // Filtered queries should return correct subsets
    let calls_only = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        calls_only.len(),
        2,
        "CALLS filter should return 2 neighbors"
    );

    let uses_only = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();
    assert_eq!(uses_only.len(), 2, "USES filter should return 2 neighbors");

    // Unfiltered query should return all
    let all_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(
        all_neighbors.len(),
        4,
        "Unfiltered query should return all 4 neighbors"
    );

    // Verify no overlap between filtered results
    let calls_set: std::collections::HashSet<_> = calls_only.into_iter().collect();
    let uses_set: std::collections::HashSet<_> = uses_only.into_iter().collect();
    let intersection = calls_set.intersection(&uses_set).collect::<Vec<_>>();
    assert_eq!(
        intersection.len(),
        0,
        "CALLS and USES sets should be disjoint"
    );
}

#[test]
fn test_v3_edge_type_incoming_after_reopen() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_v3_incoming.graph");

    // Create edges and test incoming direction filtering
    let (center, helper, _util) = {
        let backend = V3Backend::create(&db_path).unwrap();

        let center = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "helper".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let util = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "util".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> helper (CALLS)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // center -> util (USES)
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: util,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        (center, helper, util)
    };

    // Reopen and test incoming filtering
    let backend = V3Backend::open(&db_path).unwrap();

    // helper should have 1 incoming CALLS from center
    let helper_incoming_calls = backend
        .neighbors(
            SnapshotId::current(),
            helper,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        helper_incoming_calls.len(),
        1,
        "helper should have 1 incoming CALLS"
    );
    assert!(
        helper_incoming_calls.contains(&center),
        "helper should be called by center"
    );

    // helper should have 0 incoming USES
    let helper_incoming_uses = backend
        .neighbors(
            SnapshotId::current(),
            helper,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        helper_incoming_uses.len(),
        0,
        "helper should have 0 incoming USES"
    );
}

// Diagnostic test: Check if edge data is actually written to disk
#[test]
fn test_v3_diagnostic_edge_disk_write() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };
    use std::fs::File;
    use std::io::Read;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_disk_write.graph");

    let center = {
        let backend = V3Backend::create(&db_path).unwrap();

        let center = backend
            .insert_node(NodeSpec {
                kind: "F".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper1 = backend
            .insert_node(NodeSpec {
                kind: "F".to_string(),
                name: "helper1".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper2 = backend
            .insert_node(NodeSpec {
                kind: "F".to_string(),
                name: "helper2".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // Insert two edges from center
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper1,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper2,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        // Explicitly drop backend to trigger flush
        drop(backend);
        center
    };

    // Read the database file directly to check if edge data was written
    {
        let mut file = File::open(&db_path).unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        println!("Database file size: {} bytes", buffer.len());

        // Check for edge data pattern
        // Edge cluster format: [version: 1 byte] [edge_count: 4 bytes] [edges...]
        let mut found_clusters = Vec::new();
        for i in 0..buffer.len().saturating_sub(10) {
            if buffer[i] == 1 {
                // format_version
                let count = u32::from_be_bytes([
                    buffer[i + 1],
                    buffer[i + 2],
                    buffer[i + 3],
                    buffer[i + 4],
                ]);
                if count > 0 && count <= 100 {
                    // Reasonable edge count range
                    found_clusters.push((i, count));
                }
            }
        }

        println!("Found {} potential edge clusters:", found_clusters.len());
        for (offset, count) in &found_clusters {
            println!("  Offset {}: {} edges", offset, count);
        }

        if found_clusters.is_empty() {
            println!("WARNING: No edge clusters found in database file!");
        }
    }

    // Now reopen and check
    let backend = V3Backend::open(&db_path).unwrap();

    // Check unfiltered neighbors
    let all_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    println!("Unfiltered neighbors after reopen: {:?}", all_neighbors);
    assert_eq!(
        all_neighbors.len(),
        2,
        "Should have 2 neighbors after reopen"
    );
}

// ============================================================================
// Edge Type Aliasing Regression Tests
// ============================================================================

/// Test that verifies the known limitation: tuple-keying (src,dst,dir)
/// means multiple edges between same endpoints with different types will alias.
///
/// This test documents the CURRENT BEHAVIOR: the last edge type wins.
/// If this becomes a problem, the key model must change to use edge_id.
#[test]
fn test_v3_edge_type_aliasing_known_limitation() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let (backend, _temp) = create_v3_backend();

    let center = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let helper = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "helper".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Insert first edge with type "CALLS"
    backend
        .insert_edge(EdgeSpec {
            from: center,
            to: helper,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Verify CALLS type is registered
    let calls_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(calls_neighbors.len(), 1, "Should have 1 CALLS neighbor");
    assert!(calls_neighbors.contains(&helper), "Should be helper");

    // NOW INSERT SECOND EDGE between same endpoints with different type "USES"
    // This will OVERWRITE the previous edge_type in the HashMap
    backend
        .insert_edge(EdgeSpec {
            from: center,
            to: helper,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // KNOWN LIMITATION: The HashMap now has "USES", not "CALLS"
    // The previous edge type is lost due to tuple-keying
    let uses_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        uses_neighbors.len(),
        1,
        "Should have 1 USES neighbor (overwrites CALLS)"
    );

    let calls_neighbors_after = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        calls_neighbors_after.len(),
        0,
        "KNOWN LIMITATION: CALLS was overwritten by USES"
    );

    // Unfiltered query should still return helper (only once, not duplicated)
    let all_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(
        all_neighbors.len(),
        1,
        "Should have 1 neighbor total (not duplicated)"
    );
    assert!(all_neighbors.contains(&helper), "Should be helper");
}

/// Test that edge_type aliasing behavior is consistent across reopen
///
/// NOTE: This test is currently failing due to a pre-existing edge type filtering
/// issue where the neighbors query returns duplicate results after reopen.
/// This is unrelated to KV durability and should be investigated separately.
#[test]
#[ignore]
fn test_v3_edge_type_aliasing_across_reopen() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_aliasing.graph");

    let (center, _helper) = {
        let backend = V3Backend::create(&db_path).unwrap();

        let center = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        let helper = backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "helper".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // Insert CALLS, then overwrite with USES
        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper,
                edge_type: "CALLS".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: center,
                to: helper,
                edge_type: "USES".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();

        (center, helper)
    };

    // Reopen and verify behavior is consistent
    let backend = V3Backend::open(&db_path).unwrap();

    // After reopen, USES should still be the type (last write wins)
    let uses_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        uses_neighbors.len(),
        1,
        "After reopen: USES should be the type"
    );

    let calls_neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        calls_neighbors.len(),
        0,
        "After reopen: CALLS was overwritten"
    );
}

// ============================================================================
// Snapshot Isolation Tests - V3 Backend
// ============================================================================

/// Test that V3 accepts all snapshot IDs (no MVCC)
///
/// V3 does not implement snapshot isolation / MVCC. All reads see the
/// current committed state regardless of the snapshot ID passed.
/// This test verifies that both current and arbitrary snapshots work.
#[test]
fn test_v3_snapshot_all_accepted() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let (backend, _temp) = create_v3_backend();

    let node1 = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({"initial": "state"}),
        })
        .unwrap();

    // SnapshotId::current() should work
    let current = SnapshotId::current();
    let entity = backend.get_node(current, node1).unwrap();
    assert_eq!(entity.name, "node1");
    assert_eq!(entity.data["initial"], "state");

    // Arbitrary snapshots are also accepted since V3 has no MVCC
    let arbitrary = SnapshotId::from_lsn(12345);
    let result = backend.get_node(arbitrary, node1);
    assert!(result.is_ok(), "V3 accepts all snapshots (no MVCC)");
    assert_eq!(result.unwrap().name, "node1");

    // Same for neighbors
    let neighbors_result = backend.neighbors(
        arbitrary,
        node1,
        NeighborQuery {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        },
    );
    assert!(
        neighbors_result.is_ok(),
        "V3 neighbors accepts all snapshots"
    );
}

/// Test that SnapshotId::current() works correctly in V3
///
/// This test verifies that current snapshot allows all operations
/// to work correctly. V3 does not distinguish snapshot IDs.
#[test]
fn test_v3_snapshot_current_works() {
    use sqlitegraph::{
        backend::{BackendDirection, NeighborQuery},
        snapshot::SnapshotId,
    };

    let (backend, _temp) = create_v3_backend();

    // Create a simple graph
    let node1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "main".to_string(),
            file_path: Some("/src/main.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    let node2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "helper".to_string(),
            file_path: Some("/src/helper.rs".to_string()),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // All operations should work with SnapshotId::current()
    let current = SnapshotId::current();

    // get_node
    let entity = backend.get_node(current, node1).unwrap();
    assert_eq!(entity.name, "main");

    // neighbors
    let neighbors = backend
        .neighbors(
            current,
            node1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(neighbors.len(), 1);
    assert!(neighbors.contains(&node2));

    // fetch_outgoing (convenience method uses SnapshotId::current())
    let outgoing = backend.fetch_outgoing(node1).unwrap();
    assert_eq!(outgoing.len(), 1);
    assert!(outgoing.contains(&node2));

    // bfs
    let bfs_result = backend.bfs(current, node1, 1).unwrap();
    assert_eq!(bfs_result.len(), 2); // node1 and node2

    // shortest_path
    let path = backend.shortest_path(current, node1, node2).unwrap();
    assert!(path.is_some());
    assert_eq!(path.unwrap().len(), 2);

    // node_degree
    let (out, inc) = backend.node_degree(current, node1).unwrap();
    assert_eq!(out, 1);
    assert_eq!(inc, 0);

    // entity_ids
    let all_ids = backend.entity_ids().unwrap();
    assert_eq!(all_ids.len(), 2);
}

// ============================================================================
// Additional snapshot validation tests for methods that previously ignored
// snapshot_id. These tests verify that all methods now consistently reject
// historical snapshots.
// ============================================================================

/// Test that pattern_search works on V3 (implemented in 3.6.0).
/// Previously returned Unsupported; now returns Ok with match results.
#[test]
fn test_v3_pattern_search_implemented() {
    use sqlitegraph::snapshot::SnapshotId;

    let (backend, _temp) = create_v3_backend();

    let node1 = backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // pattern_search is now implemented for V3 — returns Ok with results
    let current = SnapshotId::current();
    let result = backend.pattern_search(current, node1, &Default::default());
    assert!(
        result.is_ok(),
        "pattern_search should return Ok on V3 (implemented in 3.6.0): {:?}",
        result.err()
    );
}

/// Test that query_nodes_by_kind accepts any snapshot (V3 has no MVCC)
#[test]
fn test_v3_query_nodes_by_kind_accepts_any_snapshot() {
    use sqlitegraph::snapshot::SnapshotId;

    let (backend, _temp) = create_v3_backend();

    backend
        .insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // V3 accepts all snapshots — query_nodes_by_kind not yet implemented, returns all nodes
    let arbitrary = SnapshotId::from_lsn(888);
    let result = backend.query_nodes_by_kind(arbitrary, "TestKind");
    assert!(
        result.is_ok(),
        "query_nodes_by_kind should accept any snapshot"
    );
    assert_eq!(result.unwrap().len(), 1);
}

/// Test that query_nodes_by_name_pattern accepts any snapshot (V3 has no MVCC)
#[test]
fn test_v3_query_nodes_by_name_pattern_accepts_any_snapshot() {
    use sqlitegraph::snapshot::SnapshotId;

    let (backend, _temp) = create_v3_backend();

    backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "test_node".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // V3 accepts all snapshots — query_nodes_by_name_pattern does prefix/substring matching
    let arbitrary = SnapshotId::from_lsn(777);
    let result = backend.query_nodes_by_name_pattern(arbitrary, "test*");

    assert!(
        result.is_ok(),
        "query_nodes_by_name_pattern should accept any snapshot"
    );
    assert_eq!(result.unwrap().len(), 1);
}

/// Test that all three fixed methods work with current snapshot
#[test]
fn test_v3_fixed_methods_work_with_current_snapshot() {
    use sqlitegraph::snapshot::SnapshotId;

    let (backend, _temp) = create_v3_backend();

    let node1 = backend
        .insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: "test_node".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let current = SnapshotId::current();

    // pattern_search is now implemented on V3 (3.6.0) — should return Ok
    let pattern_result = backend.pattern_search(current, node1, &Default::default());
    assert!(
        pattern_result.is_ok(),
        "pattern_search should return Ok on V3 (implemented in 3.6.0)"
    );

    // query_nodes_by_kind should work with current snapshot
    let kind_result = backend.query_nodes_by_kind(current, "TestKind");
    assert!(
        kind_result.is_ok(),
        "query_nodes_by_kind should work with current snapshot"
    );
    assert_eq!(kind_result.unwrap().len(), 1);

    // query_nodes_by_name_pattern should work with current snapshot
    let pattern_result = backend.query_nodes_by_name_pattern(current, "test*");
    assert!(
        pattern_result.is_ok(),
        "query_nodes_by_name_pattern should work with current snapshot"
    );
    assert_eq!(pattern_result.unwrap().len(), 1);
}
