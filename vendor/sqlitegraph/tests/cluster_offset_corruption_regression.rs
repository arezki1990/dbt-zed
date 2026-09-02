//! Cluster Offset Corruption Regression Test
//!
//! Regression test to ensure node data survives edge insertion operations.
//! Tests verify through the GraphBackend API (portable across V2/V3 layouts)
//! rather than reading raw disk bytes (which differ between backend versions).

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

/// Verify a node exists and has the expected name through the GraphBackend API.
fn verify_node(graph: &dyn sqlitegraph::GraphBackend, node_id: i64, expected_name: &str) -> bool {
    let snapshot = SnapshotId::current();
    match graph.get_node(snapshot, node_id) {
        Ok(entity) => entity.name == expected_name,
        Err(_) => false,
    }
}

#[test]
fn test_node_data_survives_edge_insertion() {
    println!("=== NODE DATA SURVIVAL REGRESSION TEST ===");

    let test_sizes = [10, 50, 100, 256, 300, 500, 1000];

    for &size in &test_sizes {
        println!("\n--- Testing graph size: {} nodes ---", size);

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let graph =
            open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native graph");

        // Create nodes
        let mut node_ids = Vec::new();
        for i in 1..=size {
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

        // Verify all nodes are readable immediately after creation
        for (i, &node_id) in node_ids.iter().enumerate() {
            assert!(
                verify_node(graph.as_ref(), node_id, &format!("node_{}", i + 1)),
                "Node {} (id={}) not readable after creation",
                i + 1,
                node_id
            );
        }

        // Create edges in chain pattern (1->2->3->...->size)
        for i in 0..(size as usize) - 1 {
            graph
                .insert_edge(EdgeSpec {
                    from: node_ids[i],
                    to: node_ids[i + 1],
                    edge_type: "chain".to_string(),
                    data: serde_json::json!({"edge_index": i}),
                })
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to insert edge {} -> {}",
                        node_ids[i],
                        node_ids[i + 1]
                    )
                });

            // Spot-check critical nodes around every 100th edge insertion
            if i % 100 == 0 || i == (size as usize) - 2 {
                let sample_indices: Vec<usize> = vec![
                    0,
                    if size >= 50 { size as usize / 4 } else { 0 },
                    if size >= 100 { size as usize / 2 } else { 0 },
                    if size >= 150 {
                        (3 * size as usize) / 4
                    } else {
                        size as usize - 1
                    },
                    size as usize - 1,
                ];

                for &idx in &sample_indices {
                    if idx < node_ids.len() {
                        assert!(
                            verify_node(
                                graph.as_ref(),
                                node_ids[idx],
                                &format!("node_{}", idx + 1)
                            ),
                            "Node {} (id={}) corrupted after edge {} insertion",
                            idx + 1,
                            node_ids[idx],
                            i
                        );
                    }
                }
            }
        }

        // Final verification: all nodes should still be readable
        for (i, &node_id) in node_ids.iter().enumerate() {
            assert!(
                verify_node(graph.as_ref(), node_id, &format!("node_{}", i + 1)),
                "Node {} (id={}) corrupted after all edge insertions",
                i + 1,
                node_id
            );
        }

        println!(
            "✅ Graph size {}: {} nodes and {} edges with NO corruption",
            size,
            size,
            size - 1
        );
    }

    println!("\n🎉 ALL REGRESSION TESTS PASSED - Node data integrity verified!");
}

