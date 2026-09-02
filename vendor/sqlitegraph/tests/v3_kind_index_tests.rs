//! Tests for V3 kind index functionality
//!
//! Run with: cargo test --features native-v3 test_kind_index --release -- --nocapture

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_kind_index_insert_and_query() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("kind_index_test.graph");

    // Create database and insert nodes with various kinds
    let backend = V3Backend::create(&db_path).unwrap();

    let id1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let id2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let id3 = backend
        .insert_node(NodeSpec {
            kind: "Class".to_string(),
            name: "class_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend.flush_to_disk().unwrap();

    // Query by kind - should use index (O(1))
    use sqlitegraph::SnapshotId;
    let function_ids = backend
        .query_nodes_by_kind(SnapshotId::current(), "Function")
        .unwrap();
    let class_ids = backend
        .query_nodes_by_kind(SnapshotId::current(), "Class")
        .unwrap();
    let empty_ids = backend
        .query_nodes_by_kind(SnapshotId::current(), "NonExistent")
        .unwrap();

    // Verify results
    assert_eq!(function_ids.len(), 2);
    assert!(function_ids.contains(&id1));
    assert!(function_ids.contains(&id2));

    assert_eq!(class_ids.len(), 1);
    assert!(class_ids.contains(&id3));

    assert_eq!(empty_ids.len(), 0);

    println!("✓ Kind index works correctly for insert and query");
}

#[test]
fn test_kind_index_survives_reopen() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("kind_index_reopen.graph");

    // Create and populate database
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: if i % 2 == 0 { "Even" } else { "Odd" }.to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({ "i": i }),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    // Reopen and verify index was rebuilt
    let backend = V3Backend::open(&db_path).unwrap();
    use sqlitegraph::SnapshotId;

    let even_ids = backend
        .query_nodes_by_kind(SnapshotId::current(), "Even")
        .unwrap();
    let odd_ids = backend
        .query_nodes_by_kind(SnapshotId::current(), "Odd")
        .unwrap();

    assert_eq!(even_ids.len(), 50);
    assert_eq!(odd_ids.len(), 50);

    // Verify IDs are correct
    // Note: Node IDs start at 1, so i=0 gets node_id=1, i=1 gets node_id=2, etc.
    // This means: kind="Even" (from even i values) → odd node IDs
    //              kind="Odd" (from odd i values) → even node IDs
    for &id in &even_ids {
        assert!(
            id % 2 == 1,
            "Even kind should only contain odd IDs (i=0,2,4... → node_id=1,3,5...)"
        );
    }
    for &id in &odd_ids {
        assert!(
            id % 2 == 0,
            "Odd kind should only contain even IDs (i=1,3,5... → node_id=2,4,6...)"
        );
    }

    println!("✓ Kind index correctly rebuilt after reopen");
}

#[test]
fn test_kind_index_with_multiple_kinds() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("multi_kind_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert nodes with many different kinds
    let kinds = vec!["A", "B", "C", "D", "E"];
    let mut expected_counts = std::collections::HashMap::new();

    for (i, kind) in kinds.iter().cycle().take(100).enumerate() {
        backend
            .insert_node(NodeSpec {
                kind: kind.to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        *expected_counts.entry(kind.to_string()).or_insert(0) += 1;
    }

    backend.flush_to_disk().unwrap();

    // Verify each kind returns correct count
    use sqlitegraph::SnapshotId;
    for kind in &kinds {
        let ids = backend
            .query_nodes_by_kind(SnapshotId::current(), kind)
            .unwrap();
        let expected = expected_counts.get(*kind).copied().unwrap_or(0);
        assert_eq!(
            ids.len(),
            expected,
            "Kind {} should have {} nodes",
            kind,
            expected
        );
    }

    println!("✓ Kind index handles multiple kinds correctly");
}
