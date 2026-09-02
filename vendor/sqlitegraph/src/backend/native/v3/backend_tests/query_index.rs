use super::*;
use tempfile::TempDir;

#[test]
fn test_query_nodes_by_kind() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Class".to_string(),
            name: "class_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    use crate::SnapshotId;
    let snapshot = SnapshotId::current();

    let functions = backend.query_nodes_by_kind(snapshot, "Function").unwrap();
    assert_eq!(functions.len(), 2);

    let classes = backend.query_nodes_by_kind(snapshot, "Class").unwrap();
    assert_eq!(classes.len(), 1);

    let unknown = backend.query_nodes_by_kind(snapshot, "Unknown").unwrap();
    assert!(unknown.is_empty());
}

#[test]
fn test_query_nodes_by_name_pattern_exact() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    use crate::SnapshotId;
    let snapshot = SnapshotId::current();

    let results = backend
        .query_nodes_by_name_pattern(snapshot, "func_a")
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn test_query_nodes_by_name_pattern_prefix() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Class".to_string(),
            name: "class_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    use crate::SnapshotId;
    let snapshot = SnapshotId::current();

    let results = backend
        .query_nodes_by_name_pattern(snapshot, "func*")
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_query_nodes_by_name_pattern_substring() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "my_func_a".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Function".to_string(),
            name: "my_func_b".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    use crate::SnapshotId;
    let snapshot = SnapshotId::current();

    let results = backend
        .query_nodes_by_name_pattern(snapshot, "*func*")
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_index_persistence_across_open() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    {
        let backend = V3Backend::create(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Function".to_string(),
                name: "func_a".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        backend.flush_to_disk().unwrap();
    }

    let index_path = crate::backend::native::v3::index_persistence::index_path_for_db(&db_path);
    assert!(
        index_path.exists(),
        "Index sidecar should be created on flush"
    );

    {
        let backend = V3Backend::open(&db_path).unwrap();
        use crate::SnapshotId;
        let snapshot = SnapshotId::current();

        let results = backend.query_nodes_by_kind(snapshot, "Function").unwrap();
        assert_eq!(
            results.len(),
            1,
            "Kind index should be restored from sidecar"
        );
    }
}

#[test]
fn test_index_rebuild_on_open_without_sidecar() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    {
        let backend = V3Backend::create(&db_path).unwrap();
        backend
            .insert_node(NodeSpec {
                kind: "Class".to_string(),
                name: "class_a".to_string(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        backend.flush_to_disk().unwrap();
    }

    let index_path = crate::backend::native::v3::index_persistence::index_path_for_db(&db_path);
    let _ = std::fs::remove_file(&index_path);

    {
        let backend = V3Backend::open(&db_path).unwrap();
        use crate::SnapshotId;
        let snapshot = SnapshotId::current();

        let results = backend.query_nodes_by_kind(snapshot, "Class").unwrap();
        assert_eq!(
            results.len(),
            1,
            "Kind index should be rebuilt from node scan"
        );
    }
}
