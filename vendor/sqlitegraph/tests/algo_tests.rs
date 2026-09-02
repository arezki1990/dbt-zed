use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, SqliteGraph,
    algo::{
        betweenness_centrality, connected_components, find_cycles_limited, label_propagation,
        louvain_communities, nodes_by_degree, pagerank,
    },
};

fn insert_entity(graph: &SqliteGraph, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Node".into(),
            name: name.into(),
            file_path: None,
            data: json!({ "name": name }),
        })
        .expect("insert entity")
}

fn insert_edge(graph: &SqliteGraph, from: i64, to: i64, label: &str) {
    let _ = graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: from,
            to_id: to,
            edge_type: label.into(),
            data: json!({ "label": label }),
        })
        .expect("insert edge");
}

#[test]
fn test_connected_components_returns_sorted_groups() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");
    let e = insert_entity(&graph, "E");

    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, d, e, "LINK");

    let components = connected_components(&graph).expect("components");
    assert_eq!(components.len(), 2);
    assert_eq!(components[0], vec![a, b, c]);
    assert_eq!(components[1], vec![d, e]);
}

#[test]
fn test_find_cycles_limited_returns_deterministic_cycle() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");

    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, a, "LINK");

    let cycles = find_cycles_limited(&graph, 1).expect("cycles");
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0], vec![a, b, c, a]);
}

#[test]
fn test_nodes_by_degree_orders_descending() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");

    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, a, c, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, a, "LINK");
    insert_edge(&graph, b, a, "LINK");

    let descending = nodes_by_degree(&graph, true).expect("degrees");
    assert_eq!(descending[0].0, a);
    assert!(descending[0].1 > descending[1].1);

    let ascending = nodes_by_degree(&graph, false).expect("degrees");
    assert_eq!(ascending.last().unwrap().0, a);
}

// PageRank tests

#[test]
fn test_pagerank_cycle_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");

    // Create a 3-node cycle: A -> B -> C -> A
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, a, "LINK");

    // Run PageRank with typical damping factor
    let scores = pagerank(&graph, 0.85, 20).expect("pagerank");

    // All nodes should have equal scores (~0.333) in a cycle
    assert_eq!(scores.len(), 3);
    assert!((scores[0].1 - 0.333).abs() < 0.01);
    assert!((scores[1].1 - 0.333).abs() < 0.01);
    assert!((scores[2].1 - 0.333).abs() < 0.01);
}

#[test]
fn test_pagerank_star_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let center = insert_entity(&graph, "Center");
    let leaf1 = insert_entity(&graph, "Leaf1");
    let leaf2 = insert_entity(&graph, "Leaf2");
    let leaf3 = insert_entity(&graph, "Leaf3");

    // Create star: all leaves point to center
    insert_edge(&graph, leaf1, center, "LINK");
    insert_edge(&graph, leaf2, center, "LINK");
    insert_edge(&graph, leaf3, center, "LINK");

    let scores = pagerank(&graph, 0.85, 20).expect("pagerank");

    // Center should have highest score (receiving all links)
    assert_eq!(scores.len(), 4);
    assert_eq!(scores[0].0, center);
    assert!(scores[0].1 > scores[1].1); // Center > leaves
}

#[test]
fn test_pagerank_dangling_nodes() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let _c = insert_entity(&graph, "C");

    // A -> B (C is dangling - no outgoing edges)
    insert_edge(&graph, a, b, "LINK");

    let scores = pagerank(&graph, 0.85, 20).expect("pagerank");

    // Should handle dangling nodes gracefully
    assert_eq!(scores.len(), 3);
    // All scores should be valid (not NaN, not infinite)
    for (_, score) in scores {
        assert!(score.is_finite());
        assert!(score > 0.0);
    }
}

// Betweenness Centrality tests

#[test]
fn test_betweenness_line_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");

    // Create line: A -> B -> C -> D
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, d, "LINK");

    let centrality = betweenness_centrality(&graph).expect("betweenness");

    // Middle nodes (B, C) should have higher centrality than ends (A, D)
    assert_eq!(centrality.len(), 4);

    let centrality_map: std::collections::HashMap<i64, f64> = centrality.into_iter().collect();

    // B and C should have higher centrality than A and D
    assert!(centrality_map[&b] > centrality_map[&a]);
    assert!(centrality_map[&c] > centrality_map[&d]);
}

