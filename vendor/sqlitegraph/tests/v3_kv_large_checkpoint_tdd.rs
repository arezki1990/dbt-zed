//! TDD: Large KV values (JSON maps) (> 65535 bytes) in V3 checkpoint
use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::kv_store::types::KvValue;
use sqlitegraph::snapshot::SnapshotId;
use tempfile::TempDir;

#[test]
fn test_large_kv_value_survives_checkpoint() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let key = b"large_json".to_vec();

    // Generate a large string value (> 65535 bytes)
    // 70,000 'A' characters
    let large_str = "A".repeat(70000);
    let large_value = KvValue::String(large_str.clone());

    // Phase 1: Set large value and flush to write checkpoint
    {
        let backend = V3Backend::create(&db_path).unwrap();
        backend.kv_set_v3(key.clone(), large_value, None);
        backend.flush().unwrap();
    }

    // Phase 2: Reopen database and verify the large value survives intact
    {
        let backend = V3Backend::open(&db_path).unwrap();
        let value = backend
            .kv_get_v3(SnapshotId::current(), &key)
            .expect("Failed to get large key after reopen");
        if let KvValue::String(s) = value {
            assert_eq!(s.len(), 70000);
            assert_eq!(s, large_str);
        } else {
            panic!("Expected KvValue::String");
        }
    }
}
