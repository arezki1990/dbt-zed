//! V3 Insert/Read Path Forensic Investigation Test Suite
//!
//! Deep dive into node insert, get_node, and neighbors performance.
//!
//! Run with: cargo test --features native-v3,v3-forensics --test v3_insert_read_forensics -- --nocapture

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::{
    FORENSIC_COUNTERS, ForensicDelta, ForensicSnapshot,
};

#[cfg(feature = "v3-forensics")]
fn print_header(name: &str) {
    println!("\n{}", "=".repeat(70));
    println!("  {}", name);
    println!("{}", "=".repeat(70));
}

#[cfg(feature = "v3-forensics")]
fn reset_counters() {
    #[cfg(feature = "v3-forensics")]
    FORENSIC_COUNTERS.reset();
}

#[cfg(feature = "v3-forensics")]
fn get_snapshot() -> ForensicSnapshot {
    FORENSIC_COUNTERS.snapshot()
}

#[cfg(feature = "v3-forensics")]
fn print_delta(delta: &ForensicDelta, elapsed: std::time::Duration) {
    println!("Time elapsed: {:?}", elapsed);
    delta.print_report();
}

//=============================================================================
// INSERT PATH FORENSICS
//=============================================================================

#[test]
#[cfg(feature = "v3-forensics")]
fn insert_1_into_empty_db() {
    print_header("INSERT: 1 node into EMPTY DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("insert_empty.graph");

    reset_counters();
    let before = get_snapshot();

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

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn insert_1_into_100_node_db() {
    print_header("INSERT: 1 node into 100-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("insert_100.graph");

    // Pre-populate
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node_100".to_string(),
                file_path: None,
                data: serde_json::json!({"i": 100}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn insert_1_into_1k_node_db() {
    print_header("INSERT: 1 node into 1K-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("insert_1k.graph");

    // Pre-populate
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node_1000".to_string(),
                file_path: None,
                data: serde_json::json!({"i": 1000}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn insert_1_into_10k_node_db() {
    print_header("INSERT: 1 node into 10K-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("insert_10k.graph");

    // Pre-populate
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..10000 {
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Test".to_string(),
                name: "node_10000".to_string(),
                file_path: None,
                data: serde_json::json!({"i": 10000}),
            })
            .unwrap();
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

//=============================================================================
// GET_NODE PATH FORENSICS
//=============================================================================

#[test]
#[cfg(feature = "v3-forensics")]
fn cold_get_node_100_node_db() {
    print_header("GET_NODE: Cold lookup in 100-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("get_100.graph");
    let target_id = 50;

    // Pre-populate
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
        println!("Retrieved: {} ({})", node.name, node.id);
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn cold_get_node_1k_node_db() {
    print_header("GET_NODE: Cold lookup in 1K-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("get_1k.graph");
    let target_id = 500;

    // Pre-populate
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
        println!("Retrieved: {} ({})", node.name, node.id);
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn cold_get_node_10k_node_db() {
    print_header("GET_NODE: Cold lookup in 10K-node DB");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("get_10k.graph");
    let target_id = 5000;

    // Pre-populate
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..10000 {
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
        println!("Retrieved: {} ({})", node.name, node.id);
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn warm_get_node_100x() {
    print_header("GET_NODE: 100x repeated lookup (cache warm-up)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("get_warm.graph");
    let target_id = 500;

    // Pre-populate with 1K nodes
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
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        // Get the same node 100 times
        for _ in 0..100 {
            let node = backend.get_node(SnapshotId::current(), target_id).unwrap();
            if target_id == 500 {
                println!("First get: {}", node.name);
            }
        }
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
    println!("Per-get time: {:?}", elapsed / 100);
}

//=============================================================================
// NEIGHBORS PATH FORENSICS
//=============================================================================

#[test]
#[cfg(feature = "v3-forensics")]
fn cold_neighbors_small_db() {
    print_header("NEIGHBORS: Cold query in small DB (100 nodes, 10 edges)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("neighbors_small.graph");

    // Pre-populate with nodes and edges
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
        // Create edges: 0->1, 0->2, ..., 0->9
        for i in 1..=10 {
            backend
                .insert_edge(sqlitegraph::EdgeSpec {
                    from: 0,
                    to: i,
                    edge_type: String::new(),
                    data: serde_json::json!(null),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                0,
                sqlitegraph::backend::NeighborQuery::default(),
            )
            .unwrap();
        println!("Neighbors of node 0: {:?}", neighbors);
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn cold_neighbors_medium_db() {
    print_header("NEIGHBORS: Cold query in medium DB (1K nodes, 10 edges)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("neighbors_medium.graph");

    // Pre-populate with nodes and edges
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
        // Create edges from node 500
        for i in 501..=510 {
            backend
                .insert_edge(sqlitegraph::EdgeSpec {
                    from: 500,
                    to: i,
                    edge_type: String::new(),
                    data: serde_json::json!(null),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                500,
                sqlitegraph::backend::NeighborQuery::default(),
            )
            .unwrap();
        println!("Neighbors of node 500: {:?}", neighbors);
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
}

#[test]
#[cfg(feature = "v3-forensics")]
fn warm_neighbors_100x() {
    print_header("NEIGHBORS: 100x repeated query (cache warm-up)");

    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("neighbors_warm.graph");

    // Pre-populate
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
        for i in 501..=510 {
            backend
                .insert_edge(sqlitegraph::EdgeSpec {
                    from: 500,
                    to: i,
                    edge_type: String::new(),
                    data: serde_json::json!(null),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    reset_counters();
    let before = get_snapshot();

    let start = Instant::now();
    {
        let backend = V3Backend::open(&db_path).unwrap();
        // Query neighbors 100 times
        for _ in 0..100 {
            let neighbors = backend
                .neighbors(
                    SnapshotId::current(),
                    500,
                    sqlitegraph::backend::NeighborQuery::default(),
                )
                .unwrap();
            if neighbors.len() > 0 && neighbors[0] == 501 {
                println!("First query returned {} neighbors", neighbors.len());
            }
        }
    }
    let elapsed = start.elapsed();

    let after = get_snapshot();
    let delta = before.diff(&after);
    print_delta(&delta, elapsed);
    println!("Per-query time: {:?}", elapsed / 100);
}