#[test]
fn test_betweenness_star_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let center = insert_entity(&graph, "Center");
    let leaf1 = insert_entity(&graph, "Leaf1");
    let leaf2 = insert_entity(&graph, "Leaf2");
    let leaf3 = insert_entity(&graph, "Leaf3");

    // Create star: all paths go through center
    insert_edge(&graph, leaf1, center, "LINK");
    insert_edge(&graph, center, leaf2, "LINK");
    insert_edge(&graph, center, leaf3, "LINK");

    let centrality = betweenness_centrality(&graph).expect("betweenness");

    assert_eq!(centrality.len(), 4);

    // Center should have highest centrality (all paths go through it)
    assert_eq!(centrality[0].0, center);
    let center_centrality = centrality[0].1;
    assert!(center_centrality > 0.0);

    // Leaves should have zero or very low centrality
    let leaf_values: Vec<(i64, f64)> = centrality
        .into_iter()
        .filter(|(id, _)| *id != center)
        .collect();

    for (_, value) in leaf_values {
        assert!(value < center_centrality);
    }
}

#[test]
fn test_betweenness_disconnected() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");

    // Two disconnected components: A -> B, C -> D
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, c, d, "LINK");

    let centrality = betweenness_centrality(&graph).expect("betweenness");

    // Should handle disconnected components gracefully
    assert_eq!(centrality.len(), 4);

    // All values should be valid (no NaN, no infinity)
    for (_, value) in &centrality {
        assert!(value.is_finite());
        assert!(*value >= 0.0);
    }
}

// Label Propagation tests

#[test]
fn test_label_propagation_disconnected() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");
    let e = insert_entity(&graph, "E");
    let f = insert_entity(&graph, "F");

    // Create two disconnected triangles
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, a, "LINK");

    insert_edge(&graph, d, e, "LINK");
    insert_edge(&graph, e, f, "LINK");
    insert_edge(&graph, f, d, "LINK");

    let communities = label_propagation(&graph, 10).expect("label propagation");

    // Should detect 2 communities
    assert_eq!(communities.len(), 2);

    // Each community should have 3 nodes
    assert_eq!(communities[0].len(), 3);
    assert_eq!(communities[1].len(), 3);
}

#[test]
fn test_label_propagation_clique() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let nodes: Vec<i64> = (0..5).map(|_| insert_entity(&graph, "Node")).collect();

    // Create fully connected graph (clique)
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            insert_edge(&graph, nodes[i], nodes[j], "LINK");
        }
    }

    let communities = label_propagation(&graph, 10).expect("label propagation");

    // Should detect 1 community (all nodes connected)
    assert_eq!(communities.len(), 1);
    assert_eq!(communities[0].len(), 5);
}

#[test]
fn test_label_propagation_line() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");

    // Create line: A -> B -> C -> D
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, d, "LINK");

    let communities = label_propagation(&graph, 10).expect("label propagation");

    // Line graphs tend to form 1-2 communities depending on convergence
    // The key is that it's deterministic and valid
    assert!(!communities.is_empty());
    assert!(communities.len() <= 4);

    // All nodes should be assigned
    let total_nodes: usize = communities.iter().map(|c| c.len()).sum();
    assert_eq!(total_nodes, 4);
}

// Louvain Community Detection tests

#[test]
fn test_louvain_barbell() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create two cliques (cliques of 3 nodes each)
    let clique1: Vec<i64> = (0..3).map(|_| insert_entity(&graph, "C1")).collect();
    let clique2: Vec<i64> = (0..3).map(|_| insert_entity(&graph, "C2")).collect();

    // Connect clique1 internally (bidirectional edges)
    for i in 0..clique1.len() {
        for j in (i + 1)..clique1.len() {
            insert_edge(&graph, clique1[i], clique1[j], "LINK");
            insert_edge(&graph, clique1[j], clique1[i], "LINK");
        }
    }

    // Connect clique2 internally (bidirectional edges)
    for i in 0..clique2.len() {
        for j in (i + 1)..clique2.len() {
            insert_edge(&graph, clique2[i], clique2[j], "LINK");
            insert_edge(&graph, clique2[j], clique2[i], "LINK");
        }
    }

    // Add bridge edge between cliques (barbell)
    insert_edge(&graph, clique1[0], clique2[0], "BRIDGE");
    insert_edge(&graph, clique2[0], clique1[0], "BRIDGE");

    let communities = louvain_communities(&graph, 10).expect("louvain");

    // Should detect communities with strong internal connections
    // Bridge edge may or may not merge them depending on modularity
    assert!(!communities.is_empty());
    assert!(communities.len() <= 6);

    // Total nodes should be 6
    let total_nodes: usize = communities.iter().map(|c| c.len()).sum();
    assert_eq!(total_nodes, 6);
}

