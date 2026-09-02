use super::{HnswIndexMetadata, HnswSearchConfig, V3Backend};
use crate::SqliteGraphError;
use crate::hnsw::HnswIndex;

fn normalize_vector_result_id(hnsw: &HnswIndex, vector_id: u64) -> Result<i64, SqliteGraphError> {
    let normalized_id = hnsw
        .get_vector(vector_id)
        .map_err(|e| {
            SqliteGraphError::validation(format!(
                "HNSW metadata lookup failed for vector {}: {:?}",
                vector_id, e
            ))
        })?
        .map(|(_, metadata)| {
            metadata
                .get("node_id")
                .and_then(|value| value.as_i64())
                .unwrap_or(vector_id as i64)
        })
        .unwrap_or(vector_id as i64);
    Ok(normalized_id)
}

fn validate_ef_search_override(config: HnswSearchConfig) -> Result<(), SqliteGraphError> {
    if let Some(ef_search) = config.ef_search_override
        && (ef_search == 0 || ef_search > 200)
    {
        return Err(SqliteGraphError::validation(format!(
            "Invalid ef_search_override: {}. Must be in 1..=200",
            ef_search
        )));
    }

    Ok(())
}

pub(super) fn hnsw_exact_search(
    metadata_arc: &HnswIndexMetadata,
    query_vector: &[f32],
    k: usize,
    ef_search_override: Option<usize>,
) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
    let mut hnsw = metadata_arc.lock_hnsw("<exact-search>")?;
    let original_ef_search = hnsw.config.ef_search;
    if let Some(ef_search) = ef_search_override {
        hnsw.config.ef_search = ef_search;
    }

    let results = hnsw
        .search(query_vector, k)
        .map_err(|e| SqliteGraphError::validation(format!("HNSW search error: {:?}", e)));
    hnsw.config.ef_search = original_ef_search;

    let results = results?;
    results
        .into_iter()
        .map(|(vector_id, distance)| {
            normalize_vector_result_id(&hnsw, vector_id).map(|id| (id, distance))
        })
        .collect()
}

pub(super) fn hnsw_vector_search_with_config(
    backend: &V3Backend,
    _index_name: &str,
    metadata_arc: &HnswIndexMetadata,
    query_vector: &[f32],
    k: usize,
    config: HnswSearchConfig,
) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
    validate_ef_search_override(config)?;

    #[cfg(feature = "turbovec")]
    {
        let count = *metadata_arc.lock_embedding_count(_index_name)?;
        if count > super::TURBOVEC_THRESHOLD && !config.force_exact {
            backend.ensure_turbovec_index(_index_name)?;

            let turbovec = metadata_arc.lock_turbovec(_index_name)?;
            if let Some(ref index) = *turbovec {
                return Ok(backend.turbovec_search(index, query_vector, k));
            }
        }
    }

    backend.hnsw_exact_search(metadata_arc, query_vector, k, config.ef_search_override)
}
