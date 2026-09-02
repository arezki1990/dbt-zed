//! KV operations tests for SQLite backend

use super::SqliteGraphBackend;
use crate::backend::GraphBackend;
use crate::backend::native::v3::kv_store::types::KvValue;
use crate::snapshot::SnapshotId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kv_set_and_get() {
        // Test basic set and get operations
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Set a value
        backend
            .kv_set(b"test_key".to_vec(), KvValue::Integer(42), None)
            .unwrap();

        // Get the value back
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"test_key").unwrap();
        assert_eq!(result, Some(KvValue::Integer(42)));
    }

    #[test]
    fn test_kv_get_missing_key() {
        // Test getting a non-existent key returns None
        let backend = SqliteGraphBackend::in_memory().unwrap();

        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"missing_key").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_kv_delete() {
        // Test deleting a key
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Set a value
        backend
            .kv_set(
                b"delete_me".to_vec(),
                KvValue::String("value".to_string()),
                None,
            )
            .unwrap();

        // Verify it exists
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"delete_me").unwrap();
        assert_eq!(result, Some(KvValue::String("value".to_string())));

        // Delete it
        backend.kv_delete(b"delete_me").unwrap();

        // Verify it's gone
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"delete_me").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_kv_delete_nonexistent() {
        // Test deleting a non-existent key should succeed (idempotent)
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Delete non-existent key - should not error
        backend.kv_delete(b"nonexistent").unwrap();
    }

    #[test]
    fn test_kv_update() {
        // Test updating an existing key
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Set initial value
        backend
            .kv_set(b"update_key".to_vec(), KvValue::Integer(100), None)
            .unwrap();

        // Update to new value
        backend
            .kv_set(b"update_key".to_vec(), KvValue::Integer(200), None)
            .unwrap();

        // Verify updated value
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"update_key").unwrap();
        assert_eq!(result, Some(KvValue::Integer(200)));
    }

    #[test]
    fn test_kv_different_value_types() {
        // Test storing different value types
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Bytes
        backend
            .kv_set(
                b"bytes_key".to_vec(),
                KvValue::Bytes(vec![1, 2, 3, 4]),
                None,
            )
            .unwrap();
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"bytes_key").unwrap();
        assert_eq!(result, Some(KvValue::Bytes(vec![1, 2, 3, 4])));

        // String
        backend
            .kv_set(
                b"string_key".to_vec(),
                KvValue::String("hello world".to_string()),
                None,
            )
            .unwrap();
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"string_key").unwrap();
        assert_eq!(result, Some(KvValue::String("hello world".to_string())));

        // Float
        backend
            .kv_set(b"float_key".to_vec(), KvValue::Float(3.14159), None)
            .unwrap();
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"float_key").unwrap();
        assert_eq!(result, Some(KvValue::Float(3.14159)));

        // Boolean
        backend
            .kv_set(b"bool_key".to_vec(), KvValue::Boolean(true), None)
            .unwrap();
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"bool_key").unwrap();
        assert_eq!(result, Some(KvValue::Boolean(true)));

        // Json
        backend
            .kv_set(
                b"json_key".to_vec(),
                KvValue::Json(serde_json::json!({"key": "value", "number": 42})),
                None,
            )
            .unwrap();
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"json_key").unwrap();
        assert_eq!(
            result,
            Some(KvValue::Json(
                serde_json::json!({"key": "value", "number": 42})
            ))
        );
    }

    #[test]
    fn test_kv_with_ttl() {
        // Test TTL expiration
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Set a value with 2 second TTL
        backend
            .kv_set(
                b"ttl_key".to_vec(),
                KvValue::String("expires".to_string()),
                Some(2),
            )
            .unwrap();

        // Should be visible immediately
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"ttl_key").unwrap();
        assert_eq!(result, Some(KvValue::String("expires".to_string())));

        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_secs(3));

        // Should be expired now
        let snapshot = SnapshotId::current();
        let result = backend.kv_get(snapshot, b"ttl_key").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_kv_binary_keys() {
        // Test that binary keys work correctly
        let backend = SqliteGraphBackend::in_memory().unwrap();

        // Keys with various binary content
        let key1: Vec<u8> = vec![0x00, 0x01, 0x02, 0xFF];
        let key2: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];

        backend
            .kv_set(key1.clone(), KvValue::Integer(1), None)
            .unwrap();
        backend
            .kv_set(key2.clone(), KvValue::Integer(2), None)
            .unwrap();

        let snapshot = SnapshotId::current();
        let result1 = backend.kv_get(snapshot, &key1).unwrap();
        let result2 = backend.kv_get(snapshot, &key2).unwrap();

        assert_eq!(result1, Some(KvValue::Integer(1)));
        assert_eq!(result2, Some(KvValue::Integer(2)));
    }
}
