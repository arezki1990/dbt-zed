use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use std::collections::HashSet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    println!("Creating test database with 1K nodes...");
    let backend = V3Backend::create(&db_path)?;

    // Create 1K nodes with predictable IDs
    let mut expected_ids = HashSet::new();
    for i in 0..1000 {
        let node_id = backend.insert_node(sqlitegraph::backend::NodeSpec {
            kind: "TestKind".to_string(),
            name: format!("node_{:05}", i),
            file_path: None,
            data: serde_json::json!({"value": i}),
        })?;
        expected_ids.insert(node_id);
    }
    backend.flush_to_disk()?;
    drop(backend);

    println!("Reopening and testing lazy decode correctness...");
    let backend = V3Backend::open(&db_path)?;
    let snapshot_id = sqlitegraph::snapshot::SnapshotId::current();

    // Verify all nodes can be retrieved
    let mut found_ids = HashSet::new();
    for &expected_id in &expected_ids {
        match backend.get_node(snapshot_id, expected_id) {
            Ok(node) => {
                found_ids.insert(node.id);
                if node.id % 100 == 0 {
                    println!(
                        "Found node {}: kind={}, name={}",
                        node.id, node.kind, node.name
                    );
                }
            }
            Err(e) => {
                eprintln!("ERROR: Failed to get node {}: {}", expected_id, e);
                return Err(e.into());
            }
        }
    }

    // Verify all expected nodes were found
    let missing: Vec<_> = expected_ids.difference(&found_ids).collect();
    if !missing.is_empty() {
        eprintln!("ERROR: {} nodes not found: {:?}", missing.len(), missing);
        return Err("Some nodes not found".into());
    }

    println!(
        "SUCCESS: All {} nodes found via lazy decode!",
        found_ids.len()
    );
    Ok(())
}
