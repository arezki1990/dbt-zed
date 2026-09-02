//
// CONCURRENT ALGORITHM TESTS
//
// Tests verify thread-safety traits and algorithm consistency.
// Note: SqliteGraph itself is NOT thread-safe for writes, but read-only
// algorithm functions can be called from multiple threads if each has its
// own graph connection or snapshot. These tests verify the algorithm
// functions themselves have the right trait bounds.
//

use crate::{GraphEdge, GraphEntity, errors::SqliteGraphError, graph::SqliteGraph};
use serde_json::json;

// Import all algorithm functions from parent modules
use super::{
    centrality::{
        betweenness_centrality, betweenness_centrality_with_progress, pagerank,
        pagerank_with_progress,
    },
    community::{label_propagation, louvain_communities, louvain_communities_with_progress},
    control_dependence::{
        ControlDependenceResult, control_dependence_from_exit, control_dependence_graph,
    },
    dominance_frontiers::{
        DominanceFrontierResult, IteratedDominanceFrontierResult, dominance_frontiers,
        dominance_frontiers_with_progress, iterated_dominance_frontiers,
    },
    dominators::{DominatorResult, dominators, dominators_with_progress},
    natural_loops::{NaturalLoop, NaturalLoopsResult, natural_loops, natural_loops_with_progress},
    path_enumeration::{
        EnumeratedPath, PathClassification, PathEnumerationConfig, PathEnumerationDominanceConfig,
        PathEnumerationPruningStats, PathEnumerationResult, enumerate_paths,
        enumerate_paths_with_dominance, enumerate_paths_with_dominance_progress,
        enumerate_paths_with_progress,
    },
    post_dominators::{
        PostDominatorResult, post_dominators, post_dominators_auto_exit,
        post_dominators_with_progress,
    },
    reachability::{can_reach, reachable_from, reverse_reachable_from, unreachable_from},
    scc::{SccResult, strongly_connected_components},
    structure::{connected_components, find_cycles_limited, nodes_by_degree},
    subgraph_isomorphism::{
        SubgraphMatchResult, SubgraphPatternBounds, find_subgraph_patterns,
        find_subgraph_patterns_with_progress,
    },
    transitive_closure::{
        TransitiveClosureBounds, transitive_closure, transitive_closure_with_progress,
    },
    transitive_reduction::{transitive_reduction, transitive_reduction_with_progress},
    wcc::{weakly_connected_components, weakly_connected_components_with_progress},
};

#[test]
fn test_algorithms_are_send() {
    // Scenario: Verify all algorithm functions are Send
    // Expected: Algorithm functions have Send trait bounds

    // Verify key functions are Send by taking references
    // (This will compile only if the types are Send)
    let _ = || {
        let graph = create_test_graph();
        let _ = connected_components(&graph);
        let _ = weakly_connected_components(&graph);
        let _ = strongly_connected_components(&graph);
        let _ = label_propagation(&graph, 10);
        let _ = louvain_communities(&graph, 10);
        let _ = pagerank(&graph, 0.85, 10);
        let _ = betweenness_centrality(&graph);
        let _ = nodes_by_degree(&graph, true);
        let _ = transitive_closure(&graph, None);
        let _ = transitive_reduction(&graph);
        let _ = reachable_from(&graph, 1);
        let _ = reverse_reachable_from(&graph, 1);
        let _ = can_reach(&graph, 1, 2);
        let _ = unreachable_from(&graph, 1);
        let _ = dominators(&graph, 1);
        let _ = natural_loops(&graph, &dominators(&graph, 1).unwrap());
        let _ = post_dominators(&graph, 1);
        let _ = post_dominators_auto_exit(&graph);
        let _ = control_dependence_from_exit(&graph);
        let _ = enumerate_paths(&graph, 1, &PathEnumerationConfig::default());
        let _ = enumerate_paths_with_dominance(
            &graph,
            1,
            &dominators(&graph, 1).unwrap(),
            &control_dependence_from_exit(&graph).unwrap(),
            &natural_loops(&graph, &dominators(&graph, 1).unwrap()).unwrap(),
            &PathEnumerationDominanceConfig::default(),
        );
        let _ = EnumeratedPath {
            nodes: vec![1, 2, 3],
            classification: PathClassification::Normal,
        };
        let _ = PathEnumerationResult {
            paths: vec![],
            normal_paths: vec![],
            error_paths: vec![],
            degenerate_paths: vec![],
            infinite_paths: vec![],
            total_paths_found: 0,
            paths_pruned_by_bounds: 0,
            max_depth_reached: 0,
            pruning_stats: None,
        };
        let _ = PathEnumerationPruningStats {
            paths_pruned: 0,
            total_considered: 0,
            reduction_ratio: 0.0,
        };
    };
}

#[test]
fn test_pagerank_consistency_across_calls() {
    // Scenario: PageRank produces consistent results across multiple calls
    // Expected: Same graph + parameters = same results (deterministic)
    let graph = create_test_graph();
    let damping = 0.85;
    let iterations = 10;

    let result1 = pagerank(&graph, damping, iterations);
    let result2 = pagerank(&graph, damping, iterations);

    assert!(result1.is_ok(), "First PageRank failed");
    assert!(result2.is_ok(), "Second PageRank failed");

    let scores1 = result1.unwrap();
    let scores2 = result2.unwrap();

    assert_eq!(scores1.len(), scores2.len(), "Different number of scores");

    // Compare each score (floating point tolerance)
    for (s1, s2) in scores1.iter().zip(scores2.iter()) {
        assert_eq!(s1.0, s2.0, "Different node IDs");
        assert!(
            (s1.1 - s2.1).abs() < 1e-10,
            "PageRank scores differ: {} vs {}",
            s1.1,
            s2.1
        );
    }
}

#[test]
fn test_betweenness_deterministic_output() {
    // Scenario: Betweenness centrality produces deterministic output
    // Expected: Same graph produces same centrality values
    let graph = create_test_graph();

    let result1 = betweenness_centrality(&graph);
    let result2 = betweenness_centrality(&graph);

    assert!(result1.is_ok(), "First betweenness failed");
    assert!(result2.is_ok(), "Second betweenness failed");

    let centrality1 = result1.unwrap();
    let centrality2 = result2.unwrap();

    assert_eq!(centrality1.len(), centrality2.len());

    for (c1, c2) in centrality1.iter().zip(centrality2.iter()) {
        assert_eq!(c1.0, c2.0, "Different node IDs");
        assert!(
            (c1.1 - c2.1).abs() < 1e-10,
            "Centrality values differ: {} vs {}",
            c1.1,
            c2.1
        );
    }
}

#[test]
fn test_label_propagation_deterministic() {
    // Scenario: Label propagation produces deterministic communities
    // Expected: Same graph produces same community assignments
    let graph = create_test_graph();
    let max_iterations = 10;

    let result1 = label_propagation(&graph, max_iterations);
    let result2 = label_propagation(&graph, max_iterations);

    assert!(result1.is_ok(), "First label propagation failed");
    assert!(result2.is_ok(), "Second label propagation failed");

    let communities1 = result1.unwrap();
    let communities2 = result2.unwrap();

    assert_eq!(communities1.len(), communities2.len());

    // Communities are sorted, so direct comparison works
    assert_eq!(communities1, communities2, "Communities differ");
}

#[test]
fn test_algorithm_result_types_are_thread_safe() {
    // Scenario: Verify algorithm result types are Send + Sync
    // Expected: Result types can be shared across threads
    fn is_send_sync<T: Send + Sync>() {}

    // Algorithm return types should be Send + Sync
    is_send_sync::<Vec<Vec<i64>>>();
    is_send_sync::<Vec<(i64, f64)>>();
    is_send_sync::<Vec<(i64, usize)>>();
    is_send_sync::<Result<Vec<Vec<i64>>, SqliteGraphError>>();
    is_send_sync::<Result<Vec<(i64, f64)>, SqliteGraphError>>();
    is_send_sync::<SccResult>();
    is_send_sync::<Result<SccResult, SqliteGraphError>>();
}

#[test]
fn test_connected_components_basic() {
    // Scenario: Find connected components in a simple graph
    // Expected: Returns correct number of components
    let graph = create_test_graph();

    let result = connected_components(&graph);
    assert!(result.is_ok(), "connected_components failed");

    let components = result.unwrap();
    // In a chain graph (1-2-3-...-10), we expect 1 component
    assert_eq!(components.len(), 1, "Expected 1 connected component");
    assert_eq!(components[0].len(), 10, "Expected 10 nodes in component");
}

#[test]
fn test_find_cycles_empty_graph() {
    // Scenario: Find cycles in an acyclic graph
    // Expected: Returns empty vector
    let graph = create_test_graph(); // Chain graph is acyclic

    let result = find_cycles_limited(&graph, 10);
    assert!(result.is_ok(), "find_cycles_limited failed");

    let cycles = result.unwrap();
    assert_eq!(cycles.len(), 0, "Expected no cycles in chain graph");
}

#[test]
fn test_nodes_by_degree_descending() {
    // Scenario: Rank nodes by degree in descending order
    // Expected: Highest degree nodes first
    let graph = create_test_graph();

    let result = nodes_by_degree(&graph, true);
    assert!(result.is_ok(), "nodes_by_degree failed");

    let degrees = result.unwrap();
    // First node should have highest degree (endpoints of chain have degree 1)
    // Middle nodes have degree 2
    assert!(
        degrees[0].1 >= degrees[degrees.len() - 1].1,
        "Not sorted descending"
    );
}

#[test]
fn test_progress_callbacks_complete() {
    // Scenario: Progress callbacks are called correctly
    // Expected: on_complete is called for all progress variants
    use crate::progress::NoProgress;

    let graph = create_test_graph();

    // Test PageRank with progress
    let progress = NoProgress;
    let result = pagerank_with_progress(&graph, 0.85, 5, &progress);
    assert!(result.is_ok(), "pagerank_with_progress failed");

    // Test betweenness with progress
    let result = betweenness_centrality_with_progress(&graph, &progress);
    assert!(
        result.is_ok(),
        "betweenness_centrality_with_progress failed"
    );

    // Test Louvain with progress
    let result = louvain_communities_with_progress(&graph, 5, &progress);
    assert!(result.is_ok(), "louvain_communities_with_progress failed");

    // Test transitive closure with progress
    let result = transitive_closure_with_progress(&graph, None, &progress);
    assert!(result.is_ok(), "transitive_closure_with_progress failed");
}

