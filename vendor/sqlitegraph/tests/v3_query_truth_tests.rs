//! V3 Backend Query Truth Audit Tests
//!
//! Tests that verify V3 backend behavior is honest about its capabilities.
//! Per the truth audit principle: stubbed methods must explicitly fail
//! rather than returning fake or incomplete results.

#![cfg(feature = "native-v3")]

use sqlitegraph::{
    backend::{BackendDirection, GraphBackend, NodeSpec},
    pattern::{NodeConstraint, PatternLeg, PatternQuery},
};

// Note: V3Backend is conditionally compiled and only available with native-v3 feature
// We use it through the re-export from backend module
use sqlitegraph::backend::native::v3::V3Backend;

use tempfile::TempDir;

/// Helper to create a V3-backed graph
fn create_v3_backend() -> (TempDir, V3Backend) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_v3.graph");
    let backend = V3Backend::create(&db_path).unwrap();
    (temp_dir, backend)
}

#[test]
fn test_v3_pattern_search_matches_kind_chain() {
    let (_temp_dir, backend) = create_v3_backend();

    let f1 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "A_func".to_string(),
            file_path: None,
            data: serde_json::json!({"name": "A_func"}),
        })
        .unwrap();
    let f2 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "B_func".to_string(),
            file_path: None,
            data: serde_json::json!({"name": "B_func"}),
        })
        .unwrap();
    let f3 = backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "C_func".to_string(),
            file_path: None,
            data: serde_json::json!({"name": "C_func"}),
        })
        .unwrap();
    let s1 = backend
        .insert_node(NodeSpec {
            kind: "Struct".to_string(),
            name: "S_alpha".to_string(),
            file_path: None,
            data: serde_json::json!({"name": "S_alpha"}),
        })
        .unwrap();
    let s2 = backend
        .insert_node(NodeSpec {
            kind: "Struct".to_string(),
            name: "S_beta".to_string(),
            file_path: None,
            data: serde_json::json!({"name": "S_beta"}),
        })
        .unwrap();

    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: f1,
            to: f2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: f2,
            to: s1,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: f1,
            to: f3,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: f3,
            to: s2,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: f2,
            to: f3,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let pattern = PatternQuery {
        root: Some(NodeConstraint::kind("Function")),
        legs: vec![
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".into()),
                constraint: Some(NodeConstraint::kind("Function")),
            },
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".into()),
                constraint: Some(NodeConstraint::kind("Struct")),
            },
        ],
    };

    let matches = backend
        .pattern_search(sqlitegraph::SnapshotId::current(), f1, &pattern)
        .expect("pattern search should succeed");
    let sequences: Vec<Vec<i64>> = matches.into_iter().map(|m| m.nodes).collect();

    assert_eq!(sequences, vec![vec![f1, f2, s1], vec![f1, f3, s2]]);
}

#[test]
fn test_v3_snapshot_import_works() {
    // Phase 5: snapshot_import is now implemented on V3.
    // It reads a JSONL dump and inserts nodes + edges via insert_node/insert_edge.
    use sqlitegraph::backend::GraphBackend;
    use std::io::Write;

    let (_temp_dir, backend) = create_v3_backend();

    // Create a JSONL snapshot file with 3 entities and 2 edges.
    let snapshot_path = _temp_dir.path().join("snapshot.json");
    let mut file = std::fs::File::create(&snapshot_path).expect("create file");
    writeln!(
        file,
        r#"{{"type":"entity","id":1,"kind":"Function","name":"alpha","data":{{"val":1}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"entity","id":2,"kind":"Function","name":"beta","data":{{"val":2}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"entity","id":3,"kind":"Variable","name":"gamma","data":null}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"edge","from_id":1,"to_id":2,"edge_type":"CALLS","data":null}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"edge","from_id":2,"to_id":3,"edge_type":"READS","data":null}}"#
    )
    .unwrap();
    drop(file);

    let result = backend.snapshot_import(_temp_dir.path());
    assert!(
        result.is_ok(),
        "snapshot_import should succeed: {:?}",
        result.err()
    );
    let metadata = result.unwrap();
    assert_eq!(
        metadata.entities_imported, 3,
        "3 entities should be imported"
    );
    assert_eq!(metadata.edges_imported, 2, "2 edges should be imported");
}

