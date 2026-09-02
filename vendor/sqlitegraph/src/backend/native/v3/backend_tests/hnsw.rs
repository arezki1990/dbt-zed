use super::*;
use tempfile::TempDir;

#[cfg(feature = "turbovec")]
#[test]
fn test_hnsw_vector_search_normalizes_ids_and_supports_exact_override() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("hnsw.graph");
    let backend = V3Backend::create(&db_path).unwrap();
    let dimension = 128;

    backend.create_hnsw_index("vectors", dimension).unwrap();

    let make_vector = |seed: usize| -> Vec<f32> {
        (0..dimension)
            .map(|j| {
                let mut x = ((seed as u64) << 32) ^ j as u64 ^ 0x9E37_79B9_7F4A_7C15;
                x ^= x >> 30;
                x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x ^= x >> 27;
                x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
                x ^= x >> 31;
                if x & 1 == 0 { -1.0 } else { 1.0 }
            })
            .collect()
    };

    for i in 0..100 {
        let node_id = 10_000 + i as i64;
        backend
            .insert_hnsw_vector(
                "vectors",
                &make_vector(i),
                Some(serde_json::json!({"node_id": node_id})),
            )
            .unwrap();
    }

    let small_result = backend
        .hnsw_vector_search("vectors", &make_vector(42), 1)
        .unwrap();
    assert!(
        (10_000..11_002).contains(&small_result[0].0),
        "expected normalized node_id, got {}",
        small_result[0].0
    );

    for i in 100..1002 {
        let node_id = 10_000 + i as i64;
        backend
            .insert_hnsw_vector(
                "vectors",
                &make_vector(i),
                Some(serde_json::json!({"node_id": node_id})),
            )
            .unwrap();
    }

    assert_eq!(backend.hnsw_embedding_count("vectors").unwrap(), 1002);
    assert!(backend.hnsw_turbovec_ready("vectors").unwrap());

    {
        let metadata = backend.hnsw_indexes.read();
        let index = metadata.get("vectors").unwrap();
        let mut turbovec = index.turbovec_index.lock().unwrap();
        *turbovec = None;
    }
    assert!(!backend.hnsw_turbovec_ready("vectors").unwrap());

    let exact_result = backend
        .hnsw_vector_search_with_config(
            "vectors",
            &make_vector(1001),
            1,
            HnswSearchConfig {
                force_exact: true,
                ef_search_override: Some(100),
            },
        )
        .unwrap();
    assert!(
        (10_000..11_002).contains(&exact_result[0].0),
        "expected normalized node_id, got {}",
        exact_result[0].0
    );
    assert!(
        !backend.hnsw_turbovec_ready("vectors").unwrap(),
        "force_exact should not rebuild the turbovec cache"
    );

    let turbovec_result = backend
        .hnsw_vector_search("vectors", &make_vector(1001), 1)
        .unwrap();
    assert!(
        (10_000..11_002).contains(&turbovec_result[0].0),
        "expected normalized node_id, got {}",
        turbovec_result[0].0
    );
    assert!(
        backend.hnsw_turbovec_ready("vectors").unwrap(),
        "default search should rebuild the turbovec cache when eligible"
    );
}

#[test]
fn test_hnsw_vector_search_rejects_invalid_ef_override() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("invalid_ef.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    backend.create_hnsw_index("vectors", 64).unwrap();

    let error = backend
        .hnsw_vector_search_with_config(
            "vectors",
            &[0.0; 64],
            1,
            HnswSearchConfig {
                force_exact: true,
                ef_search_override: Some(0),
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("Invalid ef_search_override"),
        "unexpected error: {error}"
    );
}