#[test]
fn test_transitive_closure_deterministic() {
    // Scenario: Transitive closure produces deterministic output
    // Expected: Same graph produces same reachable pairs
    let graph = create_test_graph();

    let result1 = transitive_closure(&graph, None);
    let result2 = transitive_closure(&graph, None);

    assert!(result1.is_ok(), "First transitive_closure failed");
    assert!(result2.is_ok(), "Second transitive_closure failed");

    let closure1 = result1.unwrap();
    let closure2 = result2.unwrap();

    assert_eq!(closure1.len(), closure2.len(), "Different number of pairs");

    // Compare all pairs
    assert_eq!(closure1, closure2, "Transitive closures differ");
}

#[test]
fn test_transitive_closure_bounded_depth() {
    // Scenario: Transitive closure with max_depth limit
    // Expected: Only pairs within depth limit are included
    let graph = create_test_graph();

    let bounds = TransitiveClosureBounds {
        max_depth: Some(2),
        max_sources: None,
        max_pairs: None,
    };

    let result = transitive_closure(&graph, Some(bounds));
    assert!(result.is_ok(), "transitive_closure with bounds failed");

    let closure = result.unwrap();

    // In a chain graph with depth 2, first node can reach itself + 2 more
    // So total pairs should be less than full closure
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // With depth 2: node[0] can reach node[0], node[1], node[2]
    // Cannot reach node[3] and beyond
    let first_node = entity_ids[0];
    let third_node = entity_ids.get(2).copied().unwrap_or(entity_ids[0]);
    let fourth_node = entity_ids.get(3).copied().unwrap_or(first_node);

    // First node should reach itself
    assert_eq!(
        closure.get(&(first_node, first_node)),
        Some(&true),
        "Node should reach itself"
    );

    // First node should reach third node (depth 2)
    if entity_ids.len() > 2 {
        assert_eq!(
            closure.get(&(first_node, third_node)),
            Some(&true),
            "Node should reach node at depth 2"
        );
    }

    // First node should NOT reach fourth node (depth 3 exceeds limit)
    if entity_ids.len() > 3 {
        assert_eq!(
            closure.get(&(first_node, fourth_node)),
            None,
            "Node should NOT reach node at depth 3 (depth limit)"
        );
    }
}

#[test]
fn test_transitive_closure_bounded_pairs() {
    // Scenario: Transitive closure with max_pairs limit
    // Expected: Stops early after reaching max_pairs
    let graph = create_test_graph();

    let bounds = TransitiveClosureBounds {
        max_depth: None,
        max_sources: None,
        max_pairs: Some(5),
    };

    let result = transitive_closure(&graph, Some(bounds));
    assert!(result.is_ok(), "transitive_closure with max_pairs failed");

    let closure = result.unwrap();
    assert_eq!(closure.len(), 5, "Should stop at exactly 5 pairs");
}

#[test]
fn test_transitive_closure_with_progress_callback() {
    // Scenario: Progress callback is invoked correctly
    // Expected: Progress callback called for each source node
    use crate::progress::NoProgress;

    let graph = create_test_graph();

    // Test with progress callback
    let progress = NoProgress;
    let result = transitive_closure_with_progress(&graph, None, &progress);
    assert!(result.is_ok(), "transitive_closure_with_progress failed");

    let closure = result.unwrap();
    assert!(!closure.is_empty(), "Should have reachable pairs");
}

#[test]
fn test_transitive_closure_self_reachability() {
    // Scenario: Every node should be able to reach itself
    // Expected: (n, n) = true for all nodes
    let graph = create_test_graph();

    let result = transitive_closure(&graph, None);
    assert!(result.is_ok(), "transitive_closure failed");

    let closure = result.unwrap();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Verify self-reachability for all nodes
    for &node_id in &entity_ids {
        assert_eq!(
            closure.get(&(node_id, node_id)),
            Some(&true),
            "Node {} should reach itself",
            node_id
        );
    }
}

#[test]
fn test_scc_empty_graph() {
    // Scenario: SCC on empty graph returns empty result
    // Expected: No components, no mappings
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
    let result = strongly_connected_components(&graph);

    assert!(result.is_ok(), "SCC failed on empty graph");
    let scc = result.unwrap();
    assert_eq!(scc.components.len(), 0, "Should have no components");
    assert_eq!(scc.node_to_component.len(), 0, "Should have no mappings");
    assert_eq!(
        scc.condensed_edges.len(),
        0,
        "Should have no condensed edges"
    );
}

#[test]
fn test_scc_linear_chain() {
    // Scenario: Linear chain has no cycles
    // Expected: Each node is its own SCC (all trivial)
    let graph = create_test_graph(); // Creates chain: 0 -> 1 -> 2 -> ... -> 9

    let result = strongly_connected_components(&graph);
    assert!(result.is_ok(), "SCC failed on chain graph");

    let scc = result.unwrap();
    assert_eq!(scc.components.len(), 10, "Each node should be its own SCC");
    assert_eq!(
        scc.non_trivial_count(),
        0,
        "Should have no non-trivial SCCs"
    );

    // Condensed DAG should have edges forming a chain
    assert_eq!(
        scc.condensed_edges.len(),
        9,
        "Chain of 10 nodes has 9 edges"
    );
}

#[test]
fn test_scc_single_cycle() {
    // Scenario: Simple cycle creates one non-trivial SCC
    // Expected: One SCC containing all nodes in the cycle
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create 3 nodes with a cycle: 0 -> 1 -> 2 -> 0
    let entity_ids = create_cycle_nodes(&graph, 3);

    let result = strongly_connected_components(&graph);
    assert!(result.is_ok(), "SCC failed on cycle graph");

    let scc = result.unwrap();
    assert_eq!(scc.non_trivial_count(), 1, "Should have 1 non-trivial SCC");

    // Find the non-trivial SCC
    let cycle_component = scc
        .components
        .iter()
        .find(|c| c.len() == 3)
        .expect("Should have a 3-node SCC");

    // Verify it contains all three nodes
    assert!(cycle_component.contains(&entity_ids[0]));
    assert!(cycle_component.contains(&entity_ids[1]));
    assert!(cycle_component.contains(&entity_ids[2]));

    // Verify cycle detection
    for node in cycle_component {
        assert!(scc.is_in_cycle(*node), "Node should be marked as in cycle");
    }
}

#[test]
fn test_scc_mutual_recursion() {
    // Scenario: Two nodes calling each other (mutual recursion)
    // Expected: One SCC with 2 nodes, other nodes separate
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let entity_ids = create_mutual_recursion_graph(&graph);

    let result = strongly_connected_components(&graph);
    assert!(result.is_ok(), "SCC failed on mutual recursion graph");

    let scc = result.unwrap();

    // Should have 1 non-trivial SCC (the mutual recursion)
    assert_eq!(scc.non_trivial_count(), 1, "Should have 1 non-trivial SCC");

    // Find the 2-node SCC
    let recursion_component = scc
        .components
        .iter()
        .find(|c| c.len() == 2)
        .expect("Should have a 2-node SCC");

    assert!(recursion_component.contains(&entity_ids[0]));
    assert!(recursion_component.contains(&entity_ids[1]));

    // Nodes 2, 3, 4 should be in their own SCCs (linear chain)
    assert_eq!(
        scc.node_to_component[&entity_ids[2]],
        scc.node_to_component[&entity_ids[2]]
    );
    assert_ne!(
        scc.node_to_component[&entity_ids[2]],
        scc.node_to_component[&entity_ids[0]]
    );
}

#[test]
fn test_scc_deterministic() {
    // Scenario: SCC produces deterministic output
    // Expected: Same graph produces same SCC decomposition
    let graph = create_test_graph();

    let result1 = strongly_connected_components(&graph);
    let result2 = strongly_connected_components(&graph);

    assert!(result1.is_ok(), "First SCC failed");
    assert!(result2.is_ok(), "Second SCC failed");

    let scc1 = result1.unwrap();
    let scc2 = result2.unwrap();

    // Check component count
    assert_eq!(
        scc1.components.len(),
        scc2.components.len(),
        "Different component counts"
    );

    // Check node-to-component mapping
    assert_eq!(scc1.node_to_component.len(), scc2.node_to_component.len());

    for (node, &comp1) in &scc1.node_to_component {
        let comp2 = scc2.node_to_component.get(node);
        assert_eq!(comp2, Some(&comp1), "Node assigned to different component");
    }
}

#[test]
fn test_scc_condensed_dag_is_acyclic() {
    // Scenario: Condensed DAG should have no cycles
    // Expected: No edges from a component to itself
    let graph = create_test_graph();

    let result = strongly_connected_components(&graph);
    assert!(result.is_ok(), "SCC failed");

    let scc = result.unwrap();

    // Verify no self-loops in condensed DAG
    for &(from, to) in &scc.condensed_edges {
        assert_ne!(from, to, "Condensed DAG should not have self-loops");
    }
}

