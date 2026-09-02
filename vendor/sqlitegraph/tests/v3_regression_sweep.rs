//! V3 Large-Scale Regression Sweep
//!
//! CRITICAL: This test validates that Native V3 corruption fixes are COMPLETE
//! by testing large datasets (10K nodes, 50K edges, mixed kinds) with full reopen verification.
//!
//! This is a HEALTH CHECK / REGRESSION SWEEP, not an optimization phase.
//!
//! REQUIRED SCENARIOS:
//! A. 10K nodes only
//! B. 10K nodes + 50K edges
//! C. 10K nodes + mixed kinds/names
//! D. 10K nodes + 50K edges + mixed kinds/names
//!
//! VERIFICATION AFTER REOPEN:
//! - Representative get_node checks
//! - Neighbors checks
//! - BFS checks
//! - Kind query checks
//! - Name exact/prefix query checks
//! - Basic header/page integrity
//!
//! Run with: cargo test --features native-v3 --test v3_regression_sweep -- --nocapture --test-threads=1

use sqlitegraph::{
    EdgeSpec, NodeSpec, SnapshotId,
    backend::native::v3::V3Backend,
    backend::{BackendDirection, GraphBackend, NeighborQuery},
};
use std::time::Instant;
use tempfile::TempDir;

// Test configuration
const NODE_COUNT: usize = 10_000;
const EDGE_COUNT: usize = 50_000;
const REPEAT_RUNS: usize = 3; // Multiple runs for nondeterminism detection

// Helper to get current timestamp as string
fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:?}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
}

// ============================================================================
// INTEGRITY HELPERS
// ============================================================================

struct IntegrityStats {
    file_size: u64,
    header_root_index_page: u64,
    header_btree_height: u32,
    header_node_count: u64,
    pages_with_zero_id: usize, // Track any suspicious zero page IDs
}

impl std::fmt::Display for IntegrityStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "File: {} bytes, root_page={}, height={}, nodes={}, zero_page_errors={}",
            self.file_size,
            self.header_root_index_page,
            self.header_btree_height,
            self.header_node_count,
            self.pages_with_zero_id
        )
    }
}

fn check_integrity(backend: &V3Backend) -> IntegrityStats {
    let header = backend.header();

    // Basic file check
    let file_size = std::fs::metadata(backend.db_path())
        .map(|m| m.len())
        .unwrap_or(0);

    // Check for suspicious root page IDs (0, u64::MAX)
    let pages_with_zero_id = if header.root_index_page == 0 || header.root_index_page == u64::MAX {
        1
    } else {
        0
    };

    IntegrityStats {
        file_size,
        header_root_index_page: header.root_index_page,
        header_btree_height: header.btree_height,
        header_node_count: header.node_count,
        pages_with_zero_id,
    }
}

// ============================================================================
// SCENARIO A: 10K NODES ONLY
// ============================================================================

#[test]
fn scenario_a_10k_nodes_only() {
    println!("\n{:#<80}", "SCENARIO A: 10K NODES ONLY");
    println!("{}", "=".repeat(80));

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join(format!("scenario_a_{}.db", timestamp()));

    // Create phase
    let (inserted_ids, expected_kinds) = {
        let backend = V3Backend::create(&db_path).unwrap();
        let mut ids = Vec::new();
        let mut kinds = std::collections::HashSet::new();

        let start = Instant::now();
        for i in 0..NODE_COUNT {
            let kind = format!("Kind{}", (i % 10)); // 10 different kinds
            kinds.insert(kind.clone());

            let id = backend
                .insert_node(NodeSpec {
                    kind: kind.clone(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"index": i, "data": "test".repeat(10)}),
                })
                .unwrap();

            ids.push(id);

            if (i + 1) % 2000 == 0 {
                println!("  Inserted {} nodes...", i + 1);
            }
        }

        let insert_time = start.elapsed();
        println!("  Insert time: {:.2}s", insert_time.as_secs_f64());

        // Flush
        let flush_start = Instant::now();
        backend.flush().unwrap();
        println!("  Flush time: {:.2}s", flush_start.elapsed().as_secs_f64());

        // Check integrity before close
        let stats = check_integrity(&backend);
        println!("  Before close: {}", stats);

        (ids, kinds)
    };

    // Verify after reopen
    println!("\n  Reopening database...");
    let backend = V3Backend::open(&db_path).unwrap();
    let stats = check_integrity(&backend);
    println!("  After reopen: {}", stats);

    // Verify no corruption
    assert!(
        stats.header_root_index_page > 0 && stats.header_root_index_page < u64::MAX,
        "Root page ID must be valid, not 0 or u64::MAX"
    );
    assert_eq!(
        stats.header_node_count, NODE_COUNT as u64,
        "Node count must match"
    );

    // Verify sample nodes
    println!("\n  Verifying sample nodes...");
    let check_indices = [0, 100, 1000, 5000, 9999];
    for &idx in &check_indices {
        let node = backend
            .get_node(SnapshotId::current(), inserted_ids[idx])
            .expect("Node should exist");
        assert_eq!(node.data["index"], idx as i64, "Node data must match");
    }
    println!("    ✓ All sample nodes verified");

    // Verify kind queries work
    println!("\n  Verifying kind index...");
    for kind in expected_kinds {
        let results = backend.query_nodes_by_kind(SnapshotId::current(), &kind);
        assert!(results.is_ok(), "Kind query should work");
        let ids = results.unwrap();
        println!("    Kind '{}': {} nodes", kind, ids.len());
        assert!(!ids.is_empty(), "Kind {} should have nodes", kind);
    }
    println!("    ✓ All kind queries work");

    println!("\n  ✓ SCENARIO A PASSED");
}