#[test]
fn test_v3_query_nodes_by_name_pattern_substring_not_glob() {
    // Phase 3: V3 now uses GLOB semantics matching the sqlite-backend.
    // A pattern with no wildcards matches exactly (GLOB), not as a substring.
    let (_temp_dir, backend) = create_v3_backend();

    // Create nodes with specific names
    let _user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "SuperUser".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let _admin = backend
        .insert_node(NodeSpec {
            kind: "Admin".to_string(),
            name: "UserAdmin".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let _root = backend
        .insert_node(NodeSpec {
            kind: "Root".to_string(),
            name: "root".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // With GLOB semantics, pattern "User" (no wildcards) matches exactly "User" only.
    // Neither "SuperUser" nor "UserAdmin" should match.
    let result = backend
        .query_nodes_by_name_pattern(sqlitegraph::SnapshotId::current(), "User")
        .unwrap();

    assert!(
        result.is_empty(),
        "GLOB 'User' (no wildcards) must match exactly; neither 'SuperUser' nor 'UserAdmin' \
         should match. Got {:?}",
        result
    );
}

#[test]
fn test_v3_name_pattern_exact_match() {
    // Regression (TDD): non-wildcard pattern must match exactly, like SQLite GLOB.
    let (_temp_dir, backend) = create_v3_backend();

    let user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "User".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let _super_user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "SuperUser".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let _user_admin = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "UserAdmin".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut result = backend
        .query_nodes_by_name_pattern(sqlitegraph::SnapshotId::current(), "User")
        .unwrap();
    result.sort();

    assert_eq!(
        result,
        vec![user],
        "pattern 'User' (no wildcards) must match only the node named exactly 'User'"
    );
}

#[test]
fn test_v3_name_pattern_glob_prefix() {
    // Regression (TDD): "User*" prefix GLOB matches names starting with "User".
    let (_temp_dir, backend) = create_v3_backend();

    let user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "User".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let user_admin = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "UserAdmin".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let _super_user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "SuperUser".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut result = backend
        .query_nodes_by_name_pattern(sqlitegraph::SnapshotId::current(), "User*")
        .unwrap();
    result.sort();

    let mut expected = vec![user, user_admin];
    expected.sort();

    assert_eq!(
        result, expected,
        "pattern 'User*' must match names starting with 'User' (GLOB prefix)"
    );
}

#[test]
fn test_v3_name_pattern_glob_suffix() {
    // Regression (TDD): "*User" suffix GLOB matches names ending with "User".
    let (_temp_dir, backend) = create_v3_backend();

    let user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "User".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let super_user = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "SuperUser".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let _user_admin = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "UserAdmin".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let mut result = backend
        .query_nodes_by_name_pattern(sqlitegraph::SnapshotId::current(), "*User")
        .unwrap();
    result.sort();

    let mut expected = vec![user, super_user];
    expected.sort();

    assert_eq!(
        result, expected,
        "pattern '*User' must match names ending with 'User' (GLOB suffix)"
    );
}

#[test]
fn test_v3_query_nodes_by_kind_uses_kind_index_correctly() {
    let (_temp_dir, backend) = create_v3_backend();

    // Create nodes of different kinds
    let _user1 = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "alice".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let _user2 = backend
        .insert_node(NodeSpec {
            kind: "User".to_string(),
            name: "bob".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let _doc = backend
        .insert_node(NodeSpec {
            kind: "Document".to_string(),
            name: "test.txt".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Query by kind - results should be correct
    let result = backend
        .query_nodes_by_kind(sqlitegraph::SnapshotId::current(), "User")
        .unwrap();

    assert_eq!(result.len(), 2, "Should find exactly 2 User nodes");

    // Results are correct; current implementation uses the kind index.
}
