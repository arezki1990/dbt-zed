//! V3 Sync Fix Validation Test Suite
//!
//! Comprehensive validation of the per-page sync removal fix.
//! Tests durability, recovery, and performance characteristics.
//!
//! Run with: cargo test --features native-v3 --test v3_sync_fix_validation -- --nocapture

use sqlitegraph::{
    EdgeSpec, NodeSpec, SnapshotId,
    backend::native::v3::V3Backend,
    backend::{BackendDirection, GraphBackend, NeighborQuery},
};
use std::fs::metadata;
use std::time::Instant;

/// Helper to get file size safely
fn file_size(path: &std::path::Path) -> u64 {
    if path.exists() {
        metadata(path).ok().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    }
}

// ============================================================================
// A. INSERT + CLEAN CLOSE + REOPEN
// ============================================================================

#[test]
fn test_a_clean_close_with_flush_reopen_nodes() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_a_flush.graph");

    let node_ids;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        let mut ids = Vec::new();
        for i in 0..100 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"index": i}),
                })
                .unwrap();
            ids.push(id);
        }
        node_ids = ids;

        // CRITICAL: Call flush_to_disk() for durability
        backend.flush_to_disk().expect("Flush must succeed");
    }

    // Reopen and verify
    let backend = V3Backend::open(&db_path).unwrap();

    let all_ids = backend.entity_ids().unwrap();
    assert_eq!(
        all_ids.len(),
        100,
        "All 100 nodes should persist after flush"
    );

    // Spot check some nodes
    for &id in &[node_ids[0], node_ids[50], node_ids[99]] {
        let node = backend.get_node(SnapshotId::current(), id).unwrap();
        assert!(node.data.get("index").is_some());
    }

    println!(
        "[TEST A] Clean close WITH flush - recovered {} nodes",
        all_ids.len()
    );
}

// ============================================================================
// B. INSERT + EXPLICIT FLUSH + REOPEN
// ============================================================================

#[test]
fn test_b_explicit_flush_durability() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_b_flush.graph");
    let wal_path = temp.path().join("test_b_flush.v3wal");

    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 500 nodes
        for i in 0..500 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("n{}", i),
                    file_path: None,
                    data: serde_json::json!({"val": i}),
                })
                .unwrap();
        }

        // Insert 500 edges
        for i in 0..500 {
            backend
                .insert_edge(EdgeSpec {
                    from: (i % 100) + 1,
                    to: ((i + 1) % 100) + 1,
                    edge_type: "edge".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        // WAL should have records now
        let wal_size_before_flush = file_size(&wal_path);
        println!(
            "[TEST B] WAL size before flush: {} bytes",
            wal_size_before_flush
        );

        backend.flush_to_disk().expect("Flush must succeed");
    }

    // Measure WAL size after
    let wal_size_after = file_size(&wal_path);
    println!("[TEST B] WAL size after flush: {} bytes", wal_size_after);

    // Reopen and verify all data
    let backend = V3Backend::open(&db_path).unwrap();
    let all_ids = backend.entity_ids().unwrap();

    assert_eq!(all_ids.len(), 500, "All 500 nodes should persist");

    // Verify neighbors work
    let neighbors = backend
        .neighbors(
            SnapshotId::current(),
            1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();

    assert!(!neighbors.is_empty(), "Should have neighbors");
    println!(
        "[TEST B] Recovered {} nodes, {} neighbors from node 1",
        all_ids.len(),
        neighbors.len()
    );
}

// ============================================================================
// C. RECOVERY BEHAVIOR
// ============================================================================

#[test]
fn test_c_recovery_without_flush() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_c_no_flush.graph");

    let expected_count = 50;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        for i in 0..expected_count {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }

        // NO FLUSH - drop backend without flush
    }

    // Reopen and observe what WAL recovery can do
    let backend = V3Backend::open(&db_path).unwrap();
    let recovered_ids = backend.entity_ids().unwrap();

    println!(
        "[TEST C] Without flush - WAL recovered {} out of {} nodes",
        recovered_ids.len(),
        expected_count
    );

    // Document actual durability boundary
    if recovered_ids.len() == expected_count {
        println!("[TEST C] WAL replay preserved all data (good recovery)");
    } else if recovered_ids.is_empty() {
        println!("[TEST C] No data recovered (no WAL flush = no durability)");
    } else {
        println!(
            "[TEST C] Partial recovery: {}/{} nodes",
            recovered_ids.len(),
            expected_count
        );
    }
}

// ============================================================================
// D. READ/TRAVERSAL SANITY
// ============================================================================

#[test]
fn test_d_point_lookup_after_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_d_reads.graph");

    let target_id;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .unwrap();
        }

        // Node IDs start at 1, so node named "node_49" has ID=50 (i=49 is 50th insert, ID=50)
        target_id = 50;
        backend.flush_to_disk().unwrap();
    }

    // Reopen and test point lookup
    let backend = V3Backend::open(&db_path).unwrap();

    let start = Instant::now();
    let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(node.name, "node_49");
    assert_eq!(node.data["idx"], 49);

    println!("[TEST D] Point lookup after reopen: {:?}", elapsed);
}