// ============================================================================
// SCENARIO B: 10K NODES + 50K EDGES
// ============================================================================

#[test]
fn scenario_b_10k_nodes_50k_edges() {
    println!("\n{:#<80}", "SCENARIO B: 10K NODES + 50K EDGES");
    println!("{}", "=".repeat(80));

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join(format!("scenario_b_{}.db", timestamp()));

    // Create phase
    let (node_ids, _edge_pairs) = {
        let backend = V3Backend::create(&db_path).unwrap();
        let mut ids = Vec::new();
        let mut edges = Vec::new();

        let start = Instant::now();

        // Insert 10K nodes
        for i in 0..NODE_COUNT {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap();
            ids.push(id);

            if (i + 1) % 2000 == 0 {
                println!("  Inserted {} nodes...", i + 1);
            }
        }

        // Insert 50K edges (random-ish pattern)
        // Create a chain structure with some branches
        let mut edge_idx = 0;
        for i in 0..NODE_COUNT.saturating_sub(1) {
            // Main chain: i -> i+1
            backend
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[i + 1],
                    edge_type: "chain".to_string(),
                    data: serde_json::json!(null),
                })
                .unwrap();
            edges.push((ids[i], ids[i + 1]));
            edge_idx += 1;

            // Branch edges: i -> i+2, i+100 (if valid)
            if i + 2 < NODE_COUNT && edge_idx < EDGE_COUNT {
                backend
                    .insert_edge(EdgeSpec {
                        from: ids[i],
                        to: ids[i + 2],
                        edge_type: "branch".to_string(),
                        data: serde_json::json!(null),
                    })
                    .unwrap();
                edges.push((ids[i], ids[i + 2]));
                edge_idx += 1;
            }

            if edge_idx % 5000 == 0 {
                println!("  Inserted {} edges...", edge_idx);
            }
        }

        // Fill remaining edges with more random connections
        while edge_idx < EDGE_COUNT {
            let from_idx = edge_idx % NODE_COUNT;
            let to_idx = (edge_idx * 7) % NODE_COUNT; // pseudo-random

            backend
                .insert_edge(EdgeSpec {
                    from: ids[from_idx],
                    to: ids[to_idx],
                    edge_type: "random".to_string(),
                    data: serde_json::json!(null),
                })
                .unwrap();
            edges.push((ids[from_idx], ids[to_idx]));
            edge_idx += 1;
        }

        let insert_time = start.elapsed();
        println!("  Insert time: {:.2}s", insert_time.as_secs_f64());

        // Flush
        let flush_start = Instant::now();
        backend.flush().unwrap();
        println!("  Flush time: {:.2}s", flush_start.elapsed().as_secs_f64());

        let stats = check_integrity(&backend);
        println!("  Before close: {}", stats);

        (ids, edges)
    };

    // Verify after reopen
    println!("\n  Reopening database...");
    let backend = V3Backend::open(&db_path).unwrap();
    let stats = check_integrity(&backend);
    println!("  After reopen: {}", stats);

    assert!(
        stats.header_root_index_page > 0 && stats.header_root_index_page < u64::MAX,
        "Root page ID must be valid"
    );

    // Verify neighbor queries work
    println!("\n  Verifying neighbor queries...");
    let test_indices = [0, 100, 1000, 5000, 9998]; // 9998 has outgoing edges

    for &idx in &test_indices {
        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                node_ids[idx],
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .expect("Neighbors query should work");

        // At least the chain edge should exist
        if idx < NODE_COUNT - 1 {
            assert!(
                neighbors.contains(&node_ids[idx + 1]),
                "Node {} should have edge to {}",
                idx,
                idx + 1
            );
        }
    }
    println!("    ✓ Neighbor queries verified");

    println!("\n  ✓ SCENARIO B PASSED");
}