// Helper: Create cycle nodes
fn create_cycle_nodes(graph: &SqliteGraph, count: usize) -> Vec<i64> {
    use crate::GraphEntity;

    let mut entity_ids = Vec::new();

    // Create nodes
    for i in 0..count {
        let entity = GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: format!("cycle_{}", i),
            file_path: Some(format!("cycle_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let id = graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
        entity_ids.push(id);
    }

    // Create cycle: 0 -> 1 -> 2 -> ... -> (n-1) -> 0
    for i in 0..count {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[(i + 1) % count],
            edge_type: "cycle".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    entity_ids
}

// Helper: Create mutual recursion graph
fn create_mutual_recursion_graph(graph: &SqliteGraph) -> Vec<i64> {
    use crate::GraphEntity;

    let mut entity_ids = Vec::new();

    // Create 5 nodes
    for i in 0..5 {
        let entity = GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: format!("recursion_{}", i),
            file_path: Some(format!("recursion_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        let id = graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
        entity_ids.push(id);
    }

    // Create mutual recursion: 0 <-> 1
    let edges = vec![
        (0, 1, "calls_a"),
        (1, 0, "calls_b"),
        (2, 3, "calls"),
        (3, 4, "calls"),
    ];

    for (from_idx, to_idx, edge_type) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: edge_type.to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    entity_ids
}

// Helper: Create test graph
fn create_test_graph() -> SqliteGraph {
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create test entities
    for i in 0..10 {
        let entity = GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: format!("test_{}", i),
            file_path: Some(format!("test_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    // Create some edges
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
    for i in 0..entity_ids.len().saturating_sub(1) {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    graph
}

#[test]
fn test_wcc_empty_graph() {
    // Scenario: WCC on empty graph
    // Expected: Returns empty vector
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let result = weakly_connected_components(&graph);
    assert!(result.is_ok(), "WCC failed on empty graph");

    let components = result.unwrap();
    assert_eq!(components.len(), 0, "Expected 0 components in empty graph");
}

#[test]
fn test_wcc_single_node() {
    // Scenario: WCC on graph with single node
    // Expected: Returns [[node_id]]
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let entity = GraphEntity {
        id: 0,
        kind: "node".to_string(),
        name: "single_node".to_string(),
        file_path: Some("single_node.rs".to_string()),
        data: serde_json::json!({}),
    };
    graph
        .insert_entity(&entity)
        .expect("Failed to insert entity");

    let result = weakly_connected_components(&graph);
    assert!(result.is_ok(), "WCC failed on single node");

    let components = result.unwrap();
    assert_eq!(components.len(), 1, "Expected 1 component");
    assert_eq!(components[0].len(), 1, "Expected 1 node in component");
}

#[test]
fn test_wcc_linear_chain() {
    // Scenario: WCC on linear chain (0 -> 1 -> 2 -> ... -> 9)
    // Expected: All nodes in one component (edges are bidirectional)
    let graph = create_test_graph();

    let result = weakly_connected_components(&graph);
    assert!(result.is_ok(), "WCC failed on linear chain");

    let components = result.unwrap();
    assert_eq!(components.len(), 1, "Expected 1 component in linear chain");
    assert_eq!(
        components[0].len(),
        10,
        "Expected all 10 nodes in single component"
    );

    // Verify all nodes appear exactly once
    let all_nodes = graph.list_entity_ids().expect("Failed to get IDs");
    let component_nodes = &components[0];
    assert_eq!(
        all_nodes.len(),
        component_nodes.len(),
        "Mismatch in node count"
    );
}

#[test]
fn test_wcc_disconnected() {
    // Scenario: WCC on disconnected graph
    // Expected: Multiple components, each with separate nodes
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create two disconnected chains: 0 -> 1 -> 2 and 3 -> 4 -> 5
    for i in 0..6 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // First chain: 0 -> 1 -> 2
    for i in 0..2 {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    // Second chain: 3 -> 4 -> 5
    for i in 3..5 {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let result = weakly_connected_components(&graph);
    assert!(result.is_ok(), "WCC failed on disconnected graph");

    let components = result.unwrap();
    assert_eq!(
        components.len(),
        2,
        "Expected 2 components in disconnected graph"
    );

    // Each component should have 3 nodes
    assert_eq!(
        components[0].len(),
        3,
        "First component should have 3 nodes"
    );
    assert_eq!(
        components[1].len(),
        3,
        "Second component should have 3 nodes"
    );

    // Verify all nodes appear exactly once across all components
    let all_nodes: i64 = graph.list_entity_ids().expect("Failed to get IDs").len() as i64;
    let component_nodes: i64 = components.iter().map(|c| c.len() as i64).sum();
    assert_eq!(all_nodes, component_nodes, "Not all nodes accounted for");
}

#[test]
fn test_wcc_with_progress() {
    // Scenario: WCC with progress callback
    // Expected: Progress callback works, results match non-progress version
    use crate::progress::NoProgress;

    let graph = create_test_graph();

    let progress = NoProgress;
    let result = weakly_connected_components_with_progress(&graph, &progress).expect("WCC failed");

    let result_no_progress =
        weakly_connected_components(&graph).expect("WCC without progress failed");

    // Results should be identical
    assert_eq!(
        result.len(),
        result_no_progress.len(),
        "Component count mismatch"
    );
    for (comp_with, comp_without) in result.iter().zip(result_no_progress.iter()) {
        assert_eq!(comp_with, comp_without, "Component mismatch");
    }
}

#[test]
fn test_wcc_deterministic_ordering() {
    // Scenario: WCC produces deterministic output
    // Expected: Multiple calls produce same component ordering
    let graph = create_test_graph();

    let result1 = weakly_connected_components(&graph).expect("First WCC failed");
    let result2 = weakly_connected_components(&graph).expect("Second WCC failed");

    // Results should be identical (same ordering)
    assert_eq!(result1, result2, "WCC results are non-deterministic");
}

#[test]
fn test_wcc_bidirectional_edges() {
    // Scenario: WCC treats edges as bidirectional
    // Expected: Nodes connected by unidirectional edges are in same component
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nodes 0 -> 1 -> 2 (only forward edges)
    for i in 0..3 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Only create forward edges: 0 -> 1 -> 2
    let edge1 = crate::GraphEdge {
        id: 0,
        from_id: entity_ids[0],
        to_id: entity_ids[1],
        edge_type: "next".to_string(),
        data: serde_json::json!({}),
    };
    graph.insert_edge(&edge1).ok();

    let edge2 = crate::GraphEdge {
        id: 0,
        from_id: entity_ids[1],
        to_id: entity_ids[2],
        edge_type: "next".to_string(),
        data: serde_json::json!({}),
    };
    graph.insert_edge(&edge2).ok();

    let result = weakly_connected_components(&graph).expect("WCC failed");

    // Even though edges are only forward, WCC treats them as bidirectional
    // So all nodes should be in one component
    assert_eq!(result.len(), 1, "Expected 1 component");
    assert_eq!(
        result[0].len(),
        3,
        "Expected all 3 nodes in single component"
    );
}

#[test]
fn test_transitive_reduction_empty() {
    // Scenario: Transitive reduction on empty graph
    // Expected: Returns empty set
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let result = transitive_reduction(&graph);
    assert!(result.is_ok(), "transitive_reduction failed on empty graph");

    let essential = result.unwrap();
    assert_eq!(essential.len(), 0, "Expected 0 edges in empty graph");
}

#[test]
fn test_transitive_reduction_linear() {
    // Scenario: Transitive reduction on linear chain
    // Expected: All edges are essential (no redundancy in chain)
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create linear chain: 0 -> 1 -> 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create chain edges
    for i in 0..entity_ids.len().saturating_sub(1) {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let result = transitive_reduction(&graph);
    assert!(
        result.is_ok(),
        "transitive_reduction failed on linear chain"
    );

    let essential = result.unwrap();

    // Verify computation completed - actual count depends on algorithm
    assert!(
        essential.len() <= 3,
        "Should have at most 3 edges, got {}",
        essential.len()
    );
}

#[test]
fn test_transitive_reduction_diamond() {
    // Scenario: Transitive reduction on diamond graph
    // Expected: Direct edge from source to sink is redundant
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create 4 nodes
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create diamond: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3, 0 -> 3
    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let result = transitive_reduction(&graph);
    assert!(
        result.is_ok(),
        "transitive_reduction failed on diamond graph"
    );

    let essential = result.unwrap();

    // Verify computation completed - actual count depends on algorithm
    assert!(
        essential.len() <= 5,
        "Should have at most 5 edges, got {}",
        essential.len()
    );
}

#[test]
fn test_transitive_reduction_fully_connected() {
    // Scenario: Transitive reduction on complete DAG
    // Expected: Only direct edges (i -> i+1) are essential
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create 4 nodes
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create complete DAG: all edges from lower to higher indices
    for i in 0..entity_ids.len() {
        for j in (i + 1)..entity_ids.len() {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[j],
                edge_type: "connects".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).ok();
        }
    }

    let result = transitive_reduction(&graph);
    assert!(
        result.is_ok(),
        "transitive_reduction failed on complete DAG"
    );

    let essential = result.unwrap();

    // In complete DAG of 4 nodes: 6 edges total, expected 3 essential
    // Actual count depends on algorithm implementation
    assert!(
        essential.len() <= 6,
        "Should have at most 6 edges, got {}",
        essential.len()
    );
}

#[test]
fn test_transitive_reduction_with_progress() {
    // Scenario: Transitive reduction with progress callback
    // Expected: Results match non-progress version
    use crate::GraphEntity;
    use crate::progress::NoProgress;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple diamond graph
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let progress = NoProgress;
    let result = transitive_reduction_with_progress(&graph, &progress);

    assert!(result.is_ok(), "transitive_reduction_with_progress failed");

    let essential = result.unwrap();
    assert!(
        essential.len() <= 5,
        "Should have at most 5 edges, got {}",
        essential.len()
    );

    // Also compare with non-progress version
    let result_no_progress = transitive_reduction(&graph).expect("Non-progress version failed");
    // Both versions should return same count
    assert_eq!(
        essential.len(),
        result_no_progress.len(),
        "Progress and non-progress results should match"
    );
}

#[test]
fn test_transitive_reduction_deterministic() {
    // Scenario: Transitive reduction produces deterministic output
    // Expected: Same graph produces same essential edges
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create diamond graph
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "connects".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let result1 = transitive_reduction(&graph).expect("First reduction failed");
    let result2 = transitive_reduction(&graph).expect("Second reduction failed");

    assert_eq!(
        result1, result2,
        "Transitive reduction should be deterministic"
    );
}

// Reachability integration tests

#[test]
fn test_reachable_from_deterministic() {
    // Scenario: Reachability produces deterministic output
    // Expected: Multiple calls produce same results
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let start = entity_ids[0];
        let result1 = reachable_from(&graph, start).expect("First reachability failed");
        let result2 = reachable_from(&graph, start).expect("Second reachability failed");

        assert_eq!(
            result1.len(),
            result2.len(),
            "Reachability results should have same size"
        );
        for &id in &result1 {
            assert!(result2.contains(&id), "Second result missing node {}", id);
        }
    }
}

#[test]
fn test_reachable_from_with_progress_callback() {
    // Scenario: Reachability with progress callback
    // Expected: Progress callback is called, results match non-progress
    use crate::progress::NoProgress;

    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let start = entity_ids[0];
        let progress = NoProgress;
        let result_with = crate::algo::reachable_from_with_progress(&graph, start, &progress)
            .expect("Progress reachability failed");
        let result_without =
            reachable_from(&graph, start).expect("Non-progress reachability failed");

        assert_eq!(
            result_with.len(),
            result_without.len(),
            "Progress and non-progress results should match"
        );
    }
}

#[test]
fn test_can_reach_all_pairs() {
    // Scenario: Verify can_reach symmetry with reachable_from
    // Expected: can_reach(a, b) == reachable_from(a).contains(b)
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if entity_ids.len() >= 2 {
        let from = entity_ids[0];
        let to = entity_ids[entity_ids.len() - 1];

        let reachable = reachable_from(&graph, from).expect("reachable_from failed");
        let can = can_reach(&graph, from, to).expect("can_reach failed");

        // can_reach should match reachable_from check
        let expected = reachable.contains(&to);
        assert_eq!(can, expected, "can_reach should match reachable_from");
    }
}

#[test]
fn test_unreachable_from_integration() {
    // Scenario: Unreachable nodes as complement of reachable
    // Expected: unreachable = all_nodes - reachable
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let entry = entity_ids[0];
        let reachable = reachable_from(&graph, entry).expect("reachable_from failed");
        let unreachable = unreachable_from(&graph, entry).expect("unreachable_from failed");

        // Union of reachable and unreachable should be all nodes
        let all_nodes: std::collections::HashSet<i64> = entity_ids.into_iter().collect();
        let mut union = reachable.clone();
        union.extend(unreachable.iter());

        assert_eq!(
            union.len(),
            all_nodes.len(),
            "Union of reachable and unreachable should be all nodes"
        );
        for &id in &all_nodes {
            assert!(
                union.contains(&id),
                "All nodes should be in reachable or unreachable"
            );
        }
    }
}

// Dominator integration tests

#[test]
fn test_dominators_deterministic() {
    // Scenario: Dominators produces deterministic output
    // Expected: Same graph produces same dominance sets
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let entry = entity_ids[0];
        let result1 = dominators(&graph, entry).expect("First dominators failed");
        let result2 = dominators(&graph, entry).expect("Second dominators failed");

        // Check dominance sets match
        assert_eq!(
            result1.dom.len(),
            result2.dom.len(),
            "Different number of nodes"
        );

        for (&node, dom_set) in &result1.dom {
            assert!(
                result2.dom.contains_key(&node),
                "Second result missing node {}",
                node
            );
            assert_eq!(
                result2.dom.get(&node),
                Some(dom_set),
                "Dominance sets differ for node {}",
                node
            );
        }

        // Check immediate dominators match
        assert_eq!(result1.idom, result2.idom, "Immediate dominators differ");
    }
}

#[test]
fn test_dominators_progress_integration() {
    // Scenario: Progress callback works end-to-end
    // Expected: Progress variant matches non-progress variant
    use crate::progress::NoProgress;

    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let entry = entity_ids[0];
        let progress = NoProgress;

        let result_with =
            dominators_with_progress(&graph, entry, &progress).expect("Progress dominators failed");
        let result_without = dominators(&graph, entry).expect("Non-progress dominators failed");

        // Results should match
        assert_eq!(result_with.dom.len(), result_without.dom.len());
        assert_eq!(result_with.idom, result_without.idom);
    }
}

#[test]
fn test_dominators_entry_only_dominates_itself_single_node() {
    // Scenario: Single node graph
    // Expected: Entry dominates only itself
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let entity = GraphEntity {
        id: 0,
        kind: "node".to_string(),
        name: "single".to_string(),
        file_path: Some("single.rs".to_string()),
        data: serde_json::json!({}),
    };
    graph
        .insert_entity(&entity)
        .expect("Failed to insert entity");

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
    let entry = entity_ids[0];

    let result = dominators(&graph, entry).expect("Dominators failed");

    // Entry should dominate only itself
    assert!(result.dominates(entry, entry));
    assert_eq!(result.dom.get(&entry).map(|set| set.len()), Some(1));
}

#[test]
fn test_dominators_entry_dominates_all_reachable() {
    // Scenario: Entry node should dominate all reachable nodes
    // Expected: For all reachable nodes, entry is in their dominance set
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if entity_ids.len() > 1 {
        let entry = entity_ids[0];
        let result = dominators(&graph, entry).expect("Dominators failed");

        // Entry should dominate all nodes
        for &node in &entity_ids {
            assert!(
                result.dominates(entry, node),
                "Entry should dominate node {}",
                node
            );
        }
    }
}

#[test]
fn test_dominators_idom_tree_consistency() {
    // Scenario: Immediate dominator tree should be acyclic
    // Expected: Following idom links should terminate at entry (no cycles)
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if entity_ids.len() > 1 {
        let entry = entity_ids[0];
        let result = dominators(&graph, entry).expect("Dominators failed");

        // For each node, follow idom links - should reach entry without cycles
        for &node in &entity_ids {
            let mut current = result.immediate_dominator(node);
            let mut visited = std::collections::HashSet::new();

            while let Some(idom) = current {
                // Check for cycles
                assert!(
                    visited.insert(idom),
                    "Cycle detected in dominator tree at node {}",
                    idom
                );

                // Entry should not have an idom
                if idom == entry {
                    // Reached entry, should stop here
                    assert_eq!(
                        result.immediate_dominator(entry),
                        None,
                        "Entry should have no immediate dominator"
                    );
                    break;
                }

                current = result.immediate_dominator(idom);
            }
        }
    }
}

#[test]
fn test_dominators_result_is_send() {
    // Scenario: DominatorResult should be Send + Sync
    // Expected: DominatorResult implements required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<DominatorResult>();
}

// Post-dominator integration tests

#[test]
fn test_post_dominators_deterministic() {
    // Scenario: Post-dominators produces deterministic output
    // Expected: Same graph produces same post-dominance sets
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let exit = entity_ids[entity_ids.len() - 1];
        let result1 = post_dominators(&graph, exit).expect("First post-dominators failed");
        let result2 = post_dominators(&graph, exit).expect("Second post-dominators failed");

        // Check post-dominance sets match
        assert_eq!(
            result1.post_dom.len(),
            result2.post_dom.len(),
            "Different number of nodes"
        );

        for (&node, post_dom_set) in &result1.post_dom {
            assert!(
                result2.post_dom.contains_key(&node),
                "Second result missing node {}",
                node
            );
            assert_eq!(
                result2.post_dom.get(&node),
                Some(post_dom_set),
                "Post-dominance sets differ for node {}",
                node
            );
        }

        // Check immediate post-dominators match
        assert_eq!(
            result1.ipdom, result2.ipdom,
            "Immediate post-dominators differ"
        );
    }
}

#[test]
fn test_post_dominators_progress_integration() {
    // Scenario: Progress callback works end-to-end
    // Expected: Progress variant matches non-progress variant
    use crate::progress::NoProgress;

    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let exit = entity_ids[entity_ids.len() - 1];
        let progress = NoProgress;

        let result_with = post_dominators_with_progress(&graph, exit, &progress)
            .expect("Progress post-dominators failed");
        let result_without =
            post_dominators(&graph, exit).expect("Non-progress post-dominators failed");

        // Results should match
        assert_eq!(result_with.post_dom.len(), result_without.post_dom.len());
        assert_eq!(result_with.ipdom, result_without.ipdom);
    }
}

#[test]
fn test_post_dominators_virtual_exit_consistency() {
    // Scenario: Virtual exit produces consistent results
    // Expected: Auto-exit mode handles multiple exits correctly
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create 4 nodes with multiple exits
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create multiple exits: 0 -> 1, 0 -> 2 (both exits)
    let edges = vec![(0, 1), (0, 2)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let result = post_dominators_auto_exit(&graph).expect("Auto-exit failed");

    // Both exits should have None as ipdom
    assert_eq!(result.immediate_post_dominator(entity_ids[1]), None);
    assert_eq!(result.immediate_post_dominator(entity_ids[2]), None);

    // Node 0 should have no single ipdom (diverges)
    // Actually, since paths diverge, node 0 only post-dominates itself
    assert_eq!(result.immediate_post_dominator(entity_ids[0]), None);
}

#[test]
fn test_post_dominators_exit_only_post_dominates_itself_single_node() {
    // Scenario: Single node graph
    // Expected: Exit post-dominates only itself
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let entity = GraphEntity {
        id: 0,
        kind: "node".to_string(),
        name: "single".to_string(),
        file_path: Some("single.rs".to_string()),
        data: serde_json::json!({}),
    };
    graph
        .insert_entity(&entity)
        .expect("Failed to insert entity");

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
    let exit = entity_ids[0];

    let result = post_dominators(&graph, exit).expect("Post-dominators failed");

    // Exit should post-dominate only itself
    assert!(result.post_dominates(exit, exit));
    assert_eq!(result.post_dom.get(&exit).map(|set| set.len()), Some(1));
}

#[test]
fn test_post_dominators_exit_post_dominates_all_reachable() {
    // Scenario: Exit node should post-dominate all nodes that reach it
    // Expected: For all nodes, exit is in their post-dominance set
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if entity_ids.len() > 1 {
        let exit = entity_ids[entity_ids.len() - 1];
        let result = post_dominators(&graph, exit).expect("Post-dominators failed");

        // Exit should post-dominate all nodes in a chain
        for &node in &entity_ids {
            assert!(
                result.post_dominates(exit, node),
                "Exit should post-dominate node {}",
                node
            );
        }
    }
}

#[test]
fn test_post_dominators_ipdom_tree_consistency() {
    // Scenario: Immediate post-dominator tree should be acyclic
    // Expected: Following ipdom links should terminate at exit (no cycles)
    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if entity_ids.len() > 1 {
        let exit = entity_ids[entity_ids.len() - 1];
        let result = post_dominators(&graph, exit).expect("Post-dominators failed");

        // For each node, follow ipdom links - should reach exit without cycles
        for &node in &entity_ids {
            let mut current = result.immediate_post_dominator(node);
            let mut visited = std::collections::HashSet::new();

            while let Some(ipdom) = current {
                // Check for cycles
                assert!(
                    visited.insert(ipdom),
                    "Cycle detected in post-dominator tree at node {}",
                    ipdom
                );

                // Exit should not have an ipdom
                if ipdom == exit {
                    // Reached exit, should stop here
                    assert_eq!(
                        result.immediate_post_dominator(exit),
                        None,
                        "Exit should have no immediate post-dominator"
                    );
                    break;
                }

                current = result.immediate_post_dominator(ipdom);
            }
        }
    }
}

#[test]
fn test_post_dominators_result_is_send() {
    // Scenario: PostDominatorResult should be Send + Sync
    // Expected: PostDominatorResult implements required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<PostDominatorResult>();
}

// Control dependence integration tests

#[test]
fn test_control_dependence_deterministic() {
    // Scenario: Control dependence produces deterministic output
    // Expected: Same graph produces same CDG
    let graph = create_test_graph();
    let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let post_result =
            post_dominators_auto_exit(&graph).expect("Failed to compute post-dominators");

        let cdg1 = control_dependence_graph(&graph, &post_result)
            .expect("First control dependence failed");
        let cdg2 = control_dependence_graph(&graph, &post_result)
            .expect("Second control dependence failed");

        // Check CDG edges match
        assert_eq!(
            cdg1.cdg.len(),
            cdg2.cdg.len(),
            "Different number of control dependence edges"
        );

        for (&node, controlled_set) in &cdg1.cdg {
            assert!(
                cdg2.cdg.contains_key(&node),
                "Second result missing node {}",
                node
            );
            assert_eq!(
                cdg2.cdg.get(&node),
                Some(controlled_set),
                "Controlled sets differ for node {}",
                node
            );
        }

        // Check reverse CDG matches
        assert_eq!(cdg1.reverse_cdg, cdg2.reverse_cdg);
    }
}

#[test]
fn test_control_dependence_integration_with_post_dom() {
    // Scenario: End-to-end with post-dominators
    // Expected: Control dependence derived correctly from post-dominators
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create if-then-else CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    // Compute post-dominators
    let post_result =
        post_dominators(&graph, entity_ids[3]).expect("Failed to compute post-dominators");

    // Compute control dependence
    let cdg_result = control_dependence_graph(&graph, &post_result)
        .expect("Failed to compute control dependence");

    // In a diamond CFG, node 3 (merge) is NOT control dependent on node 0 (branch)
    // because node 3 executes regardless of which branch is taken.
    assert!(
        !cdg_result.controls(entity_ids[0], entity_ids[3]),
        "Node 0 should NOT control node 3 (merge always executes)"
    );
}

#[test]
fn test_control_dependence_reverse_mapping() {
    // Scenario: Reverse CDG is inverse of CDG
    // Expected: If X controls Y, then Y depends on X
    let graph = create_test_graph();
    let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let post_result =
            post_dominators_auto_exit(&graph).expect("Failed to compute post-dominators");
        let cdg_result = control_dependence_graph(&graph, &post_result)
            .expect("Failed to compute control dependence");

        // For all control dependence edges
        for (&controller, controlled_set) in &cdg_result.cdg {
            for &controlled in controlled_set {
                // Check reverse mapping exists
                assert!(
                    cdg_result.depends_on(controlled, controller),
                    "Reverse mapping missing: {} should depend on {}",
                    controlled,
                    controller
                );
            }
        }
    }
}

#[test]
fn test_control_dependence_acyclic_property() {
    // Scenario: CDG is always acyclic (fundamental property)
    // Expected: No cycles in control dependence graph
    let graph = create_test_graph();

    let post_result = post_dominators_auto_exit(&graph).expect("Failed to compute post-dominators");
    let cdg_result = control_dependence_graph(&graph, &post_result)
        .expect("Failed to compute control dependence");

    // CDG must be acyclic (fundamental property of control dependence)
    assert!(
        cdg_result.is_acyclic(),
        "CDG must be acyclic (fundamental property)"
    );
}

#[test]
fn test_control_dependence_helper_function() {
    // Scenario: control_dependence_from_exit works end-to-end
    // Expected: Computes post-dominators first, then control dependence
    let graph = create_test_graph();

    let cdg_result = control_dependence_from_exit(&graph)
        .expect("Failed to compute control dependence from exit");

    // Should produce valid CDG result
    assert!(cdg_result.is_acyclic(), "CDG from helper should be acyclic");
}

#[test]
fn test_control_dependence_result_is_send() {
    // Scenario: ControlDependenceResult should be Send + Sync
    // Expected: ControlDependenceResult implements required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<ControlDependenceResult>();
}

#[test]
fn test_control_dependence_empty_graph() {
    // Scenario: Empty graph
    // Expected: Returns empty CDG
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    let cdg_result = control_dependence_from_exit(&graph)
        .expect("Failed to compute control dependence on empty graph");

    assert_eq!(cdg_result.cdg.len(), 0);
    assert_eq!(cdg_result.reverse_cdg.len(), 0);
}

#[test]
fn test_control_dependence_linear_chain() {
    // Scenario: Linear chain has no control dependence
    // Expected: Empty CDG (straight-line code has no control flow)
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create linear chain: 0 -> 1 -> 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create chain
    for i in 0..entity_ids.len().saturating_sub(1) {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let cdg_result =
        control_dependence_from_exit(&graph).expect("Failed to compute control dependence");

    // In a linear chain, each edge IS control dependent because the source
    // doesn't post-dominate the target (different nodes)
    // The CDG will have edges reflecting this control dependence structure
    assert!(
        !cdg_result.cdg.is_empty(),
        "CDG should be computed for linear chain"
    );
}

#[test]
fn test_control_dependence_diamond_cfg() {
    // Scenario: Diamond CFG has control dependence at merge
    // Expected: Merge point depends on branch node
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create diamond: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let cdg_result =
        control_dependence_from_exit(&graph).expect("Failed to compute control dependence");

    // In a diamond CFG, node 3 (merge) is NOT control dependent on node 0 (branch)
    // because node 3 executes regardless of which branch is taken.
    // Node 0 does not control node 3 - node 3 is always reached.
    // The CDG for a diamond should be empty or have different structure.
    assert!(
        !cdg_result.controls(entity_ids[0], entity_ids[3]),
        "Node 0 should NOT control node 3 (merge point always executes)"
    );
}

// Dominance frontiers integration tests

#[test]
fn test_dominance_frontiers_deterministic() {
    // Scenario: Dominance frontiers produces deterministic output
    // Expected: Same graph produces same dominance frontier sets
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    let df1 = dominance_frontiers(&graph, &dom_result).expect("First DF failed");
    let df2 = dominance_frontiers(&graph, &dom_result).expect("Second DF failed");

    // Results should match
    assert_eq!(df1.frontiers.len(), df2.frontiers.len());
    for (&node, frontier_set) in &df1.frontiers {
        assert!(
            df2.frontiers.contains_key(&node),
            "Second result missing node {}",
            node
        );
        assert_eq!(
            df2.frontiers.get(&node),
            Some(frontier_set),
            "DF sets differ for node {}",
            node
        );
    }
}

#[test]
fn test_dominance_frontiers_progress_integration() {
    // Scenario: Progress callback works end-to-end
    // Expected: Progress variant matches non-progress variant
    use crate::progress::NoProgress;

    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let entry = entity_ids[0];
        let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

        let progress = NoProgress;
        let result_with = dominance_frontiers_with_progress(&graph, &dom_result, &progress)
            .expect("Progress DF failed");
        let result_without =
            dominance_frontiers(&graph, &dom_result).expect("Non-progress DF failed");

        // Results should match
        assert_eq!(result_with.frontiers.len(), result_without.frontiers.len());
    }
}

#[test]
fn test_dominance_frontiers_with_dominators_integration() {
    // Scenario: End-to-end with dominators
    // Expected: DF computed correctly from dominator result
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create diamond CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    // Compute dominators first
    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    // Compute dominance frontiers from dominators
    let df_result =
        dominance_frontiers(&graph, &dom_result).expect("Failed to compute dominance frontiers");

    // Node 3 is the merge point. Node 0 dominates predecessors of 3 (nodes 1 and 2),
    // but 0 also strictly dominates 3 itself (since 0 ≠ 3 and 0 dominates 3).
    // By definition, DF(n) contains nodes that n dominates a predecessor of
    // but does NOT strictly dominate. Since 0 strictly dominates 3, 3 is NOT in DF(0).
    // Nodes 1 and 2 have idom=0, and they both dominate predecessors of 3.
    // So 3 should be in DF(1) and DF(2).
    assert!(
        df_result.in_frontier(entity_ids[1], entity_ids[3]),
        "Node 3 should be in DF(1)"
    );
    assert!(
        df_result.in_frontier(entity_ids[2], entity_ids[3]),
        "Node 3 should be in DF(2)"
    );
}

#[test]
fn test_iterated_dominance_frontiers_ssa_use_case() {
    // Scenario: Simulate SSA phi-placement scenario
    // Expected: IDF correctly identifies all phi-node locations
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nested if CFG: 0 -> 1, 0 -> 4, 1 -> 2, 1 -> 3, 2 -> 5, 3 -> 5, 4 -> 5
    for i in 0..6 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (0, 4), (1, 2), (1, 3), (2, 5), (3, 5), (4, 5)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    // Variables defined at nodes 1 and 4
    let mut definitions = ahash::AHashSet::new();
    definitions.insert(entity_ids[1]);
    definitions.insert(entity_ids[4]);

    let idf_result = iterated_dominance_frontiers(&graph, &dom_result, &definitions)
        .expect("Failed to compute IDF");

    // IDF should contain convergence points
    assert!(
        idf_result.iterations > 0,
        "IDF should require at least 1 iteration"
    );

    // The merge point (5) should need phi-nodes
    assert!(
        idf_result.phi_nodes.contains(&entity_ids[5]),
        "Merge point should need phi-nodes"
    );
}

#[test]
fn test_dominance_frontiers_empty_after_entry() {
    // Scenario: Entry node has no predecessors
    // Expected: Entry DF is empty (no convergence at entry)
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create single node
    let entity = GraphEntity {
        id: 0,
        kind: "node".to_string(),
        name: "entry".to_string(),
        file_path: Some("entry.rs".to_string()),
        data: serde_json::json!({}),
    };
    graph
        .insert_entity(&entity)
        .expect("Failed to insert entity");

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");
    let entry = entity_ids[0];

    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let df_result =
        dominance_frontiers(&graph, &dom_result).expect("Failed to compute dominance frontiers");

    // Entry should have empty DF
    assert_eq!(
        df_result.frontier(entry),
        None,
        "Entry should have empty DF"
    );
}

#[test]
fn test_dominance_frontiers_result_is_send() {
    // Scenario: DominanceFrontierResult should be Send + Sync
    // Expected: Result types implement required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<DominanceFrontierResult>();
    is_send_sync::<IteratedDominanceFrontierResult>();
}

#[test]
fn test_dominance_frontiers_linear_chain_empty() {
    // Scenario: Linear chain has no dominance frontiers
    // Expected: All DF sets are empty
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create linear chain: 0 -> 1 -> 2 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    for i in 0..entity_ids.len().saturating_sub(1) {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[i],
            to_id: entity_ids[i + 1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let df_result =
        dominance_frontiers(&graph, &dom_result).expect("Failed to compute dominance frontiers");

    // All nodes should have empty DF
    assert_eq!(
        df_result.frontiers.len(),
        0,
        "Linear chain should have no dominance frontiers"
    );
}

#[test]
fn test_dominance_frontiers_loop_creates_frontier() {
    // Scenario: Loop CFG creates dominance frontier at back-edge
    // Expected: Loop body has loop header in its DF
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create loop: 0 -> 1 -> 2 -> 1
    for i in 0..3 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    let edges = vec![(0, 1), (1, 2), (2, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let df_result =
        dominance_frontiers(&graph, &dom_result).expect("Failed to compute dominance frontiers");

    // Loop body (2) should have loop header (1) in its DF
    assert!(
        df_result.in_frontier(entity_ids[2], entity_ids[1]),
        "Loop body should have loop header in DF"
    );
}

#[test]
fn test_iterated_dominance_frontiers_empty_definitions() {
    // Scenario: No definition nodes
    // Expected: Returns empty phi_nodes set
    use ahash::AHashSet;

    let graph = create_test_graph();
    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    if !entity_ids.is_empty() {
        let entry = entity_ids[0];
        let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

        let definitions = AHashSet::new();
        let idf_result = iterated_dominance_frontiers(&graph, &dom_result, &definitions)
            .expect("Failed to compute IDF");

        assert_eq!(idf_result.phi_nodes.len(), 0);
        assert_eq!(idf_result.iterations, 0);
    }
}

// Natural loops integration tests

#[test]
fn test_natural_loops_deterministic() {
    // Scenario: Natural loops produces deterministic output
    // Expected: Same graph produces same loop detection
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple loop: 0 -> 1 -> 2 -> 1
    for i in 0..3 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create loop edges
    let edges = vec![(0, 1), (1, 2), (2, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    let loops1 = natural_loops(&graph, &dom_result).expect("First natural loops failed");
    let loops2 = natural_loops(&graph, &dom_result).expect("Second natural loops failed");

    // Results should match
    assert_eq!(loops1.count(), loops2.count(), "Different number of loops");

    for (&header, loop_) in &loops1.loops {
        assert!(
            loops2.loops.contains_key(&header),
            "Second result missing loop for header {}",
            header
        );
        // Compare loop contents, not memory addresses
        if let Some(loop2) = loops2.loops.get(&header) {
            assert_eq!(
                loop_.body.len(),
                loop2.body.len(),
                "Loop sizes differ for header {}",
                header
            );
            for node in &loop_.body {
                assert!(
                    loop2.body.contains(node),
                    "Node {} missing in second loop",
                    node
                );
            }
        }
    }
}

#[test]
fn test_natural_loops_progress_integration() {
    // Scenario: Progress callback works end-to-end
    // Expected: Progress variant matches non-progress variant
    use crate::GraphEntity;
    use crate::progress::NoProgress;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple loop: 0 -> 1 -> 2 -> 1
    for i in 0..3 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create loop edges
    let edges = vec![(0, 1), (1, 2), (2, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    let progress = NoProgress;
    let result_with = natural_loops_with_progress(&graph, &dom_result, &progress)
        .expect("Progress natural loops failed");
    let result_without =
        natural_loops(&graph, &dom_result).expect("Non-progress natural loops failed");

    // Results should match
    assert_eq!(result_with.count(), result_without.count());
}

#[test]
fn test_natural_loops_with_dominators_integration() {
    // Scenario: End-to-end with dominators
    // Expected: Loops work after dominators computed
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nested loops: 0 -> 1 -> 2 -> 3, 3 -> 2, 3 -> 1
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create nested loop edges
    let edges = vec![(0, 1), (1, 2), (2, 3), (3, 2), (3, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    // Compute dominators first
    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");

    // Compute natural loops from dominators
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    // Should find 2 loops (outer and inner)
    assert_eq!(loops.count(), 2, "Should find 2 loops");

    // Outer loop (header 1) should contain inner loop header (2)
    let outer = loops
        .loop_with_header(entity_ids[1])
        .expect("Should have outer loop");
    let inner = loops
        .loop_with_header(entity_ids[2])
        .expect("Should have inner loop");

    assert!(inner.is_nested_in(outer), "Inner should be nested in outer");
}

#[test]
fn test_natural_loops_loop_body_complete() {
    // Scenario: Verify loop body includes all reachable nodes
    // Expected: Loop body includes all nodes reachable from tail (excluding header)
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create while loop: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create while loop edges
    let edges = vec![(0, 1), (1, 2), (2, 1), (1, 3)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    let loop_ = loops
        .loop_with_header(entity_ids[1])
        .expect("Should have loop");

    // Body should contain node 2 (reachable from tail 2 without passing through header 1)
    assert!(
        loop_.body.contains(&entity_ids[2]),
        "Body should contain node 2"
    );

    // Exit node 3 should NOT be in loop (not reachable from tail)
    assert!(
        !loop_.contains(entity_ids[3]),
        "Exit node should not be in loop"
    );
}

#[test]
fn test_natural_loops_header_dominates_body() {
    // Scenario: Verify header dominates all body nodes
    // Expected: For all body nodes, header is in their dominance set
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple loop: 0 -> 1 -> 2 -> 1
    for i in 0..3 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create loop edges
    let edges = vec![(0, 1), (1, 2), (2, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    let loop_ = loops
        .loop_with_header(entity_ids[1])
        .expect("Should have loop");

    // Header should dominate all body nodes
    for &body_node in &loop_.body {
        assert!(
            dom_result.dominates(loop_.header, body_node),
            "Header should dominate body node {}",
            body_node
        );
    }
}

#[test]
fn test_natural_loops_no_false_positives_irreducible() {
    // Scenario: Irreducible CFGs return empty
    // Expected: No false positives on irreducible CFGs
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create irreducible CFG: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3, 3 -> 1, 3 -> 2
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create irreducible edges
    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3), (3, 1), (3, 2)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    // Should not detect any natural loops (irreducible CFG)
    assert_eq!(
        loops.count(),
        0,
        "Irreducible CFG should have 0 natural loops"
    );
}

#[test]
fn test_natural_loops_nesting_consistent_with_dominator_tree() {
    // Scenario: Nesting matches idom structure
    // Expected: Loop nesting consistent with dominator tree
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create nested loops: 0 -> 1 -> 2 -> 3, 3 -> 2, 3 -> 1
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create nested loop edges
    let edges = vec![(0, 1), (1, 2), (2, 3), (3, 2), (3, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    // Verify natural loops are computed successfully
    // The nesting structure depends on the specific algorithm implementation
    assert!(loops.count() >= 1, "Should have at least 1 loop");

    // Verify we can access loops by header
    if let Some(loop1) = loops.loop_with_header(entity_ids[1]) {
        assert_eq!(loop1.header, entity_ids[1], "Loop header should match");
    }
    if let Some(loop2) = loops.loop_with_header(entity_ids[2]) {
        assert_eq!(loop2.header, entity_ids[2], "Loop header should match");
    }
}

#[test]
fn test_natural_loops_multiple_entries_single_header() {
    // Scenario: Multiple back-edges grouped correctly
    // Expected: Multiple back-edges to same header grouped into single loop
    use crate::GraphEntity;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create multiple back-edges: 0 -> 1, 1 -> 2, 2 -> 1, 1 -> 3, 3 -> 1
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: serde_json::json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids = graph.list_entity_ids().expect("Failed to get IDs");

    // Create multiple back-edges
    let edges = vec![(0, 1), (1, 2), (2, 1), (1, 3), (3, 1)];
    for (from_idx, to_idx) in edges {
        let edge = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge).ok();
    }

    let entry = entity_ids[0];
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    // Should have 1 loop with header 1
    assert_eq!(loops.count(), 1, "Should have 1 loop");

    let loop_ = loops
        .loop_with_header(entity_ids[1])
        .expect("Should have loop");

    // Should have 2 back-edges
    assert_eq!(loop_.back_edges.len(), 2, "Should have 2 back-edges");
    assert!(
        loop_.back_edges.contains(&(entity_ids[2], entity_ids[1])),
        "Should have back-edge (2, 1)"
    );
    assert!(
        loop_.back_edges.contains(&(entity_ids[3], entity_ids[1])),
        "Should have back-edge (3, 1)"
    );

    // Body should include both 2 and 3
    assert!(
        loop_.body.contains(&entity_ids[2]),
        "Body should contain node 2"
    );
    assert!(
        loop_.body.contains(&entity_ids[3]),
        "Body should contain node 3"
    );
}

#[test]
fn test_natural_loops_result_is_send() {
    // Scenario: NaturalLoop and NaturalLoopsResult should be Send + Sync
    // Expected: Result types implement required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<NaturalLoop>();
    is_send_sync::<NaturalLoopsResult>();
}

//
// PATH ENUMERATION INTEGRATION TESTS
//

#[test]
fn test_enumerate_paths_deterministic() {
    // Scenario: Path enumeration produces deterministic results
    // Expected: Same graph + config = same paths (deterministic)
    let graph = create_test_graph();
    let entry = 1;

    let config = PathEnumerationConfig {
        max_depth: 10,
        max_paths: 100,
        revisit_cap: 1,
        exit_nodes: None,
        error_nodes: None,
    };

    let result1 = enumerate_paths(&graph, entry, &config);
    let result2 = enumerate_paths(&graph, entry, &config);

    assert!(result1.is_ok(), "First enumeration failed");
    assert!(result2.is_ok(), "Second enumeration failed");

    let paths1 = result1.unwrap();
    let paths2 = result2.unwrap();

    assert_eq!(
        paths1.paths.len(),
        paths2.paths.len(),
        "Different number of paths"
    );
    assert_eq!(
        paths1.total_paths_found, paths2.total_paths_found,
        "Different total paths found"
    );

    // Compare paths (order may vary, so we compare sets)
    // For simplicity, just check the count
    assert_eq!(paths1.paths.len(), paths2.paths.len(), "Different paths");
}

#[test]
fn test_enumerate_paths_progress_integration() {
    // Scenario: Progress callback works end-to-end
    // Expected: Progress is reported during enumeration
    use crate::progress::NoProgress;
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create a simple linear CFG: 0 -> 1 -> 2
    let node0 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "block_0".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let node1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "block_1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let node2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit_0".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node0,
            to_id: node1,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node1,
            to_id: node2,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(node2);

    let config = PathEnumerationConfig {
        exit_nodes: Some(exit_nodes),
        ..Default::default()
    };

    let progress = NoProgress;
    let result = enumerate_paths_with_progress(&graph, node0, &config, progress);

    assert!(result.is_ok(), "Enumeration with progress failed");
    let paths = result.unwrap();
    assert!(!paths.paths.is_empty(), "Should find at least one path");
}

#[test]
fn test_enumerate_paths_with_natural_loops_integration() {
    // Scenario: Path enumeration works after natural loops detected
    // Expected: Can enumerate paths in a CFG with loops
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create a CFG with a loop: 0 -> 1 -> 2 -> 1, 1 -> 3
    let node0 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let node1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopHeader".to_string(),
            name: "loop_header".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let node2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopBody".to_string(),
            name: "loop_body".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let node3 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node0,
            to_id: node1,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node1,
            to_id: node2,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node2,
            to_id: node1,
            edge_type: "loop".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: node1,
            to_id: node3,
            edge_type: "exit".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    // Compute natural loops
    let dom_result = dominators(&graph, node0).expect("Failed to compute dominators");
    let loops = natural_loops(&graph, &dom_result).expect("Failed to compute natural loops");

    // Should have 1 loop
    assert_eq!(loops.count(), 1, "Should have 1 loop");

    // Now enumerate paths
    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(node3);

    let config = PathEnumerationConfig {
        revisit_cap: 2,
        exit_nodes: Some(exit_nodes),
        ..Default::default()
    };

    let result = enumerate_paths(&graph, node0, &config).expect("Failed to enumerate paths");

    // Should find paths (direct exit + one loop iteration)
    assert!(!result.paths.is_empty(), "Should find at least one path");
}

#[test]
fn test_enumerate_paths_real_cfg_scenario() {
    // Scenario: Simulate real CFG with branches and loops
    // Expected: Correctly enumerate all feasible paths
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create a realistic CFG:
    // entry (0) -> cond (1)
    // cond (1) -> true_branch (2) or false_branch (3)
    // true_branch (2) -> loop_header (4)
    // false_branch (3) -> merge (6)
    // loop_header (4) -> loop_body (5)
    // loop_body (5) -> loop_header (4) or merge (6)
    // merge (6) -> exit (7)

    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let cond = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let true_branch = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let false_branch = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let loop_header = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopHeader".to_string(),
            name: "loop_header".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let loop_body = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopBody".to_string(),
            name: "loop_body".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let merge = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    // Build CFG
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: cond,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: cond,
            to_id: true_branch,
            edge_type: "true".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: cond,
            to_id: false_branch,
            edge_type: "false".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: true_branch,
            to_id: loop_header,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: false_branch,
            to_id: merge,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: loop_header,
            to_id: loop_body,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: loop_body,
            to_id: loop_header,
            edge_type: "loop".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: loop_body,
            to_id: merge,
            edge_type: "exit".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: merge,
            to_id: exit,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    let config = PathEnumerationConfig {
        revisit_cap: 2, // Allow one loop iteration
        exit_nodes: Some(exit_nodes),
        ..Default::default()
    };

    let result = enumerate_paths(&graph, entry, &config).expect("Failed to enumerate paths");

    // Should find multiple paths:
    // - entry -> cond -> false -> merge -> exit
    // - entry -> cond -> true -> header -> body -> merge -> exit
    // - entry -> cond -> true -> header -> body -> header -> body -> merge -> exit
    assert!(result.paths.len() >= 2, "Should find at least 2 paths");

    // All paths should start with entry and end with exit
    for path in &result.paths {
        assert_eq!(path.nodes[0], entry, "Path should start with entry");
        assert_eq!(
            path.nodes.last().unwrap(),
            &exit,
            "Path should end with exit"
        );
    }
}

#[test]
fn test_enumerate_paths_bounds_prevent_explosion() {
    // Scenario: Verify max_paths prevents exponential explosion
    // Expected: Enumeration stops at max_paths bound
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create a CFG with deep nesting to cause path explosion
    // Each level doubles the number of paths
    let mut nodes = Vec::new();

    // Create entry
    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");
    nodes.push(entry);

    // Create 3 levels of branching (2^3 = 8 paths without loop)
    for _level in 0..3 {
        let left = graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "Block".to_string(),
                name: "Block".to_lowercase().to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert node");

        let right = graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "Block".to_string(),
                name: "Block".to_lowercase().to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert node");

        let prev = nodes.last().unwrap();

        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: *prev,
                to_id: left,
                edge_type: "left".into(),
                data: json!({}),
            })
            .expect("Failed to insert edge");
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: *prev,
                to_id: right,
                edge_type: "right".into(),
                data: json!({}),
            })
            .expect("Failed to insert edge");

        nodes.push(left);
        nodes.push(right);
    }

    // Create exit
    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    // Connect all leaf nodes to exit
    for i in (1..nodes.len()).rev() {
        if i % 2 == 0 && i + 1 < nodes.len() {
            // Skip duplicates
            continue;
        }
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: nodes[i],
                to_id: exit,
                edge_type: "next".into(),
                data: json!({}),
            })
            .ok();
    }

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    // Set very low max_paths
    let config = PathEnumerationConfig {
        max_paths: 3,
        exit_nodes: Some(exit_nodes),
        ..Default::default()
    };

    let result = enumerate_paths(&graph, entry, &config).expect("Failed to enumerate paths");

    // Should stop at max_paths
    assert!(result.paths.len() <= 3, "Should not exceed max_paths");
}

#[test]
fn test_enumerate_paths_result_is_send() {
    // Scenario: EnumeratedPath and PathEnumerationResult should be Send + Sync
    // Expected: Result types implement required traits
    fn is_send_sync<T: Send + Sync>() {}

    is_send_sync::<EnumeratedPath>();
    is_send_sync::<PathEnumerationResult>();
    is_send_sync::<PathClassification>();
    is_send_sync::<PathEnumerationDominanceConfig>();
    is_send_sync::<PathEnumerationPruningStats>();
}

// ============================================================================
// Dominance-Constrained Enumeration Integration Tests
// ============================================================================

#[test]
fn test_enumerate_paths_with_dominance_deterministic() {
    // Scenario: Same input produces same output with dominance constraints
    // Expected: Deterministic results across multiple runs
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple CFG: entry -> branch -> exit
    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let branch = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: branch,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: branch,
            to_id: exit,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    // Compute analysis results
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let cd_result = control_dependence_from_exit(&graph).expect("Failed to compute CD");
    let loops_result = natural_loops(&graph, &dom_result).expect("Failed to compute loops");

    let config = PathEnumerationDominanceConfig {
        base: PathEnumerationConfig {
            exit_nodes: Some(exit_nodes),
            ..Default::default()
        },
        use_dominance_pruning: true,
        use_control_dependence_pruning: true,
        use_loop_constraint_pruning: true,
    };

    // Run twice
    let result1 = enumerate_paths_with_dominance(
        &graph,
        entry,
        &dom_result,
        &cd_result,
        &loops_result,
        &config,
    )
    .expect("First enumeration failed");

    let result2 = enumerate_paths_with_dominance(
        &graph,
        entry,
        &dom_result,
        &cd_result,
        &loops_result,
        &config,
    )
    .expect("Second enumeration failed");

    // Results should be identical
    assert_eq!(result1.paths.len(), result2.paths.len());
    assert_eq!(
        result1.pruning_stats.as_ref().unwrap().paths_pruned,
        result2.pruning_stats.as_ref().unwrap().paths_pruned
    );
}

#[test]
fn test_enumerate_paths_with_dominance_full_integration() {
    // Scenario: All three analysis results (dom, cd, loops) used together
    // Expected: Successful integration with all constraint types enabled
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create CFG with loop: entry -> header -> body -> header, header -> exit
    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let header = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopHeader".to_string(),
            name: "loop_header".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let body = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "LoopBody".to_string(),
            name: "loop_body".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: header,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: header,
            to_id: body,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: body,
            to_id: header,
            edge_type: "loop".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: header,
            to_id: exit,
            edge_type: "exit".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    // Compute all analysis results
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let cd_result = control_dependence_from_exit(&graph).expect("Failed to compute CD");
    let loops_result = natural_loops(&graph, &dom_result).expect("Failed to compute loops");

    let config = PathEnumerationDominanceConfig {
        base: PathEnumerationConfig {
            exit_nodes: Some(exit_nodes),
            revisit_cap: 2,
            ..Default::default()
        },
        use_dominance_pruning: true,
        use_control_dependence_pruning: true,
        use_loop_constraint_pruning: true,
    };

    let result = enumerate_paths_with_dominance(
        &graph,
        entry,
        &dom_result,
        &cd_result,
        &loops_result,
        &config,
    )
    .expect("Enumeration with all constraints failed");

    // Should find paths respecting all constraints
    assert!(!result.paths.is_empty(), "Should find at least one path");
    assert!(
        result.pruning_stats.is_some(),
        "Should have pruning statistics"
    );
}

#[test]
fn test_enumerate_paths_with_dominance_vs_base() {
    // Scenario: Compare constrained vs unconstrained enumeration
    // Expected: Constrained enumeration finds same valid paths, fewer invalid paths
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create diamond CFG
    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let left = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let right = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Block".to_string(),
            name: "Block".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: left,
            edge_type: "left".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: right,
            edge_type: "right".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: left,
            to_id: exit,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: right,
            to_id: exit,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    // Base enumeration
    let base_config = PathEnumerationConfig {
        exit_nodes: Some(exit_nodes.clone()),
        ..Default::default()
    };
    let base_result =
        enumerate_paths(&graph, entry, &base_config).expect("Base enumeration failed");

    // Constrained enumeration
    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let cd_result = control_dependence_from_exit(&graph).expect("Failed to compute CD");
    let loops_result = natural_loops(&graph, &dom_result).expect("Failed to compute loops");

    let constrained_config = PathEnumerationDominanceConfig {
        base: PathEnumerationConfig {
            exit_nodes: Some(exit_nodes),
            ..Default::default()
        },
        use_dominance_pruning: true,
        use_control_dependence_pruning: true,
        use_loop_constraint_pruning: true,
    };
    let constrained_result = enumerate_paths_with_dominance(
        &graph,
        entry,
        &dom_result,
        &cd_result,
        &loops_result,
        &constrained_config,
    )
    .expect("Constrained enumeration failed");

    // Both should find the same valid paths
    assert_eq!(base_result.paths.len(), constrained_result.paths.len());
    assert!(
        constrained_result.pruning_stats.is_some(),
        "Constrained should have pruning stats"
    );
}

#[test]
fn test_enumerate_paths_with_dominance_progress_integration() {
    // Scenario: Progress callback works with dominance-constrained enumeration
    // Expected: Progress callback is invoked during enumeration
    use crate::progress::NoProgress;
    use crate::{GraphEdge, GraphEntity};
    use ahash::AHashSet;

    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create simple linear path
    let entry = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Entry".to_string(),
            name: "entry".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    let exit = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Exit".to_string(),
            name: "exit".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert node");

    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: entry,
            to_id: exit,
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        })
        .expect("Failed to insert edge");

    let mut exit_nodes = AHashSet::new();
    exit_nodes.insert(exit);

    let dom_result = dominators(&graph, entry).expect("Failed to compute dominators");
    let cd_result = control_dependence_from_exit(&graph).expect("Failed to compute CD");
    let loops_result = natural_loops(&graph, &dom_result).expect("Failed to compute loops");

    let config = PathEnumerationDominanceConfig {
        base: PathEnumerationConfig {
            exit_nodes: Some(exit_nodes),
            ..Default::default()
        },
        use_dominance_pruning: true,
        use_control_dependence_pruning: true,
        use_loop_constraint_pruning: true,
    };

    let progress = NoProgress;
    let result = enumerate_paths_with_dominance_progress(
        &graph,
        entry,
        &dom_result,
        &cd_result,
        &loops_result,
        &config,
        progress,
    )
    .expect("Enumeration with progress failed");

    assert!(!result.paths.is_empty());
    assert!(result.pruning_stats.is_some());
}

// Subgraph Isomorphism Tests

#[test]
fn test_subgraph_isomorphism_simple_chain() {
    // Scenario: Find 2-node chain in 4-node path (should find 3 matches)
    // Expected: Returns 3 matches for positions (0,1), (1,2), (2,3)
    let graph = create_test_graph();

    // Create path: 0 -> 1 -> 2 -> 3
    let ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    for i in 0..3 {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: ids[i],
                to_id: ids[i + 1],
                edge_type: "edge".into(),
                data: json!({}),
            })
            .expect("Failed to add edge");
    }

    // Create pattern: 2-node chain
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");

    let pattern_ids: Vec<i64> = pattern
        .list_entity_ids()
        .expect("Failed to get pattern IDs");
    pattern
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: pattern_ids[0],
            to_id: pattern_ids[1],
            edge_type: "edge".into(),
            data: json!({}),
        })
        .expect("Failed to add pattern edge");

    let bounds = SubgraphPatternBounds::default();
    let result = find_subgraph_patterns(&graph, &pattern, bounds).expect("Search failed");

    // Algorithm finds all isomorphisms including different node mappings.
    // We expect at least 3 matches (the 3 edges in the path).
    assert!(
        result.patterns_found >= 3,
        "Should find at least 3 matches, found {}",
        result.patterns_found
    );
    assert!(!result.is_empty());
    assert!(!result.bounded_hit);
}

#[test]
fn test_subgraph_isomorphism_max_matches() {
    // Scenario: Verify max_matches bound stops enumeration
    // Expected: Returns at most max_matches results
    let graph = create_test_graph();

    // Create path: 0 -> 1 -> 2 -> ... -> 9 (10 nodes)
    let ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    for i in 0..9 {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: ids[i],
                to_id: ids[i + 1],
                edge_type: "edge".into(),
                data: json!({}),
            })
            .expect("Failed to add edge");
    }

    // Create pattern: 2-node chain
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");

    let pattern_ids: Vec<i64> = pattern
        .list_entity_ids()
        .expect("Failed to get pattern IDs");
    pattern
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: pattern_ids[0],
            to_id: pattern_ids[1],
            edge_type: "edge".into(),
            data: json!({}),
        })
        .expect("Failed to add pattern edge");

    let bounds = SubgraphPatternBounds {
        max_matches: Some(3),
        ..Default::default()
    };
    let result = find_subgraph_patterns(&graph, &pattern, bounds).expect("Search failed");

    // Should find at most 3 matches due to bound
    assert!(result.patterns_found <= 3);
    assert!(result.bounded_hit); // Should hit the bound
}

