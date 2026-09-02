//! TDD Tests for V3VectorStorage
//!
//! These tests verify that V3VectorStorage correctly implements the VectorStorage trait
//! using V3Backend's KV store as the underlying storage.

#![allow(
    clippy::unnecessary_unwrap,
    reason = "test assertions use unwrap on known-Some values"
)]
use crate::backend::native::v3::V3Backend;
use crate::hnsw::storage::VectorBatch;
use serde_json::json;
use tempfile::TempDir;

fn create_test_vector(dimension: usize) -> Vec<f32> {
    (1..=dimension).map(|i| i as f32).collect()
}

/// Test that V3VectorStorage can be created
#[test]
fn test_v3_vector_storage_create() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let storage = backend.create_hnsw_storage("test_index");

    // Should be able to create storage
    assert!(storage.is_some());
}

/// Test storing and retrieving a single vector
#[test]
fn test_v3_storage_store_and_get() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    let vector = create_test_vector(128);
    let metadata = Some(json!({"source": "test"}));

    // Store vector
    let id = storage.store_vector(&vector, metadata.clone()).unwrap();
    assert_eq!(id, 1); // First vector should have ID 1

    // Retrieve vector
    let retrieved = storage.get_vector(id).unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), vector);

    // Retrieve with metadata
    let retrieved_with_meta = storage.get_vector_with_metadata(id).unwrap();
    assert!(retrieved_with_meta.is_some());
    let (retrieved_vec, retrieved_meta) = retrieved_with_meta.unwrap();
    assert_eq!(retrieved_vec, vector);
    assert_eq!(retrieved_meta, metadata.unwrap());
}

/// Test storing vector with explicit ID
#[test]
fn test_v3_storage_store_with_explicit_id() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    let vector = create_test_vector(64);
    let explicit_id = 42u64;

    // Store with explicit ID
    storage
        .store_vector_with_id(explicit_id, vector.clone(), None)
        .unwrap();

    // Retrieve
    let retrieved = storage.get_vector(explicit_id).unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), vector);
}

/// Test storing multiple vectors and counting
#[test]
fn test_v3_storage_vector_count() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    // Initially empty
    assert_eq!(storage.vector_count().unwrap(), 0);

    // Store 5 vectors
    for i in 0..5 {
        let vector = create_test_vector(32);
        let id = storage.store_vector(&vector, None).unwrap();
        assert_eq!(id, (i + 1) as u64);
    }

    // Count should be 5
    assert_eq!(storage.vector_count().unwrap(), 5);
}

/// Test listing all vectors
///
/// NOTE: list_vectors() currently returns empty because V3 KV store
/// doesn't support prefix scan needed to list all vector keys.
/// This is a known limitation documented in the implementation.
#[test]
fn test_v3_storage_list_vectors() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    // Store 3 vectors
    let mut ids = Vec::new();
    for _ in 0..3 {
        let vector = create_test_vector(16);
        let id = storage.store_vector(&vector, None).unwrap();
        ids.push(id);
    }

    // List currently returns empty due to KV prefix scan limitation
    // This documents the current behavior
    let listed = storage.list_vectors().unwrap();
    assert!(
        listed.is_empty(),
        "list_vectors returns empty due to prefix scan limitation"
    );

    // But count should still work
    assert_eq!(storage.vector_count().unwrap(), 3);
}

/// Test deleting vectors
#[test]
fn test_v3_storage_delete_vector() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    // Store a vector
    let vector = create_test_vector(32);
    let id = storage.store_vector(&vector, None).unwrap();

    // Verify it exists
    assert!(storage.get_vector(id).unwrap().is_some());
    assert_eq!(storage.vector_count().unwrap(), 1);

    // Delete it
    storage.delete_vector(id).unwrap();

    // Verify it's gone
    assert!(storage.get_vector(id).unwrap().is_none());
    assert_eq!(storage.vector_count().unwrap(), 0);
}

/// Test batch storage
#[test]
fn test_v3_storage_batch_store() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let mut storage = backend.create_hnsw_storage("test_index").unwrap();

    // Create batch vectors and metadata
    let mut vectors = Vec::new();
    let mut metadatas = Vec::new();
    for _ in 0..10 {
        vectors.push(create_test_vector(8));
        metadatas.push(Some(json!({"batch": true})));
    }

    // Create batch
    let batch = VectorBatch::new(vectors, metadatas).unwrap();

    // Store batch
    let ids = storage.store_batch(batch).unwrap();
    assert_eq!(ids.len(), 10);

    // Verify count
    assert_eq!(storage.vector_count().unwrap(), 10);

    // Verify each vector
    for id in ids {
        let retrieved = storage.get_vector(id).unwrap();
        assert!(retrieved.is_some());
    }
}

/// Test getting non-existent vector returns None
#[test]
fn test_v3_storage_get_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();
    let storage = backend.create_hnsw_storage("test_index").unwrap();

    // Get non-existent ID
    let result = storage.get_vector(999).unwrap();
    assert!(result.is_none());

    let result_with_meta = storage.get_vector_with_metadata(999).unwrap();
    assert!(result_with_meta.is_none());
}

/// Test that storage survives backend operations
#[test]
fn test_v3_storage_with_graph_operations() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");

    let backend = V3Backend::create(&db_path).unwrap();

    // Perform graph operations
    use crate::backend::{EdgeSpec, GraphBackend, NodeSpec};

    let node1 = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "a.rs".to_string(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();

    let node2 = backend
        .insert_node(NodeSpec {
            kind: "File".to_string(),
            name: "b.rs".to_string(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: node1,
            to: node2,
            edge_type: "depends_on".to_string(),
            data: json!({}),
        })
        .unwrap();

    // Now use HNSW storage
    let mut storage = backend.create_hnsw_storage("embeddings").unwrap();
    let vector = create_test_vector(64);
    let id = storage.store_vector(&vector, None).unwrap();

    // Verify it works
    let retrieved = storage.get_vector(id).unwrap();
    assert!(retrieved.is_some());
}