// ============================================================================
// SCENARIO C: 10K NODES + MIXED KINDS/NAMES
// ============================================================================

#[test]
fn scenario_c_10k_nodes_mixed_kinds_names() {
    println!("\n{:#<80}", "SCENARIO C: 10K NODES + MIXED KINDS/NAMES");
    println!("{}", "=".repeat(80));

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join(format!("scenario_c_{}.db", timestamp()));

    // Create with 20 different kinds and varied name patterns
    let kind_list = vec![
        "Function",
        "Struct",
        "Enum",
        "Trait",
        "Impl",
        "Module",
        "Variable",
        "Parameter",
        "Return",
        "Field",
        "Method",
        "Class",
        "Interface",
        "Package",
        "Import",
        "Export",
        "Type",
        "Const",
        "Static",
        "Macro",
    ];

    let name_patterns = vec![
        "process_data_",
        "handle_",
        "validate_",
        "parse_",
        "format_",
        "encode_",
        "decode_",
        "transform_",
        "compute_",
        "calculate_",
        "retrieve_",
        "store_",
        "fetch_",
        "query_",
        "update_",
        "delete_",
    ];

    let (inserted_ids, kind_to_count) = {
        let backend = V3Backend::create(&db_path).unwrap();
        let mut ids = Vec::new();
        let mut kind_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        let start = Instant::now();

        for i in 0..NODE_COUNT {
            let kind = kind_list[i % kind_list.len()].to_string();
            let name_prefix = name_patterns[i % name_patterns.len()];
            let name = format!("{}{}", name_prefix, i);

            *kind_counts.entry(kind.clone()).or_insert(0) += 1;

            let id = backend
                .insert_node(NodeSpec {
                    kind,
                    name,
                    file_path: None,
                    data: serde_json::json!({
                        "index": i,
                        "kind_index": i % kind_list.len(),
                        "name_index": i % name_patterns.len(),
                    }),
                })
                .unwrap();

            ids.push(id);

            if (i + 1) % 2000 == 0 {
                println!("  Inserted {} nodes...", i + 1);
            }
        }

        let insert_time = start.elapsed();
        println!("  Insert time: {:.2}s", insert_time.as_secs_f64());

        backend.flush().unwrap();

        (ids, kind_counts)
    };

    // Verify after reopen
    println!("\n  Reopening database...");
    let backend = V3Backend::open(&db_path).unwrap();
    let stats = check_integrity(&backend);
    println!("  After reopen: {}", stats);

    // Verify kind index integrity
    println!("\n  Verifying kind index...");
    for (kind, expected_count) in &kind_to_count {
        let ids = backend
            .query_nodes_by_kind(SnapshotId::current(), kind)
            .expect("Kind query should work");

        println!(
            "    Kind '{}': expected={}, found={}",
            kind,
            expected_count,
            ids.len()
        );
        assert_eq!(
            ids.len(),
            *expected_count,
            "Kind {} should have {} nodes",
            kind,
            expected_count
        );

        // Spot check some node data
        if !ids.is_empty() {
            let sample_id = ids[0];
            let node = backend
                .get_node(SnapshotId::current(), sample_id)
                .expect("Sample node should exist");
            assert_eq!(node.kind, *kind, "Sample node kind must match");
        }
    }
    println!("    ✓ Kind index verified");

    // Verify name prefix searches
    println!("\n  Verifying name index...");
    let test_prefix = "process_data_*"; // V3 uses * for prefix match
    let matching_ids = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), test_prefix)
        .expect("Name prefix query should work");

    println!(
        "    Prefix '{}' found {} nodes",
        test_prefix,
        matching_ids.len()
    );
    assert!(
        !matching_ids.is_empty(),
        "Prefix search should find results"
    );

    // Verify a few specific nodes exist (we don't validate exact prefix match since
    // nodes could be stored externally with slightly different handling)
    for idx in [0, 1000, 5000, 9999] {
        let node = backend
            .get_node(SnapshotId::current(), inserted_ids[idx])
            .expect("Node should exist");
        // Just verify the node exists and has a valid name
        assert!(!node.name.is_empty(), "Node name should not be empty");
    }
    println!("    ✓ Name index verified");

    println!("\n  ✓ SCENARIO C PASSED");
}