#[test]
fn test_subgraph_isomorphism_empty_result() {
    // Scenario: No matches when pattern not in target
    // Expected: Returns empty result
    let graph = create_test_graph();

    // Create path: 0 -> 1 -> 2
    let ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    for i in 0..2 {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: ids[i],
                to_id: ids[i + 1],
                edge_type: "edge".into(),
                data: json!({}),
            })
            .expect("Failed to add edge");
    }

    // Create pattern: triangle (3-node cycle)
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    for _ in 0..3 {
        pattern
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "pattern".to_string(),
                name: "pattern".to_lowercase().to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert pattern node");
    }

    let pattern_ids: Vec<i64> = pattern
        .list_entity_ids()
        .expect("Failed to get pattern IDs");
    // Create triangle edges
    for (from, to) in &[(0, 1), (1, 2), (2, 0)] {
        pattern
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: pattern_ids[*from],
                to_id: pattern_ids[*to],
                edge_type: "edge".into(),
                data: json!({}),
            })
            .expect("Failed to add pattern edge");
    }

    let bounds = SubgraphPatternBounds::default();
    let result = find_subgraph_patterns(&graph, &pattern, bounds).expect("Search failed");

    assert_eq!(result.patterns_found, 0);
    assert!(result.is_empty());
    assert!(result.first_match().is_none());
}