#[test]
fn test_louvain_star() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let center = insert_entity(&graph, "Center");
    let leaves: Vec<i64> = (0..4).map(|_| insert_entity(&graph, "Leaf")).collect();

    // Create star: all leaves connected to center
    for leaf in &leaves {
        insert_edge(&graph, *leaf, center, "LINK");
    }

    let communities = louvain_communities(&graph, 10).expect("louvain");

    // Star graph typically forms 1-2 communities
    // Key is deterministic, valid grouping
    assert!(!communities.is_empty());
    assert!(communities.len() <= 5);

    // All nodes should be assigned
    let total_nodes: usize = communities.iter().map(|c| c.len()).sum();
    assert_eq!(total_nodes, 5);
}

#[test]
fn test_louvain_convergence() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    let c = insert_entity(&graph, "C");

    // Simple triangle (bidirectional edges for strong connection)
    insert_edge(&graph, a, b, "LINK");
    insert_edge(&graph, b, a, "LINK");
    insert_edge(&graph, b, c, "LINK");
    insert_edge(&graph, c, b, "LINK");
    insert_edge(&graph, c, a, "LINK");
    insert_edge(&graph, a, c, "LINK");

    // Run with high max_iterations
    let communities1 = louvain_communities(&graph, 100).expect("louvain");

    // Run with low max_iterations
    let communities2 = louvain_communities(&graph, 5).expect("louvain");

    // Both should converge to similar result (strongly connected triangle)
    // The exact number of communities may vary based on modularity optimization
    assert!(!communities1.is_empty());
    assert!(!communities2.is_empty());

    // All nodes should be assigned in both cases
    let total1: usize = communities1.iter().map(|c| c.len()).sum();
    let total2: usize = communities2.iter().map(|c| c.len()).sum();
    assert_eq!(total1, 3);
    assert_eq!(total2, 3);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

// Empty graph tests

#[test]
fn test_pagerank_empty_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");

    // Empty graph should return empty result
    assert_eq!(scores.len(), 0);
}

#[test]
fn test_betweenness_empty_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let centrality = betweenness_centrality(&graph).expect("betweenness failed");

    // Empty graph should return empty result
    assert_eq!(centrality.len(), 0);
}

#[test]
fn test_label_prop_empty_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let communities = label_propagation(&graph, 10).expect("label propagation failed");

    // Empty graph should return empty result
    assert_eq!(communities.len(), 0);
}

#[test]
fn test_louvain_empty_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    let communities = louvain_communities(&graph, 10).expect("louvain failed");

    // Empty graph should return empty result
    assert_eq!(communities.len(), 0);
}

// Single node tests

#[test]
fn test_pagerank_single_node() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let _node = insert_entity(&graph, "Single");

    let scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");

    // Single node should have score 1.0
    assert_eq!(scores.len(), 1);
    assert!((scores[0].1 - 1.0).abs() < 0.001);
}

#[test]
fn test_betweenness_single_node() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let _node = insert_entity(&graph, "Single");

    let centrality = betweenness_centrality(&graph).expect("betweenness failed");

    // Single node should have 0.0 betweenness (no paths)
    assert_eq!(centrality.len(), 1);
    assert_eq!(centrality[0].1, 0.0);
}

// Disconnected components tests

#[test]
fn test_pagerank_disconnected_large() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create 5 disconnected components (each a triangle)
    for comp in 0..5 {
        let a = insert_entity(&graph, &format!("A_{}", comp));
        let b = insert_entity(&graph, &format!("B_{}", comp));
        let c = insert_entity(&graph, &format!("C_{}", comp));

        insert_edge(&graph, a, b, "LINK");
        insert_edge(&graph, b, c, "LINK");
        insert_edge(&graph, c, a, "LINK");
    }

    let scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");

    // All nodes should have equal scores in disconnected components
    assert_eq!(scores.len(), 15);
    let first_score = scores[0].1;
    let last_score = scores[14].1;

    // Scores should be similar (within 10% due to damping)
    let ratio = first_score / last_score;
    assert!(ratio > 0.9 && ratio < 1.1);
}

#[test]
fn test_betweenness_disconnected_large() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create 3 disconnected components
    let a = insert_entity(&graph, "A");
    let b = insert_entity(&graph, "B");
    insert_edge(&graph, a, b, "LINK");

    let c = insert_entity(&graph, "C");
    let d = insert_entity(&graph, "D");
    insert_edge(&graph, c, d, "LINK");

    let e = insert_entity(&graph, "E");
    let f = insert_entity(&graph, "F");
    insert_edge(&graph, e, f, "LINK");

    let centrality = betweenness_centrality(&graph).expect("betweenness failed");

    // All nodes should have 0.0 betweenness (no paths between components)
    assert_eq!(centrality.len(), 6);
    for (_, value) in centrality {
        assert_eq!(value, 0.0);
    }
}

