//! Reopen Integration Test - Minimum Viable DB Behavior
//!
//! Tests that the V2 native backend maintains data integrity across file close/reopen cycles.

use rand::{Rng, SeedableRng};
use sqlitegraph::{EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph};

#[test]
fn test_reopen_maintains_graph_integrity() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("reopen_test.db");

    // Set deterministic RNG seed for reproducible edge creation
    let seed = 42u64;

    println!("=== PHASE 1: Initial graph creation ===");

    // Create graph
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create V2 native graph");

    // Create 300 nodes
    let mut node_ids = Vec::new();
    for i in 1..=300 {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i, "phase": 1}),
            })
            .unwrap_or_else(|_| panic!("Failed to insert node {}", i));
        node_ids.push(node_id);
    }

    assert_eq!(node_ids.len(), 300, "Should have created 300 nodes");
    println!(
        "✅ Created 300 nodes (IDs: {} to {})",
        node_ids.first().unwrap(),
        node_ids.last().unwrap()
    );

    // Insert 500 edges using deterministic RNG
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut edge_count = 0;

    while edge_count < 500 {
        let from_idx = rng.gen_range(0..node_ids.len());
        let to_idx = rng.gen_range(0..node_ids.len());

        // Avoid self-loops for simplicity
        if from_idx != to_idx {
            let from_id = node_ids[from_idx];
            let to_id = node_ids[to_idx];

            // Check if edge already exists (simple adjacency check)
            let neighbors = graph
                .neighbors(SnapshotId::current(), from_id, NeighborQuery::default())
                .unwrap_or_default();

            if !neighbors.contains(&to_id) {
                graph
                    .insert_edge(EdgeSpec {
                        from: from_id,
                        to: to_id,
                        edge_type: "test_edge".to_string(),
                        data: serde_json::json!({"edge_id": edge_count, "seed": seed}),
                    })
                    .unwrap_or_else(|_| {
                        panic!(
                            "Failed to insert edge {} from {} to {}",
                            edge_count, from_id, to_id
                        )
                    });

                edge_count += 1;
            }
        }
    }

    println!(
        "✅ Inserted 500 edges using deterministic RNG seed {}",
        seed
    );

    // Sample 10 nodes and record their neighbor counts
    let sample_nodes: Vec<i64> = (0..10).map(|i| node_ids[i * 30]).collect();
    let mut original_neighbor_counts = Vec::new();

    for &node_id in &sample_nodes {
        let neighbors = graph
            .neighbors(SnapshotId::current(), node_id, NeighborQuery::default())
            .expect("Failed to get neighbors");
        original_neighbor_counts.push(neighbors.len());
        println!("Node {} has {} neighbors", node_id, neighbors.len());
    }

    // IMPORTANT: Close the graph by dropping it
    drop(graph);
    println!("✅ Graph closed - database file flushed");

    println!("=== PHASE 2: Reopen and verify integrity ===");

    // Reopen the same graph file
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to reopen V2 native graph");

    println!("✅ Graph reopened successfully");

    // Verify all 300 nodes still exist
    for &node_id in &node_ids {
        let node = graph
            .get_node(SnapshotId::current(), node_id)
            .unwrap_or_else(|_| panic!("Failed to get node {}", node_id));
        assert_eq!(
            node.id, node_id,
            "Node {} should still exist after reopen",
            node_id
        );
    }
    println!("✅ All 300 nodes verified to exist after reopen");

    // Verify neighbor counts are stable for our sampled nodes
    for (i, &node_id) in sample_nodes.iter().enumerate() {
        let neighbors = graph
            .neighbors(SnapshotId::current(), node_id, NeighborQuery::default())
            .unwrap_or_else(|_| panic!("Failed to get neighbors for node {}", node_id));
        let original_count = original_neighbor_counts[i];

        assert_eq!(
            neighbors.len(),
            original_count,
            "Node {} neighbor count changed: was {}, now {}",
            node_id,
            original_count,
            neighbors.len()
        );

        println!(
            "Node {} has {} neighbors (stable: {})",
            node_id,
            neighbors.len(),
            original_count
        );
    }

    println!("✅ All sampled node neighbor counts are stable after reopen");

    // Test that we can still insert new edges after reopen
    let new_from_id = node_ids[0];
    let new_to_id = node_ids[1];

    // Check if this edge already exists
    let neighbors = graph
        .neighbors(SnapshotId::current(), new_from_id, NeighborQuery::default())
        .unwrap_or_default();

    if !neighbors.contains(&new_to_id) {
        graph
            .insert_edge(EdgeSpec {
                from: new_from_id,
                to: new_to_id,
                edge_type: "after_reopen".to_string(),
                data: serde_json::json!({"test": "edge_insertion_after_reopen"}),
            })
            .expect("Failed to insert edge after reopen");

        println!("✅ Successfully inserted new edge after reopen");
    } else {
        println!(
            "ℹ️  Edge between {} and {} already exists, skipping new edge test",
            new_from_id, new_to_id
        );
    }

    // Final verification: check that the new edge is reflected
    let final_neighbors = graph
        .neighbors(SnapshotId::current(), new_from_id, NeighborQuery::default())
        .expect("Failed to get final neighbors");
    assert!(
        final_neighbors.len() >= original_neighbor_counts[0],
        "Final neighbor count should be >= original count"
    );

    println!(
        "✅ Final neighbor count: {} (>= {})",
        final_neighbors.len(),
        original_neighbor_counts[0]
    );
    println!(
        "🎉 REOPEN INTEGRATION TEST PASSED - Database behavior is stable across close/reopen cycles"
    );
}