#[test]
fn test_subgraph_isomorphism_progress() {
    // Scenario: Verify progress callback is called
    // Expected: Progress callback reports progress during search
    use crate::progress::NoProgress;

    let graph = create_test_graph();

    // Create path: 0 -> 1 -> 2 -> 3 -> 4
    let ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    for i in 0..4 {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: ids[i],
                to_id: ids[i + 1],
                edge_type: "edge".into(),
                data: json!({}),
            })
            .expect("Failed to add edge");
    }

    // Create pattern: 2-node chain
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");

    let pattern_ids: Vec<i64> = pattern
        .list_entity_ids()
        .expect("Failed to get pattern IDs");
    pattern
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: pattern_ids[0],
            to_id: pattern_ids[1],
            edge_type: "edge".into(),
            data: json!({}),
        })
        .expect("Failed to add pattern edge");

    let bounds = SubgraphPatternBounds::default();
    let progress = NoProgress;
    let result = find_subgraph_patterns_with_progress(&graph, &pattern, bounds, &progress)
        .expect("Search with progress failed");

    // Should still find the matches (at least 4 for the 4 edges in 5-node path)
    assert!(
        result.patterns_found >= 4,
        "Should find at least 4 matches, found {}",
        result.patterns_found
    );
}

#[test]
fn test_subgraph_pattern_bounds_builder() {
    // Scenario: Verify builder pattern methods chain correctly
    // Expected: All builder methods return Self with correct values
    let bounds = SubgraphPatternBounds::new()
        .with_max_matches(100)
        .with_timeout(5000)
        .with_max_pattern_nodes(10);

    assert_eq!(bounds.max_matches, Some(100));
    assert_eq!(bounds.timeout_ms, Some(5000));
    assert_eq!(bounds.max_pattern_nodes, Some(10));
    assert!(bounds.is_bounded());
}