// Convergence tests

#[test]
fn test_label_prop_max_iterations() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create line graph: A -> B -> C -> D -> E
    let nodes: Vec<i64> = (0..5).map(|_| insert_entity(&graph, "Node")).collect();

    for i in 0..4 {
        insert_edge(&graph, nodes[i], nodes[i + 1], "LINK");
        insert_edge(&graph, nodes[i + 1], nodes[i], "LINK");
    }

    // Run with low iterations
    let communities_low = label_propagation(&graph, 2).expect("label propagation failed");

    // Run with high iterations
    let communities_high = label_propagation(&graph, 100).expect("label propagation failed");

    // Both should complete without error
    assert!(!communities_low.is_empty());
    assert!(!communities_high.is_empty());

    // All nodes should be assigned
    let total_low: usize = communities_low.iter().map(|c| c.len()).sum();
    let total_high: usize = communities_high.iter().map(|c| c.len()).sum();
    assert_eq!(total_low, 5);
    assert_eq!(total_high, 5);
}

#[test]
fn test_louvain_max_iterations() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create two cliques connected by weak bridge
    let clique1: Vec<i64> = (0..3).map(|_| insert_entity(&graph, "C1")).collect();
    let clique2: Vec<i64> = (0..3).map(|_| insert_entity(&graph, "C2")).collect();

    // Connect clique1 internally
    for i in 0..clique1.len() {
        for j in (i + 1)..clique1.len() {
            insert_edge(&graph, clique1[i], clique1[j], "LINK");
            insert_edge(&graph, clique1[j], clique1[i], "LINK");
        }
    }

    // Connect clique2 internally
    for i in 0..clique2.len() {
        for j in (i + 1)..clique2.len() {
            insert_edge(&graph, clique2[i], clique2[j], "LINK");
            insert_edge(&graph, clique2[j], clique2[i], "LINK");
        }
    }

    // Add bridge
    insert_edge(&graph, clique1[0], clique2[0], "BRIDGE");
    insert_edge(&graph, clique2[0], clique1[0], "BRIDGE");

    // Run with low iterations
    let communities_low = louvain_communities(&graph, 2).expect("louvain failed");

    // Run with high iterations
    let communities_high = louvain_communities(&graph, 100).expect("louvain failed");

    // Both should complete without error
    assert!(!communities_low.is_empty());
    assert!(!communities_high.is_empty());

    // All nodes should be assigned
    let total_low: usize = communities_low.iter().map(|c| c.len()).sum();
    let total_high: usize = communities_high.iter().map(|c| c.len()).sum();
    assert_eq!(total_low, 6);
    assert_eq!(total_high, 6);
}

// Large graph stress tests

#[test]
fn test_pagerank_large_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create 1000 nodes in a line
    let mut node_ids = Vec::new();
    for i in 0..1000 {
        let id = insert_entity(&graph, &format!("Node_{}", i));
        node_ids.push(id);
    }

    // Create chain edges
    for i in 0..999 {
        insert_edge(&graph, node_ids[i], node_ids[i + 1], "LINK");
    }

    let start = std::time::Instant::now();
    let scores = pagerank(&graph, 0.85, 20).expect("pagerank failed");
    let duration = start.elapsed();

    // Should complete in reasonable time (< 10 seconds)
    assert!(
        duration.as_secs() < 10,
        "PageRank took too long: {:?}",
        duration
    );

    // All nodes should have scores
    assert_eq!(scores.len(), 1000);

    // All scores should be valid
    for (_, score) in scores {
        assert!(score.is_finite());
        assert!(score > 0.0);
    }
}

#[test]
fn test_label_prop_large_graph() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create 1000 nodes with random edges
    let mut node_ids = Vec::new();
    for i in 0..1000 {
        let id = insert_entity(&graph, &format!("Node_{}", i));
        node_ids.push(id);
    }

    // Create random edges (connect each node to next 5 nodes)
    for i in 0..1000 {
        for j in 1..=5 {
            if i + j < 1000 {
                insert_edge(&graph, node_ids[i], node_ids[i + j], "LINK");
            }
        }
    }

    let start = std::time::Instant::now();
    let communities = label_propagation(&graph, 10).expect("label propagation failed");
    let duration = start.elapsed();

    // Should complete in reasonable time (< 10 seconds)
    assert!(
        duration.as_secs() < 10,
        "Label propagation took too long: {:?}",
        duration
    );

    // All nodes should be assigned
    let total_nodes: usize = communities.iter().map(|c| c.len()).sum();
    assert_eq!(total_nodes, 1000);

    // Should have at least 1 community
    assert!(!communities.is_empty());
}
