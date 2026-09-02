use super::V3Backend;
use crate::SqliteGraphError;

#[cfg(not(feature = "turbovec"))]
pub(super) fn insert_hnsw_vector(
    backend: &V3Backend,
    index_name: &str,
    vector: &[f32],
    metadata: Option<serde_json::Value>,
) -> Result<(), SqliteGraphError> {
    backend.ensure_hnsw_not_in_graph_transaction("insert_hnsw_vector")?;
    let metadata_arc = backend.hnsw_metadata(index_name)?;
    V3Backend::validate_hnsw_vector_dimension(&metadata_arc, vector, "Vector")?;

    let mut hnsw = metadata_arc.lock_hnsw(index_name)?;
    hnsw.insert_vector(vector, metadata)
        .map_err(|e| SqliteGraphError::validation(format!("HNSW insert failed: {:?}", e)))?;

    Ok(())
}

#[cfg(feature = "turbovec")]
pub(super) fn insert_hnsw_vector_turbovec(
    backend: &V3Backend,
    index_name: &str,
    vector: &[f32],
    metadata: Option<serde_json::Value>,
) -> Result<(), SqliteGraphError> {
    backend.ensure_hnsw_not_in_graph_transaction("insert_hnsw_vector")?;
    let metadata_arc = backend.hnsw_metadata(index_name)?;
    V3Backend::validate_hnsw_vector_dimension(&metadata_arc, vector, "Vector")?;

    let mut hnsw = metadata_arc.lock_hnsw(index_name)?;
    hnsw.insert_vector(vector, metadata)
        .map_err(|e| SqliteGraphError::validation(format!("HNSW insert failed: {:?}", e)))?;
    drop(hnsw);

    let mut count = metadata_arc.lock_embedding_count(index_name)?;
    *count += 1;
    let current_count = *count;
    drop(count);

    if current_count == super::TURBOVEC_THRESHOLD + 1 {
        backend.build_turbovec_index(index_name)?;
    }

    Ok(())
}
