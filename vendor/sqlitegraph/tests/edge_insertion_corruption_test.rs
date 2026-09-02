//! Edge Insertion Corruption Test
//!
//! Verifies that edge insertion does not corrupt nodes around boundary IDs.
//! Uses the public GraphBackend API rather than raw file offsets so the test
//! remains valid across storage-format changes (V2 fixed slots → V3 B+Tree pages).

use sqlitegraph::{EdgeSpec, GraphBackend, GraphConfig, NodeSpec, SnapshotId, open_graph};

fn verify_nodes(graph: &dyn GraphBackend, node_ids: &[i64], label: &str) {
    println!("=== {} ===", label);
    for &node_id in node_ids {
        match graph.get_node(SnapshotId::current(), node_id) {
            Ok(node) => {
                println!("Node {}: kind={}, name={}", node_id, node.kind, node.name);
            }
            Err(e) => {
                println!("Node {}: ERROR fetching - {}", node_id, e);
            }
        }
    }
}

fn assert_nodes_intact(graph: &dyn GraphBackend, node_ids: &[i64], context: &str) {
    for &node_id in node_ids {
        let node = graph
            .get_node(SnapshotId::current(), node_id)
            .unwrap_or_else(|e| panic!("{}: failed to fetch node {}: {}", context, node_id, e));

        assert_eq!(
            node.id, node_id,
            "{}: node {} ID mismatch",
            context, node_id
        );
        assert_eq!(
            node.kind, "Node",
            "{}: node {} kind mismatch",
            context, node_id
        );
        assert_eq!(
            node.name,
            format!("node_{}", node_id),
            "{}: node {} name mismatch",
            context,
            node_id
        );
        assert_eq!(
            node.data,
            serde_json::json!({"id": node_id}),
            "{}: node {} data mismatch",
            context,
            node_id
        );
    }
}

#[test]
fn test_edge_insertion_corruption_isolation() {
    println!("=== EDGE INSERTION CORRUPTION ISOLATION TEST ===");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create native V3 graph
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native V3 graph");

    // Create exactly 300 nodes to cross the 256 boundary
    let mut node_ids = Vec::new();
    for i in 1..=300 {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap_or_else(|_| panic!("Failed to insert node {}", i));
        node_ids.push(node_id);
    }

    // Critical nodes to monitor
    let critical_nodes = [255i64, 256, 257, 258, 259, 260];

    verify_nodes(graph.as_ref(), &critical_nodes, "AFTER NODE CREATION");
    assert_nodes_intact(graph.as_ref(), &critical_nodes, "After node creation");

    println!("\n=== BEGINNING EDGE INSERTION WITH MONITORING ===");

    // Insert edges one by one and monitor critical nodes
    for i in 0..299 {
        let from_id = node_ids[i];
        let to_id = node_ids[i + 1];

        // Check critical nodes before inserting this edge
        if (250..=270).contains(&from_id) || (250..=270).contains(&to_id) {
            println!("\n--- EDGE {}: {} -> {} ---", i, from_id, to_id);
            assert_nodes_intact(
                graph.as_ref(),
                &critical_nodes,
                &format!("Before edge {} ({} -> {})", i, from_id, to_id),
            );
        }

        // Insert the edge - this is where corruption might happen
        match graph.insert_edge(EdgeSpec {
            from: from_id,
            to: to_id,
            edge_type: "chain".to_string(),
            data: serde_json::json!({"edge_index": i}),
        }) {
            Ok(_) => {
                // Check critical nodes after successful edge insertion
                if (250..=270).contains(&from_id) || (250..=270).contains(&to_id) {
                    verify_nodes(
                        graph.as_ref(),
                        &critical_nodes,
                        &format!("AFTER EDGE {}: {} -> {}", i, from_id, to_id),
                    );
                    assert_nodes_intact(
                        graph.as_ref(),
                        &critical_nodes,
                        &format!("After edge {} ({} -> {})", i, from_id, to_id),
                    );
                }
            }
            Err(e) => {
                println!("Edge {} ({} -> {}) FAILED: {}", i, from_id, to_id, e);

                if e.to_string().contains("uninitialized slot")
                    || e.to_string().contains("version=0")
                {
                    println!(
                        "🔥 TARGET CORRUPTION DETECTED at edge {} ({} -> {})",
                        i, from_id, to_id
                    );
                    verify_nodes(graph.as_ref(), &critical_nodes, "CORRUPTION DETECTED");
                    panic!(
                        "REPRODUCED: V3 uninitialized slot corruption detected at edge {}",
                        i
                    );
                } else {
                    panic!("Unexpected error (not uninitialized slot): {}", e);
                }
            }
        }
    }

    println!("\n=== EDGE INSERTION COMPLETED SUCCESSFULLY ===");
    verify_nodes(graph.as_ref(), &critical_nodes, "FINAL VERIFICATION");
    assert_nodes_intact(graph.as_ref(), &critical_nodes, "Final verification");

    println!("✅ NO CORRUPTION DETECTED - All edge insertions completed successfully");
}

#[test]
fn test_direct_edge_to_node_257() {
    println!("=== DIRECT EDGE TO NODE 257 TEST ===");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    // Create native V3 graph
    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native V3 graph");

    // Create exactly 257 nodes to focus on boundary
    let mut node_ids = Vec::new();
    for i in 1..=257 {
        let node_id = graph
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap_or_else(|_| panic!("Failed to insert node {}", i));
        node_ids.push(node_id);
    }

    verify_nodes(graph.as_ref(), &[255, 256, 257], "AFTER NODE CREATION");
    assert_nodes_intact(graph.as_ref(), &[255, 256, 257], "After node creation");

    // Try to insert a single edge to node 257 - this should trigger corruption if it exists
    println!("Inserting edge: 256 -> 257");
    match graph.insert_edge(EdgeSpec {
        from: node_ids[255], // node 256
        to: node_ids[256],   // node 257
        edge_type: "test".to_string(),
        data: serde_json::json!({"test": true}),
    }) {
        Ok(_) => {
            println!("Edge 256 -> 257 inserted successfully");
            verify_nodes(graph.as_ref(), &[255, 256, 257], "AFTER EDGE INSERTION");
            assert_nodes_intact(graph.as_ref(), &[255, 256, 257], "After edge insertion");
        }
        Err(e) => {
            println!("Edge 256 -> 257 failed: {}", e);
            if e.to_string().contains("uninitialized slot") || e.to_string().contains("version=0") {
                panic!("REPRODUCED: Node 257 corruption when inserting edge 256 -> 257");
            } else {
                panic!("Different error occurred: {}", e);
            }
        }
    }

    println!("Direct edge to node 257 test completed successfully");
}
