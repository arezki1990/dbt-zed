//! Roundtrip test for V3 open optimization
//!
//! Verifies that:
//! 1. Data can be inserted and persisted
//! 2. Indexes are persisted to sidecar
//! 3. Open correctly restores indexes
//! 4. Data can be queried after reopen

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== V3 OPEN ROUNDTRIP TEST ===\n");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    // STEP 1: Create database with nodes
    println!("STEP 1: Creating database with 10,000 nodes...");
    let backend = V3Backend::create(&db_path)?;
    for i in 0..10_000 {
        backend.insert_node(NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i}),
        })?;
    }
    backend.flush_to_disk()?;
    drop(backend);
    println!("  ✓ Created and flushed");

    // STEP 2: Verify index file was created
    let index_path = db_path.with_extension("v3index");
    assert!(index_path.exists(), "Index file should exist after flush");
    let index_size = std::fs::metadata(&index_path)?.len();
    println!(
        "  ✓ Index file created: {:.2} KB",
        index_size as f64 / 1024.0
    );

    // STEP 3: Reopen database and verify indexes are restored
    println!("\nSTEP 2: Reopening database...");
    let backend = V3Backend::open(&db_path)?;
    println!("  ✓ Database opened");

    // STEP 4: Query nodes by name to verify index is working
    println!("\nSTEP 3: Verifying index functionality...");

    // V3 backend uses snapshot_id 0 to represent "current" (all committed data)
    use sqlitegraph::SnapshotId;
    let snapshot_id = SnapshotId(0);

    // Query for a specific node by exact name pattern
    let results = backend.query_nodes_by_name_pattern(snapshot_id, "node_04200")?;
    assert_eq!(results.len(), 1, "Should find exactly one node");
    println!("  ✓ Name index working: found node_04200");

    // STEP 5: Verify a few more lookups
    for i in [0, 100, 1000, 5000, 9999] {
        let name = format!("node_{:05}", i);
        let results = backend.query_nodes_by_name_pattern(snapshot_id, &name)?;
        assert_eq!(
            results.len(),
            1,
            "Should find exactly one node for {}",
            name
        );
    }
    println!("  ✓ Multiple name lookups working correctly");

    println!("\n=== ALL TESTS PASSED ===");
    Ok(())
}