#[test]
fn test_boundary_conditions_around_node_257() {
    println!("=== BOUNDARY CONDITION TEST AROUND NODE 257 ===");

    let boundary_ranges = [(250, 260), (255, 260), (256, 258), (257, 257), (300, 310)];

    for &(start, end) in &boundary_ranges {
        println!("\n--- Testing nodes {} to {} ---", start, end);

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let graph =
            open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native graph");

        // Create nodes in this range
        let mut node_ids = Vec::new();
        for i in start..=end {
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

        // Create edges between all nodes in this range
        for i in 0..node_ids.len() - 1 {
            for j in i + 1..node_ids.len() {
                graph
                    .insert_edge(EdgeSpec {
                        from: node_ids[i],
                        to: node_ids[j],
                        edge_type: "test".to_string(),
                        data: serde_json::json!({"from": i, "to": j}),
                    })
                    .unwrap_or_else(|_| {
                        panic!("Failed to insert edge {} -> {}", node_ids[i], node_ids[j])
                    });
            }
        }

        // Verify all nodes in range are still readable after edge insertion
        for (idx, &node_id) in node_ids.iter().enumerate() {
            let expected_name = format!("node_{}", start + idx);
            assert!(
                verify_node(graph.as_ref(), node_id, &expected_name),
                "Node {} (id={}) corrupted after edge insertion",
                start + idx,
                node_id
            );
        }

        println!(
            "✅ Nodes {}-{}: NO corruption with {} edges",
            start,
            end,
            (node_ids.len() * (node_ids.len() - 1)) / 2
        );
    }

    println!("\n🎉 ALL BOUNDARY TESTS PASSED - Node 257 boundary is safe!");
}

#[test]
fn test_node_region_calculation_invariants() {
    println!("=== NODE REGION CALCULATION INVARIANTS TEST ===");

    let test_cases = [1, 10, 100, 257, 500, 1000, 10000];

    for &node_count in &test_cases {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let graph =
            open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native graph");

        let mut node_ids = Vec::new();
        for i in 1..=node_count {
            let id = graph
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .expect("insert failed");
            node_ids.push(id);
        }

        // All nodes must be retrievable
        for (i, &id) in node_ids.iter().enumerate() {
            assert!(
                verify_node(graph.as_ref(), id, &format!("node_{}", i + 1)),
                "Node {} (id={}) not retrievable in graph with {} nodes",
                i + 1,
                id,
                node_count
            );
        }

        println!("  ✅ {} nodes: all retrievable", node_count);
    }

    println!("\n🎉 ALL INVARIANT TESTS PASSED - Node storage scales correctly!");
}

#[test]
fn test_comprehensive_edge_patterns() {
    println!("=== COMPREHENSIVE EDGE PATTERNS TEST ===");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let graph =
        open_graph(&db_path, &GraphConfig::native()).expect("Failed to create native graph");

    // Create 1000 nodes for comprehensive testing
    let mut node_ids = Vec::new();
    for i in 1..=1000 {
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

    println!("Created 1000 nodes successfully");

    // Test 1: Chain pattern (1->2->3->...->1000)
    println!("Testing chain pattern...");
    for i in 0..999 {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i + 1],
                edge_type: "chain".to_string(),
                data: serde_json::json!({"chain_index": i}),
            })
            .expect("Chain edge insertion failed");
    }
    println!("✅ Chain pattern: 999 edges inserted");

    // Test 2: Star pattern (node 1 -> all others)
    println!("Testing star pattern...");
    for i in 1..1000 {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[0],
                to: node_ids[i],
                edge_type: "star".to_string(),
                data: serde_json::json!({"star_index": i}),
            })
            .expect("Star edge insertion failed");
    }
    println!("✅ Star pattern: 999 edges inserted");

    // Test 3: Reverse chain (1000->999->...->1)
    println!("Testing reverse chain pattern...");
    for i in (1..1000).rev() {
        graph
            .insert_edge(EdgeSpec {
                from: node_ids[i],
                to: node_ids[i - 1],
                edge_type: "reverse".to_string(),
                data: serde_json::json!({"reverse_index": i}),
            })
            .expect("Reverse chain edge insertion failed");
    }
    println!("✅ Reverse chain pattern: 999 edges inserted");

    // Test 4: Random edges
    println!("Testing random pattern...");
    use rand::{RngCore, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x12345678);

    for _ in 0..1000 {
        let from_idx = (rng.next_u32() as usize) % 1000;
        let mut to_idx = (rng.next_u32() as usize) % 1000;
        while to_idx == from_idx {
            to_idx = (rng.next_u32() as usize) % 1000;
        }

        graph
            .insert_edge(EdgeSpec {
                from: node_ids[from_idx],
                to: node_ids[to_idx],
                edge_type: "random".to_string(),
                data: serde_json::json!({"random_id": rng.next_u64()}),
            })
            .expect("Random edge insertion failed");
    }
    println!("✅ Random pattern: 1000 edges inserted");

    // Final verification: all nodes should still be readable through the API
    println!("Final verification of all nodes...");
    for (i, &node_id) in node_ids.iter().enumerate() {
        assert!(
            verify_node(graph.as_ref(), node_id, &format!("node_{}", i + 1)),
            "Node {} (id={}) corrupted after comprehensive edge testing",
            i + 1,
            node_id
        );
    }

    println!("🎉 COMPREHENSIVE TEST PASSED: 3997 edges with NO corruption!");
}
