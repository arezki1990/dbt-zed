use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::backend::HnswSearchConfig;

const TOTAL_VECTORS: usize = 10_000;
const DIMENSION: usize = 128;
const NODE_ID_BASE: i64 = 50_000;
const SAMPLE_ORDINALS: &[usize] = &[0, 1, 127, 1024, 4095, 8191, 9_999];

fn deterministic_vector(ordinal: usize) -> Vec<f32> {
    (0..DIMENSION)
        .map(|dim| {
            let mut x = ((ordinal as u64) << 32) ^ dim as u64 ^ 0x9E37_79B9_7F4A_7C15;
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 27;
            x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^= x >> 31;
            if x & 1 == 0 { -1.0 } else { 1.0 }
        })
        .collect()
}

fn checksum(vector: &[f32]) -> f32 {
    vector
        .iter()
        .enumerate()
        .map(|(idx, value)| *value * (idx as f32 + 1.0))
        .sum()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("hnsw_10k_integrity.graph");

    println!("=== HNSW 10K Integrity Job ===");
    println!("db_path={}", db_path.display());
    println!("vectors={} dimension={}", TOTAL_VECTORS, DIMENSION);

    let backend = V3Backend::create(&db_path)?;
    backend.create_hnsw_index("integrity_index", DIMENSION)?;

    for ordinal in 0..TOTAL_VECTORS {
        if ordinal % 2_000 == 0 {
            println!("inserted {} vectors...", ordinal);
        }
        let vector = deterministic_vector(ordinal);
        let node_id = NODE_ID_BASE + ordinal as i64;
        let metadata = serde_json::json!({
            "node_id": node_id,
            "ordinal": ordinal,
            "checksum": checksum(&vector),
        });
        backend.insert_hnsw_vector("integrity_index", &vector, Some(metadata))?;
    }

    backend.flush_to_disk()?;

    let tracked_count = backend.hnsw_embedding_count("integrity_index")?;
    assert_eq!(
        tracked_count, TOTAL_VECTORS,
        "embedding tracker mismatch: expected {}, got {}",
        TOTAL_VECTORS, tracked_count
    );

    let index = backend
        .get_hnsw_index("integrity_index")?
        .ok_or("missing integrity_index after creation")?;
    let index = index.lock().unwrap();
    assert_eq!(
        index.vector_count(),
        TOTAL_VECTORS,
        "raw HNSW vector_count mismatch"
    );

    println!("verifying sampled stored vectors...");
    for &ordinal in SAMPLE_ORDINALS {
        let vector_id = ordinal as u64 + 1;
        let expected_vector = deterministic_vector(ordinal);
        let expected_checksum = checksum(&expected_vector);
        let expected_node_id = NODE_ID_BASE + ordinal as i64;

        let (stored_vector, metadata) = index
            .get_vector(vector_id)?
            .ok_or_else(|| format!("missing raw vector_id {}", vector_id))?;

        assert_eq!(
            stored_vector, expected_vector,
            "stored vector mismatch for ordinal {}",
            ordinal
        );
        assert_eq!(
            metadata["node_id"].as_i64(),
            Some(expected_node_id),
            "node_id metadata mismatch for ordinal {}",
            ordinal
        );
        assert_eq!(
            metadata["ordinal"].as_u64(),
            Some(ordinal as u64),
            "ordinal metadata mismatch for ordinal {}",
            ordinal
        );
        let stored_checksum = metadata["checksum"]
            .as_f64()
            .ok_or("missing checksum metadata")? as f32;
        assert!(
            (stored_checksum - expected_checksum).abs() < 1e-5,
            "checksum mismatch for ordinal {}: expected {}, got {}",
            ordinal,
            expected_checksum,
            stored_checksum
        );
    }
    drop(index);

    println!("warming routed search...");
    let warm_query = deterministic_vector(SAMPLE_ORDINALS[SAMPLE_ORDINALS.len() - 1]);
    let warm_results = backend.hnsw_vector_search("integrity_index", &warm_query, 5)?;
    assert!(
        !warm_results.is_empty(),
        "warm routed search returned no candidates"
    );
    assert!(
        warm_results
            .iter()
            .all(|(node_id, _)| *node_id >= NODE_ID_BASE
                && *node_id < NODE_ID_BASE + TOTAL_VECTORS as i64),
        "warm routed search returned non-normalized IDs: {:?}",
        warm_results
    );

    #[cfg(feature = "turbovec")]
    {
        assert!(
            backend.hnsw_turbovec_ready("integrity_index")?,
            "turbovec should be ready after a routed search over 10k vectors"
        );
    }

    println!("verifying exact and routed probe searches...");
    for &ordinal in SAMPLE_ORDINALS {
        let query = deterministic_vector(ordinal);
        let expected_node_id = NODE_ID_BASE + ordinal as i64;

        let exact = backend.hnsw_vector_search_with_config(
            "integrity_index",
            &query,
            1,
            HnswSearchConfig {
                force_exact: true,
                ef_search_override: Some(200),
            },
        )?;
        assert_eq!(
            exact[0].0, expected_node_id,
            "force_exact search failed for ordinal {}",
            ordinal
        );

        let routed = backend.hnsw_vector_search("integrity_index", &query, 5)?;
        assert!(
            !routed.is_empty(),
            "routed search returned no results for ordinal {}",
            ordinal
        );
        assert!(
            routed.iter().all(|(node_id, _)| *node_id >= NODE_ID_BASE
                && *node_id < NODE_ID_BASE + TOTAL_VECTORS as i64),
            "routed search returned non-normalized IDs for ordinal {}: {:?}",
            ordinal,
            routed
        );
    }

    println!("✓ HNSW 10K integrity job passed");
    println!("  - inserted {} vectors", TOTAL_VECTORS);
    println!("  - verified raw stored vector payloads on sampled IDs");
    println!("  - verified metadata node_id/ordinal/checksum integrity");
    println!("  - verified exact-route search on sampled probes");
    println!("  - verified routed search returns normalized node_id values");

    Ok(())
}