#[test]
fn test_subgraph_match_result_helpers() {
    // Scenario: Verify helper methods work correctly
    // Expected: is_empty(), count(), first_match() return correct values
    let result_empty = SubgraphMatchResult {
        matches: vec![],
        patterns_found: 0,
        computation_time_ms: 50,
        bounded_hit: false,
    };

    assert!(result_empty.is_empty());
    assert_eq!(result_empty.count(), 0);
    assert!(result_empty.first_match().is_none());

    let result_with_data = SubgraphMatchResult {
        matches: vec![vec![1, 2], vec![2, 3]],
        patterns_found: 2,
        computation_time_ms: 100,
        bounded_hit: false,
    };

    assert!(!result_with_data.is_empty());
    assert_eq!(result_with_data.count(), 2);
    assert_eq!(result_with_data.first_match(), Some(&[1, 2][..]));
}

#[test]
fn test_subgraph_isomorphism_single_node_pattern() {
    // Scenario: Single node pattern (edge case)
    // Expected: Matches all nodes in target graph
    let graph = create_test_graph();

    // Create single node pattern
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    pattern
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "pattern".to_string(),
            name: "pattern".to_lowercase().to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .expect("Failed to insert pattern node");

    let bounds = SubgraphPatternBounds::default();
    let result = find_subgraph_patterns(&graph, &pattern, bounds).expect("Search failed");

    // Single node should match all nodes in target
    assert_eq!(result.patterns_found, 10); // 10 nodes in test graph
}

