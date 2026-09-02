//! Forensic instrumentation test for V3 backend
//!
//! This test isolates single operations and measures the internal work
//! performed by the V3 backend to identify amplification issues.
//!
//! Run with:
//! cargo test --features native-v3,v3-forensics v3_forensics -- --nocapture

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;

// ============================================================================
// WRITE PATH SCENARIOS
// ============================================================================

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore] // Run explicitly with cargo test --ignored
fn forensic_insert_node_into_empty_db() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("          SCENARIO: Insert 1 node into EMPTY DB            ");
    println!("═══════════════════════════════════════════════════════════\n");

    FORENSIC_COUNTERS.reset();

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    let before = FORENSIC_COUNTERS.snapshot();

    // Single insert operation
    let node = NodeSpec {
        kind: "TestNode".to_string(),
        name: "test".to_string(),
        file_path: None,
        data: serde_json::json!({"test": "data"}),
    };
    let _node_id = backend.insert_node(node).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    // Assert we have reasonable counts
    assert_eq!(delta.logical_insert_node_calls, 1);
    println!("Expected: 1 logical insert_node call\n");

    // Check for pathological sync patterns
    if delta.sync_data_count > 5 {
        println!(
            "⚠️  WARNING: Excessive sync_data() calls: {}",
            delta.sync_data_count
        );
    }
    if delta.sync_all_count > 5 {
        println!(
            "⚠️  WARNING: Excessive sync_all() calls: {}",
            delta.sync_all_count
        );
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_insert_node_after_100_nodes() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("       SCENARIO: Insert 1 node after 100 existing nodes     ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Pre-populate with 100 nodes
    for i in 0..100 {
        let node = NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        };
        backend.insert_node(node).unwrap();
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single insert after 100 nodes
    let node = NodeSpec {
        kind: "TestNode".to_string(),
        name: "node_after_100".to_string(),
        file_path: None,
        data: serde_json::json!({"test": "data"}),
    };
    let _node_id = backend.insert_node(node).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_insert_node_calls, 1);

    // Check for amplification as DB grows
    if delta.page_write_count > 10 {
        println!(
            "⚠️  WARNING: High page write count for single insert: {}",
            delta.page_write_count
        );
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_insert_node_after_10k_nodes() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("      SCENARIO: Insert 1 node after 10K existing nodes       ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Pre-populate with 10K nodes
    for i in 0..10_000 {
        let node = NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        };
        backend.insert_node(node).unwrap();
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single insert after 10K nodes
    let node = NodeSpec {
        kind: "TestNode".to_string(),
        name: "node_after_10k".to_string(),
        file_path: None,
        data: serde_json::json!({"test": "data"}),
    };
    let _node_id = backend.insert_node(node).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_insert_node_calls, 1);

    // Critical check for amplification in larger DB
    if delta.btree_insert_calls > 1 {
        println!(
            "⚠️  WARNING: Multiple B+Tree inserts for single node insert: {}",
            delta.btree_insert_calls
        );
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_insert_edge() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("            SCENARIO: Insert 1 edge                          ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Create two nodes first
    let n1 = backend
        .insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let n2 = backend
        .insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: "node2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single edge insert
    let edge = EdgeSpec {
        from: n1,
        to: n2,
        edge_type: "TestEdge".to_string(),
        data: serde_json::json!({}),
    };
    let _edge_id = backend.insert_edge(edge).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_insert_edge_calls, 1);
}

// ============================================================================
// READ PATH SCENARIOS
// ============================================================================

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_get_node_small_db() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("      SCENARIO: get_node in small DB (1K nodes)            ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Pre-populate with 1K nodes
    let mut target_id = 0;
    for i in 0..1_000 {
        let node_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
        if i == 500 {
            target_id = node_id;
        }
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single get_node operation
    let _node = backend.get_node(SnapshotId::current(), target_id).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_get_node_calls, 1);

    // Check for read amplification
    if delta.page_read_count > 5 {
        println!(
            "⚠️  WARNING: High page read count for single get_node: {}",
            delta.page_read_count
        );
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_get_node_medium_db() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("      SCENARIO: get_node in medium DB (10K nodes)           ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Pre-populate with 10K nodes
    let mut target_id = 0;
    for i in 0..10_000 {
        let node_id = backend
            .insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
        if i == 5_000 {
            target_id = node_id;
        }
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single get_node operation
    let _node = backend.get_node(SnapshotId::current(), target_id).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_get_node_calls, 1);

    // B+Tree height should be small even for 10K nodes
    if delta.page_read_count > 10 {
        println!(
            "⚠️  WARNING: Excessive page reads for B+Tree lookup: {}",
            delta.page_read_count
        );
        println!("  Expected: ~log2(10K) ≈ 14 pages max for node page + B+Tree pages");
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_neighbors_small_db() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("      SCENARIO: neighbors() in small DB (1K nodes)          ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Create a star graph: node 0 connects to 100 other nodes
    let center_id = backend
        .insert_node(NodeSpec {
            kind: "Center".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    for i in 1..=100 {
        let node_id = backend
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
                to: node_id,
                edge_type: "Connect".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single neighbors() call
    use sqlitegraph::backend::{BackendDirection, NeighborQuery};
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let neighbors = backend
        .neighbors(SnapshotId::current(), center_id, query)
        .unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_neighbors_calls, 1);
    assert_eq!(neighbors.len(), 100);
    println!("Retrieved {} neighbors\n", neighbors.len());

    // Check for read amplification
    if delta.page_read_count > 5 {
        println!(
            "⚠️  WARNING: High page read count for neighbors: {}",
            delta.page_read_count
        );
    }
}

#[test]
#[cfg(feature = "v3-forensics")]
#[ignore]
fn forensic_neighbors_medium_db() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("      SCENARIO: neighbors() in medium DB (10K nodes)         ");
    println!("═══════════════════════════════════════════════════════════\n");

    let temp = TempDir::new().unwrap();
    let backend = V3Backend::create(temp.path().join("v3.db")).unwrap();

    // Create a star graph: node 0 connects to 1000 other nodes
    let center_id = backend
        .insert_node(NodeSpec {
            kind: "Center".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    for i in 1..=1000 {
        let node_id = backend
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
                to: node_id,
                edge_type: "Connect".to_string(),
                data: serde_json::json!({}),
            })
            .unwrap();
    }

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    // Single neighbors() call
    use sqlitegraph::backend::{BackendDirection, NeighborQuery};
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let neighbors = backend
        .neighbors(SnapshotId::current(), center_id, query)
        .unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    assert_eq!(delta.logical_neighbors_calls, 1);
    assert_eq!(neighbors.len(), 1000);
    println!("Retrieved {} neighbors\n", neighbors.len());

    // Check for read amplification
    if delta.page_read_count > 5 {
        println!(
            "⚠️  WARNING: High page read count for neighbors: {}",
            delta.page_read_count
        );
    }
}