#[test]
fn test_d_neighbor_fetch_after_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_d_neighbors.graph");

    let center_id;
    {
        let backend = V3Backend::create(&db_path).unwrap();

        center_id = backend
            .insert_node(NodeSpec {
                kind: "Center".to_string(),
                name: "center".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        // Create star graph: center -> 10 neighbors
        for i in 1..=10 {
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
                    from: center_id,
                    to: leaf,
                    edge_type: "link".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        backend.flush_to_disk().unwrap();
    }

    // Reopen and test neighbor fetch
    let backend = V3Backend::open(&db_path).unwrap();

    let start = Instant::now();
    let neighbors = backend
        .neighbors(
            SnapshotId::current(),
            center_id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(neighbors.len(), 10);
    println!(
        "[TEST D] Neighbor fetch (10 nodes) after reopen: {:?}",
        elapsed
    );
}

#[test]
fn test_d_traversal_after_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_d_traversal.graph");

    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Create chain: 1 -> 2 -> 3 -> ... -> 20
        let mut prev_id: Option<i64> = None;
        for i in 1..=20 {
            let id = backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"step": i}),
                })
                .unwrap();

            if let Some(pid) = prev_id {
                backend
                    .insert_edge(EdgeSpec {
                        from: pid,
                        to: id,
                        edge_type: "next".to_string(),
                        data: serde_json::json!({}),
                    })
                    .unwrap();
            }
            prev_id = Some(id);
        }

        backend.flush_to_disk().unwrap();
    }

    // Reopen and test BFS traversal
    let backend = V3Backend::open(&db_path).unwrap();

    let start = Instant::now();
    let bfs_result = backend.bfs(SnapshotId::current(), 1, 100).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(bfs_result.len(), 20);
    println!("[TEST D] BFS (20 nodes) after reopen: {:?}", elapsed);
}

// ============================================================================
// E. THROUGHPUT MEASUREMENT
// ============================================================================

#[test]
fn test_e_insert_throughput() {
    let temp = tempfile::TempDir::new().unwrap();

    let counts = [100, 500, 1000];

    for count in counts {
        let db_path = temp.path().join(format!("perf_{}.graph", count));
        let backend = V3Backend::create(&db_path).unwrap();

        let start = Instant::now();
        for i in 0..count {
            backend
                .insert_node(NodeSpec {
                    kind: "Perf".to_string(),
                    name: format!("p{}", i),
                    file_path: None,
                    data: serde_json::json!({"x": i}),
                })
                .unwrap();
        }
        let elapsed = start.elapsed();

        let rate = count as f64 / elapsed.as_millis() as f64;
        println!(
            "[TEST E] Insert {} nodes: {:?} ({:.2} nodes/ms)",
            count, elapsed, rate
        );
    }
}

#[test]
fn test_e_reopen_time() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_e_reopen.graph");

    // Setup: Create 1000 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..1000 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: format!("n{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    // Measure reopen time
    let mut total_time = std::time::Duration::ZERO;
    let iterations = 5;

    for _ in 0..iterations {
        let start = Instant::now();
        let _backend = V3Backend::open(&db_path).unwrap();
        let elapsed = start.elapsed();
        total_time += elapsed;
    }

    let avg_time = total_time / iterations as u32;
    println!(
        "[TEST E] Average reopen time (1000 nodes): {:?} ({} iterations)",
        avg_time, iterations
    );
}

#[test]
fn test_e_wal_size_tracking() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("test_e_wal.graph");
    let wal_path = temp.path().join("test_e_wal.v3wal");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert 100 nodes and track WAL growth
    for batch in 0..10 {
        for i in 0..10 {
            backend
                .insert_node(NodeSpec {
                    kind: "WalTest".to_string(),
                    name: format!("batch{}_node{}", batch, i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }

        let wal_size = file_size(&wal_path);
        println!(
            "[TEST E] After {} nodes, WAL size: {} bytes",
            (batch + 1) * 10,
            wal_size
        );
    }

    // Flush and check WAL truncation
    let wal_size_before_flush = file_size(&wal_path);
    println!(
        "[TEST E] WAL size before flush_to_disk(): {} bytes",
        wal_size_before_flush
    );

    backend.flush_to_disk().unwrap();

    let wal_size_after_flush = file_size(&wal_path);
    println!(
        "[TEST E] WAL size after flush_to_disk(): {} bytes",
        wal_size_after_flush
    );

    // WAL should be truncated after checkpoint
    if wal_size_after_flush < wal_size_before_flush {
        println!("[TEST E] WAL was truncated after checkpoint (expected)");
    }
}
