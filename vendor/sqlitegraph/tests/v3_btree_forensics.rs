//! V3 B+Tree Forensic Investigation Test Suite
//!
//! Focused investigation of the B+Tree/node path to identify the dominant bottleneck.
//!
//! Run with: cargo test --features native-v3,v3-forensics --test v3_btree_forensics -- --nocapture
//!
//! The helper functions below are only reachable when the `v3-forensics` feature
//! is enabled; clippy reports them as dead in the default build.

#![allow(
    dead_code,
    reason = "forensic helpers used only when v3-forensics feature is enabled"
)]

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;

fn print_scenario_header(name: &str) {
    println!("\n{}", "=".repeat(70));
    println!("  SCENARIO: {}", name);
    println!("{}", "=".repeat(70));
}

fn reset_counters() {
    #[cfg(feature = "v3-forensics")]
    FORENSIC_COUNTERS.reset();
}

fn print_counters() {
    #[cfg(feature = "v3-forensics")]
    FORENSIC_COUNTERS.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_1_insert_1_node_into_empty_db() {
    print_scenario_header("Insert 1 node into EMPTY DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario1.graph");

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::create(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node1".to_string(),
                file_path: None,
                data: serde_json::json!({"x": 1}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_2_insert_1_node_after_100_nodes() {
    print_scenario_header("Insert 1 node AFTER 100 existing nodes");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario2.graph");

    // Pre-populate with 100 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node_101".to_string(),
                file_path: None,
                data: serde_json::json!({"i": 101}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_3_insert_1_node_after_1k_nodes() {
    print_scenario_header("Insert 1 node AFTER 1K existing nodes");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario3.graph");

    // Pre-populate with 1000 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..1000 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node_1001".to_string(),
                file_path: None,
                data: serde_json::json!({"i": 1000}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_4_get_node_in_100_node_db() {
    print_scenario_header("get_node in 100-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario4.graph");
    let target_id;

    // Pre-populate with 100 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        target_id = 50; // Middle node
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
        println!("Retrieved node: {}", node.name);
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_5_get_node_in_1k_node_db() {
    print_scenario_header("get_node in 1K-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario5.graph");
    let target_id;

    // Pre-populate with 1000 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..1000 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        target_id = 500; // Middle node
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
        println!("Retrieved node: {}", node.name);
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_6_burst_insert_100_nodes() {
    print_scenario_header("Burst insert 100 nodes (measure per-node overhead)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario6.graph");

    // Insert first 50 to build up tree
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..50 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    // Now measure next 50 inserts
    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        for i in 50..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed for 50 inserts: {:?}", elapsed);
    println!("Per-insert time: {:?}", elapsed / 50);
    delta.print_report();
}

/// Measure cache warm-up behavior
#[test]
#[cfg(feature = "v3-forensics")]
fn scenario_7_repeated_get_same_node() {
    print_scenario_header("Repeated get_node for SAME node (cache warm-up)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("scenario7.graph");

    // Pre-populate with 100 nodes
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = FORENSIC_COUNTERS.snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        // Get the same node 100 times
        for _ in 0..100 {
            let _node = backend.get_node(SnapshotId::current(), 50).unwrap();
        }
    }
    let elapsed = start.elapsed();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    println!("Time elapsed for 100 gets: {:?}", elapsed);
    println!("Per-get time: {:?}", elapsed / 100);
    delta.print_report();
}
