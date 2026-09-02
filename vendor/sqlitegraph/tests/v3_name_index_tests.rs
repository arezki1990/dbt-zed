//! Tests for V3 name index functionality
//!
//! Run with: cargo test --features native-v3 test_name_index --release -- --nocapture

use sqlitegraph::{
    backend::native::v3::V3Backend,
    backend::{GraphBackend, NodeSpec},
};
use tempfile::TempDir;

#[test]
fn test_name_index_exact_match() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("name_index_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert nodes with various names
    let id1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "my_func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let id2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "my_func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let _id3 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "other_func".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend.flush_to_disk().unwrap();

    use sqlitegraph::SnapshotId;

    // Exact match - O(1) lookup
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "my_func_a")
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], id1);

    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "my_func_b")
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], id2);

    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "nonexistent")
        .unwrap();
    assert_eq!(result.len(), 0);

    println!("✓ Name index exact match works correctly");
}

#[test]
fn test_name_index_prefix_match() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("prefix_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Insert nodes with prefix pattern
    let id1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "target_func_1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let id2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "target_func_2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let id3 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "other_func".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend.flush_to_disk().unwrap();

    use sqlitegraph::SnapshotId;

    // Prefix match with "*"
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "target_func*")
        .unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains(&id1));
    assert!(result.contains(&id2));
    assert!(!result.contains(&id3));

    println!("✓ Name index prefix match works correctly");
}

#[test]
fn test_name_index_survives_reopen() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("name_reopen.graph");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();
        for i in 0..100 {
            backend
                .insert_node(NodeSpec {
                    kind: "Node".to_string(),
                    name: if i % 10 == 0 {
                        format!("prefix_node_{}", i)
                    } else {
                        format!("node_{}", i)
                    },
                    file_path: None,
                    data: serde_json::json!({"i": i}),
                })
                .unwrap();
        }
        backend.flush_to_disk().unwrap();
    }

    // Reopen and verify
    let backend = V3Backend::open(&db_path).unwrap();
    use sqlitegraph::SnapshotId;

    // Exact match should work
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "prefix_node_0")
        .unwrap();
    assert_eq!(result.len(), 1);

    // Prefix match should work
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "prefix_node*")
        .unwrap();
    assert_eq!(result.len(), 10); // 0, 10, 20, ..., 90

    // Non-matching prefix should return empty
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "nonexistent*")
        .unwrap();
    assert_eq!(result.len(), 0);

    println!("✓ Name index correctly rebuilt after reopen");
}

#[test]
fn test_name_index_special_patterns() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("special_pattern_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "some_func".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    use sqlitegraph::SnapshotId;

    // Suffix wildcard → substring search, finds some_func
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "*func")
        .unwrap();
    assert_eq!(result.len(), 1, "suffix *func should match some_func");

    // Middle wildcard → substring search, finds some_func
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "some_*func")
        .unwrap();
    assert_eq!(result.len(), 1, "middle some_*func should match some_func");

    // Single char wildcard (?) — GLOB parity: matches exactly one char.
    // "some_func?" matches "some_func1" but not "some_func" (no trailing char).
    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "some_func1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "some_func?")
        .unwrap();
    assert_eq!(
        result.len(),
        1,
        "? is a single-char GLOB wildcard: 'some_func?' should match 'some_func1'"
    );

    // Character class ([abc]) — not supported by V3, treated as literal.
    let result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "some_func[abc]")
        .unwrap();
    assert_eq!(result.len(), 0, "[abc] is treated as literal, no match");

    println!("✓ Special patterns handled correctly");
}

#[test]
fn test_name_index_duplicate_names() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("duplicate_test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Multiple nodes with same name (e.g., overloaded functions)
    let id1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "overloaded".to_string(),
            file_path: Some("/path/a.rs".to_string()),
            data: serde_json::json!({"sig": "(i32) -> i32"}),
        })
        .unwrap();

    let id2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "overloaded".to_string(),
            file_path: Some("/path/b.rs".to_string()),
            data: serde_json::json!({"sig": "(f64) -> f64"}),
        })
        .unwrap();

    let id3 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "overloaded".to_string(),
            file_path: Some("/path/c.rs".to_string()),
            data: serde_json::json!({"sig": "(String) -> String"}),
        })
        .unwrap();

    use sqlitegraph::SnapshotId;

    // Exact match should return all IDs with that name
    let mut result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "overloaded")
        .unwrap();
    result.sort();
    assert_eq!(result.len(), 3);
    assert!(result.contains(&id1));
    assert!(result.contains(&id2));
    assert!(result.contains(&id3));

    // Prefix match should also return all
    let mut result = backend
        .query_nodes_by_name_pattern(SnapshotId::current(), "overloaded*")
        .unwrap();
    result.sort();
    assert_eq!(result.len(), 3);

    println!("✓ Name index handles duplicate names correctly");
}
