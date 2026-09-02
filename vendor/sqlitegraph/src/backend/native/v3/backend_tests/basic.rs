use super::*;
use crate::backend::native::v3::{V3_FORMAT_VERSION, V3_MAGIC};
use tempfile::TempDir;

#[test]
fn test_v3_backend_create() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path);
    assert!(backend.is_ok());
    assert!(db_path.exists());
}

#[test]
fn test_v3_backend_create_and_open() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    {
        let backend = V3Backend::create(&db_path).unwrap();
        assert!(!backend.is_wal_enabled());
    }

    {
        let backend = V3Backend::open(&db_path).unwrap();
        assert_eq!(backend.header().magic, V3_MAGIC);
        assert_eq!(backend.header().version, V3_FORMAT_VERSION);
    }
}

#[test]
fn test_v3_backend_rejects_sqlite_base_path() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("reserved.sqlite");

    let create_error = match V3Backend::create(&db_path) {
        Ok(_) => panic!("expected .sqlite base path rejection"),
        Err(err) => err,
    };
    assert!(
        create_error
            .to_string()
            .contains("Base path must not use .sqlite extension"),
        "unexpected error: {create_error}"
    );

    std::fs::write(&db_path, b"not-a-v3-db").unwrap();
    let open_error = match V3Backend::open(&db_path) {
        Ok(_) => panic!("expected .sqlite base path rejection"),
        Err(err) => err,
    };
    assert!(
        open_error
            .to_string()
            .contains("Base path must not use .sqlite extension"),
        "unexpected error: {open_error}"
    );
}

#[test]
fn test_v3_backend_insert_node() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    let node_id = backend
        .insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: "test_node".to_string(),
            file_path: None,
            data: serde_json::json!({"key": "value"}),
        })
        .unwrap();

    assert_eq!(node_id, 1);

    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn test_v3_backend_batch_insert_node_persists_node_properties() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("batch_node.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let mut batch = backend.begin_batch();
    let node_id = batch
        .insert_node(NodeSpec {
            kind: "Batch".to_string(),
            name: "batched_node".to_string(),
            file_path: None,
            data: serde_json::json!({"mode": "batch"}),
        })
        .unwrap();
    batch.commit().unwrap();

    let props = backend.get_node_properties(node_id).unwrap();
    let Some((kind, name, data)) = props else {
        panic!("batched node properties missing");
    };
    assert_eq!(kind, "Batch");
    assert_eq!(name, "batched_node");
    assert_eq!(data["mode"], serde_json::json!("batch"));
}

#[test]
fn test_v3_backend_insert_node_with_large_data() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_large.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    let large_data = serde_json::json!({
        "path": "src/components/user/authentication/handlers/login.rs",
        "hash": "abcdef1234567890abcdef1234567890abcdef1234567890",
        "last_indexed_at": 1234567890_i64,
        "last_modified": 1234567890_i64,
        "metadata": {
            "language": "rust",
            "lines": 150,
            "size_bytes": 4096
        }
    });

    let node_id = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "login.rs".to_string(),
            file_path: Some("src/components/user/authentication/handlers/login.rs".to_string()),
            data: large_data,
        })
        .unwrap();

    assert_eq!(node_id, 1);

    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), 1);

    use crate::SnapshotId;
    let snapshot = SnapshotId::current();
    let node = backend.get_node(snapshot, node_id).unwrap();
    assert_eq!(node.kind, "File");
    assert_eq!(node.name, "login.rs");
}

#[test]
fn test_v3_backend_open_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("nonexistent.graph");

    let result = V3Backend::open(&db_path);
    assert!(result.is_err());
}
