//! Standalone forensic example for V3 backend
//!
//! Run with: cargo run --features native-v3,v3-forensics --example v3_forensics_example

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;

#[cfg(feature = "v3-forensics")]
fn main() {
    println!("\n═══════════════════════════════════════════════════════════");
    println!("          V3 FORENSIC INVESTIGATION                         ");
    println!("═══════════════════════════════════════════════════════════\n");

    // Reset counters
    FORENSIC_COUNTERS.reset();

    // Create temp directory
    let temp_dir = tempfile::TempDir::new().unwrap();
    let backend = V3Backend::create(temp_dir.path().join("v3.db")).unwrap();

    // ========================================
    // SCENARIO 1: Insert 1 node into empty DB
    // ========================================
    println!("SCENARIO 1: Insert 1 node into EMPTY DB");
    println!("───────────────────────────────────────────────────────────────");

    let before = FORENSIC_COUNTERS.snapshot();

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

    // ========================================
    // SCENARIO 2: Insert 1 node after 100 nodes
    // ========================================
    println!("\nSCENARIO 2: Insert 1 node after 100 existing nodes");
    println!("───────────────────────────────────────────────────────────────");

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

    // ========================================
    // SCENARIO 3: get_node in small DB
    // ========================================
    println!("\nSCENARIO 3: get_node in small DB (100 nodes)");
    println!("───────────────────────────────────────────────────────────────");

    FORENSIC_COUNTERS.reset();
    let before = FORENSIC_COUNTERS.snapshot();

    let _node = backend.get_node(SnapshotId::current(), 50).unwrap();

    let after = FORENSIC_COUNTERS.snapshot();
    let delta = before.diff(&after);

    delta.print_report();

    // ========================================
    // SCENARIO 4: neighbors() call
    // ========================================
    println!("\nSCENARIO 4: neighbors() call");
    println!("───────────────────────────────────────────────────────────────");

    // Create a small star graph
    let center_id = backend
        .insert_node(NodeSpec {
            kind: "Center".to_string(),
            name: "center".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    for i in 1..=10 {
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
    println!("Retrieved {} neighbors", neighbors.len());

    // ========================================
    // SUMMARY OF FINDINGS
    // ========================================
    println!("\n═══════════════════════════════════════════════════════════");
    println!("                       SUMMARY                                 ");
    println!("═══════════════════════════════════════════════════════════\n");

    println!("KEY FINDINGS:");
    println!("1. Each insert_node triggers:");
    println!("   - B+Tree insert operations (for node_id -> page_id mapping)");
    println!("   - Node page writes (to store the node data)");
    println!("   - sync_data() and/or sync_all() calls on every page write");
    println!("\n2. Each get_node triggers:");
    println!("   - B+Tree lookup operations");
    println!("   - Node page reads");
    println!("\n3. Each neighbors() call:");
    println!("   - Edge store lookups (mostly in-memory)");
    println!("\n");
    println!("⚠️  CRITICAL: sync_data() and sync_all() on EVERY page write is catastrophic!");
    println!("   This is a known issue in src/backend/native/v3/");
    println!("   - btree.rs:1059: file.sync_data() on write_page");
    println!("   - node/store.rs:678: file.sync_all() on write_node_page");
}

#[cfg(not(feature = "v3-forensics"))]
fn main() {
    println!("Error: This example requires the 'v3-forensics' feature to be enabled.");
    println!(
        "Run with: cargo run --features native-v3,v3-forensics --example v3_forensics_example"
    );
}