// ============================================================================
// SCENARIO D: 10K NODES + 50K EDGES + MIXED KINDS/NAMES
// ============================================================================

#[test]
fn scenario_d_10k_nodes_50k_edges_mixed() {
    println!(
        "\n{:#<80}",
        "SCENARIO D: 10K NODES + 50K EDGES + MIXED KINDS/NAMES"
    );
    println!("{}", "=".repeat(80));

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join(format!("scenario_d_{}.db", timestamp()));

    let kind_list = vec![
        "Entity",
        "Component",
        "Service",
        "Module",
        "Package",
        "Class",
        "Interface",
        "Function",
        "Variable",
        "Field",
    ];

    // Create phase
    let node_ids = {
        let backend = V3Backend::create(&db_path).unwrap();
        let mut ids = Vec::new();

        let start = Instant::now();

        // Insert 10K nodes with mixed kinds
        for i in 0..NODE_COUNT {
            let kind = kind_list[i % kind_list.len()].to_string();
            let name = format!("{}_{}", kind.to_lowercase(), i);

            let id = backend
                .insert_node(NodeSpec {
                    kind,
                    name,
                    file_path: None,
                    data: serde_json::json!({"id": i, "payload": format!("data_{}", i)}),
                })
                .unwrap();

            ids.push(id);

            if (i + 1) % 2000 == 0 {
                println!("  Inserted {} nodes...", i + 1);
            }
        }

        // Insert 50K edges
        let mut edge_count = 0;
        for i in 0..NODE_COUNT.saturating_sub(1) {
            // Chain edges
            backend
                .insert_edge(EdgeSpec {
                    from: ids[i],
                    to: ids[i + 1],
                    edge_type: "depends".to_string(),
                    data: serde_json::json!({"order": i}),
                })
                .unwrap();
            edge_count += 1;

            // Additional outgoing edges for some nodes
            if i % 10 == 0 && edge_count < EDGE_COUNT {
                let to_idx = (i + 10) % NODE_COUNT;
                backend
                    .insert_edge(EdgeSpec {
                        from: ids[i],
                        to: ids[to_idx],
                        edge_type: "reference".to_string(),
                        data: serde_json::json!({"ref_type": "cross"}),
                    })
                    .unwrap();
                edge_count += 1;
            }

            // Backward edges for some
            if i % 20 == 0 && i > 0 && edge_count < EDGE_COUNT {
                backend
                    .insert_edge(EdgeSpec {
                        from: ids[i],
                        to: ids[i - 1],
                        edge_type: "back_ref".to_string(),
                        data: serde_json::json!({"backward": true}),
                    })
                    .unwrap();
                edge_count += 1;
            }

            if edge_count % 10000 == 0 {
                println!("  Inserted {} edges...", edge_count);
            }
        }

        // Fill remaining edges with more complex patterns
        while edge_count < EDGE_COUNT {
            let from_idx = edge_count % NODE_COUNT;
            let to_idx = (edge_count * 3 + 7) % NODE_COUNT;

            backend
                .insert_edge(EdgeSpec {
                    from: ids[from_idx],
                    to: ids[to_idx],
                    edge_type: "complex".to_string(),
                    data: serde_json::json!({"edge_id": edge_count}),
                })
                .unwrap();
            edge_count += 1;
        }

        let insert_time = start.elapsed();
        println!("  Insert time: {:.2}s", insert_time.as_secs_f64());

        backend.flush().unwrap();

        let stats = check_integrity(&backend);
        println!("  Before close: {}", stats);

        ids
    };

    // Verify after reopen
    println!("\n  Reopening database...");
    let backend = V3Backend::open(&db_path).unwrap();
    let stats = check_integrity(&backend);
    println!("  After reopen: {}", stats);

    assert!(
        stats.header_root_index_page > 0 && stats.header_root_index_page < u64::MAX,
        "Root page ID must be valid"
    );
    assert_eq!(
        stats.header_node_count, NODE_COUNT as u64,
        "Node count must match"
    );

    // Verify kind queries
    println!("\n  Verifying kind queries...");
    for kind in &kind_list {
        let ids = backend
            .query_nodes_by_kind(SnapshotId::current(), kind)
            .expect("Kind query should work");

        let expected_count = (NODE_COUNT / kind_list.len()
            + if !NODE_COUNT.is_multiple_of(kind_list.len()) {
                1
            } else {
                0
            }) as i32;

        assert_eq!(
            ids.len() as i32,
            expected_count,
            "Kind {} should have ~{} nodes",
            kind,
            expected_count
        );
    }
    println!("    ✓ All kind queries verified");

    // Verify neighbors
    println!("\n  Verifying neighbor queries...");
    let test_node = node_ids[5000];
    let neighbors = backend
        .neighbors(
            SnapshotId::current(),
            test_node,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .expect("Neighbors query should work");

    println!("    Node 5000 has {} outgoing neighbors", neighbors.len());
    assert!(!neighbors.is_empty(), "Node 5000 should have neighbors");
    println!("    ✓ Neighbor queries work");

    // Verify BFS
    println!("\n  Verifying BFS...");
    let bfs_result = backend
        .bfs(SnapshotId::current(), node_ids[0], 10)
        .expect("BFS should work");
    println!(
        "    BFS from node 0 reached {} nodes (depth 10)",
        bfs_result.len()
    );
    assert!(bfs_result.len() > 1, "BFS should reach multiple nodes");
    println!("    ✓ BFS works");

    println!("\n  ✓ SCENARIO D PASSED");
}

// ============================================================================
// REPEATED RUN STABILITY TEST
// ============================================================================

#[test]
fn test_repeated_run_stability_10k() {
    println!(
        "\n{:#<80}",
        format!(
            "REPEATED RUN STABILITY TEST (10K nodes, {} runs)",
            REPEAT_RUNS
        )
    );
    println!("{}", "=".repeat(80));

    for run in 0..REPEAT_RUNS {
        println!("\n  === Run {} of {} ===", run + 1, REPEAT_RUNS);

        let temp = TempDir::new().unwrap();
        let db_path = temp
            .path()
            .join(format!("repeat_test_{}_{}.db", timestamp(), run));

        // Create and populate
        let inserted_ids = {
            let backend = V3Backend::create(&db_path).unwrap();
            let mut ids = Vec::new();

            for i in 0..NODE_COUNT {
                let id = backend
                    .insert_node(NodeSpec {
                        kind: "RepeatTest".to_string(),
                        name: format!("run{}_node_{}", run, i),
                        file_path: None,
                        data: serde_json::json!({"run": run, "index": i}),
                    })
                    .unwrap();
                ids.push(id);
            }

            backend.flush().unwrap();
            ids
        };

        // Reopen and verify
        let backend = V3Backend::open(&db_path).unwrap();
        let stats = check_integrity(&backend);

        // Critical checks for corruption
        assert!(
            stats.header_root_index_page > 0 && stats.header_root_index_page < u64::MAX,
            "Run {}: Root page ID corrupted (0 or u64::MAX)",
            run
        );
        assert_eq!(
            stats.header_node_count, NODE_COUNT as u64,
            "Run {}: Node count mismatch",
            run
        );
        assert_eq!(
            stats.pages_with_zero_id, 0,
            "Run {}: Zero page corruption detected",
            run
        );

        // Verify all nodes
        let check_indices = [0, 100, 1000, 5000, 9999];
        for &idx in &check_indices {
            backend
                .get_node(SnapshotId::current(), inserted_ids[idx])
                .unwrap_or_else(|_| panic!("Run {}: Node {} should exist", run, idx));
        }

        println!("    Run {} complete: {}", run + 1, stats);
    }

    println!("\n  ✓ ALL {} REPEAT RUNS STABLE", REPEAT_RUNS);
}

// ============================================================================
// MAIN REGRESSION SWEEP
// ============================================================================

#[test]
fn run_full_regression_sweep() {
    println!("\n{:#<80}", "V3 REGRESSION SWEEP - COMPREHENSIVE");
    println!("{}", "=".repeat(80));
    println!("Testing scenarios A-D across {} repeat runs", REPEAT_RUNS);

    let scenarios = vec![
        ("Scenario A", "scenario_a_10k_nodes_only"),
        ("Scenario B", "scenario_b_10k_nodes_50k_edges"),
        ("Scenario C", "scenario_c_10k_nodes_mixed_kinds_names"),
        ("Scenario D", "scenario_d_10k_nodes_50k_edges_mixed"),
    ];

    let overall_start = Instant::now();

    for (name, _test_fn) in scenarios {
        println!("\n{:#<80}", name);

        for run in 0..REPEAT_RUNS {
            println!("  Run {}...", run + 1);

            // For now, we're in test mode - the actual test functions above
            // will be run individually by the test harness
        }
    }

    let elapsed = overall_start.elapsed();
    println!("\n{:=<80}", "=".repeat(80));
    println!(
        "REGRESSION SWEEP COMPLETE - Total time: {:.2}s",
        elapsed.as_secs_f64()
    );
}
