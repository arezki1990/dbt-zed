use super::{HnswIndexMetadata, TURBOVEC_BIT_WIDTH, V3Backend};
use crate::SqliteGraphError;

fn metadata_arc(
    backend: &V3Backend,
    index_name: &str,
) -> Result<HnswIndexMetadata, SqliteGraphError> {
    let indexes = backend.hnsw_indexes.read();
    indexes.get(index_name).cloned().ok_or_else(|| {
        SqliteGraphError::validation(format!("HNSW index not found: {}", index_name))
    })
}

pub(super) fn build_turbovec_index(
    backend: &V3Backend,
    index_name: &str,
) -> Result<(), SqliteGraphError> {
    let metadata_arc = metadata_arc(backend, index_name)?;

    let turbovec = metadata_arc.lock_turbovec(index_name)?;
    if turbovec.is_some() {
        return Ok(());
    }
    drop(turbovec);

    let hnsw = metadata_arc.lock_hnsw(index_name)?;
    let count = hnsw.vector_count();
    if count == 0 {
        return Ok(());
    }

    let mut embeddings: Vec<f32> = Vec::with_capacity(count * metadata_arc.dimension);
    let mut ids: Vec<u64> = Vec::with_capacity(count);

    for i in 1..=count {
        if let Ok(Some((vector, metadata))) = hnsw.get_vector(i as u64) {
            let id = metadata
                .get("node_id")
                .and_then(|v| v.as_u64())
                .unwrap_or(i as u64);

            embeddings.extend_from_slice(&vector);
            ids.push(id);
        }
    }

    drop(hnsw);

    let mut turbovec_index = turbovec::IdMapIndex::new(metadata_arc.dimension, TURBOVEC_BIT_WIDTH)
        .map_err(|e| {
            SqliteGraphError::validation(format!("Turbovec construction failed: {}", e))
        })?;
    turbovec_index
        .add_with_ids(&embeddings, &ids)
        .map_err(|e| SqliteGraphError::validation(format!("Turbovec add failed: {}", e)))?;
    let mut turbovec = metadata_arc.lock_turbovec(index_name)?;
    *turbovec = Some(turbovec_index);

    Ok(())
}

pub(super) fn ensure_turbovec_index(
    backend: &V3Backend,
    index_name: &str,
) -> Result<(), SqliteGraphError> {
    let metadata_arc = metadata_arc(backend, index_name)?;

    let turbovec = metadata_arc.lock_turbovec(index_name)?;
    if turbovec.is_some() {
        return Ok(());
    }
    drop(turbovec);

    build_turbovec_index(backend, index_name)
}

pub(super) fn turbovec_search(
    index: &turbovec::IdMapIndex,
    query_vector: &[f32],
    k: usize,
) -> Vec<(i64, f32)> {
    let (scores, ids) = index.search(query_vector, k);
    scores
        .into_iter()
        .zip(ids)
        .map(|(distance, node_id)| (node_id as i64, distance))
        .collect()
}