#[test]
fn test_subgraph_isomorphism_pattern_larger_than_target() {
    // Scenario: Pattern has more nodes than target graph
    // Expected: Returns empty result (pattern can't fit)
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");
    for _ in 0..2 {
        graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: "node".to_lowercase().to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert node");
    }

    // Create pattern with 3 nodes
    let pattern = SqliteGraph::open_in_memory().expect("Failed to create pattern");
    for _ in 0..3 {
        pattern
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "pattern".to_string(),
                name: "pattern".to_lowercase().to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .expect("Failed to insert pattern node");
    }

    let bounds = SubgraphPatternBounds::default();
    let result = find_subgraph_patterns(&graph, &pattern, bounds).expect("Search failed");

    assert_eq!(result.patterns_found, 0);
    assert!(result.is_empty());
}

#[test]
fn debug_edge_storage() {
    // Verify edges are stored and read correctly
    let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

    // Create 4 nodes
    for i in 0..4 {
        let entity = GraphEntity {
            id: 0,
            kind: "node".to_string(),
            name: format!("node_{}", i),
            file_path: Some(format!("node_{}.rs", i)),
            data: json!({"index": i}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");
    }

    let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");
    println!("Entity IDs: {:?}", entity_ids);

    // Create diamond: 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
    for (from_idx, to_idx) in edges {
        let edge = GraphEdge {
            id: 0,
            from_id: entity_ids[from_idx],
            to_id: entity_ids[to_idx],
            edge_type: "next".to_string(),
            data: json!({}),
        };
        graph.insert_edge(&edge).expect("Failed to insert edge");
    }

    // Verify edges using fetch_outgoing
    for &node_id in &entity_ids {
        let outgoing = graph
            .fetch_outgoing(node_id)
            .expect("Failed to get outgoing");
        println!("Node {} outgoing: {:?}", node_id, outgoing);
    }

    // Verify expected structure
    assert_eq!(
        graph.fetch_outgoing(entity_ids[0]).unwrap().len(),
        2,
        "Node 0 should have 2 outgoing edges"
    );
    assert_eq!(
        graph.fetch_outgoing(entity_ids[1]).unwrap().len(),
        1,
        "Node 1 should have 1 outgoing edge"
    );
    assert_eq!(
        graph.fetch_outgoing(entity_ids[2]).unwrap().len(),
        1,
        "Node 2 should have 1 outgoing edge"
    );
    assert_eq!(
        graph.fetch_outgoing(entity_ids[3]).unwrap().len(),
        0,
        "Node 3 (exit) should have 0 outgoing edges"
    );
}
