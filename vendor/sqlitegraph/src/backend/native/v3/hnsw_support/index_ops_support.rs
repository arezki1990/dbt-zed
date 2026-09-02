use super::{HnswIndexMetadata, V3Backend};
use crate::SqliteGraphError;
use crate::backend::native::v3::KvValue;
use crate::hnsw::{HnswConfigBuilder, HnswIndex};
use crate::snapshot::SnapshotId;
use std::sync::Arc;

pub(super) fn hnsw_metadata(
    backend: &V3Backend,
    index_name: &str,
) -> Result<HnswIndexMetadata, SqliteGraphError> {
    let indexes = backend.hnsw_indexes.read();
    indexes.get(index_name).cloned().ok_or_else(|| {
        SqliteGraphError::validation(format!("HNSW index not found: {}", index_name))
    })
}

pub(super) fn validate_hnsw_vector_dimension(
    metadata_arc: &HnswIndexMetadata,
    vector: &[f32],
    expected_label: &str,
) -> Result<(), SqliteGraphError> {
    if vector.len() != metadata_arc.dimension {
        return Err(SqliteGraphError::validation(format!(
            "{expected_label} dimension mismatch: expected {}, got {}",
            metadata_arc.dimension,
            vector.len()
        )));
    }

    Ok(())
}

pub(super) fn create_hnsw_index(
    backend: &V3Backend,
    index_name: &str,
    dimension: usize,
) -> Result<(), SqliteGraphError> {
    backend.ensure_hnsw_not_in_graph_transaction("create_hnsw_index")?;

    {
        let indexes = backend.hnsw_indexes.read();
        if indexes.contains_key(index_name) {
            return Ok(());
        }
    }

    let config = HnswConfigBuilder::new()
        .dimension(dimension)
        .distance_metric(crate::hnsw::DistanceMetric::Euclidean)
        .build()
        .map_err(|e| SqliteGraphError::validation(format!("HNSW config error: {:?}", e)))?;

    #[cfg(feature = "native-v3")]
    let hnsw_index = {
        let storage_handle =
            crate::hnsw::v3_storage::V3VectorStorageHandle::new(backend, index_name);
        HnswIndex::with_storage(index_name, config, Box::new(storage_handle)).map_err(|e| {
            SqliteGraphError::validation(format!("HNSW index creation error: {:?}", e))
        })?
    };

    #[cfg(not(feature = "native-v3"))]
    let hnsw_index = {
        HnswIndex::new(index_name, config).map_err(|e| {
            SqliteGraphError::validation(format!("HNSW index creation error: {:?}", e))
        })?
    };

    let metadata_key = format!("hnsw:{}:metadata", index_name);
    let metadata_value = serde_json::json!({
        "name": index_name,
        "dimension": dimension,
        "created_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| {
                SqliteGraphError::validation(format!(
                    "System clock before UNIX epoch during HNSW metadata creation: {}",
                    e
                ))
            })?
            .as_secs()
    });
    backend.kv_set_v3(
        metadata_key.into_bytes(),
        KvValue::Json(metadata_value),
        None,
    );

    let mut indexes = backend.hnsw_indexes.write();
    indexes.insert(
        index_name.to_string(),
        HnswIndexMetadata {
            hnsw_index: Arc::new(std::sync::Mutex::new(hnsw_index)),
            #[cfg(feature = "turbovec")]
            turbovec_index: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(feature = "turbovec")]
            embedding_count: Arc::new(std::sync::Mutex::new(0)),
            dimension,
        },
    );

    Ok(())
}

pub(super) fn has_hnsw_index(backend: &V3Backend, index_name: &str) -> bool {
    let indexes = backend.hnsw_indexes.read();
    indexes.contains_key(index_name)
}

pub(super) fn hnsw_embedding_count(
    backend: &V3Backend,
    index_name: &str,
) -> Result<usize, SqliteGraphError> {
    let metadata = hnsw_metadata(backend, index_name)?;

    #[cfg(feature = "turbovec")]
    {
        return Ok(*metadata.lock_embedding_count(index_name)?);
    }

    #[cfg(not(feature = "turbovec"))]
    {
        let hnsw = metadata.lock_hnsw(index_name)?;
        Ok(hnsw.vector_count())
    }
}

pub(super) fn hnsw_turbovec_ready(
    backend: &V3Backend,
    index_name: &str,
) -> Result<bool, SqliteGraphError> {
    let metadata = hnsw_metadata(backend, index_name)?;

    #[cfg(feature = "turbovec")]
    {
        Ok(metadata.lock_turbovec(index_name)?.is_some())
    }

    #[cfg(not(feature = "turbovec"))]
    {
        let _ = metadata;
        Ok(false)
    }
}

pub(super) fn get_hnsw_index(
    backend: &V3Backend,
    index_name: &str,
) -> Result<Option<Arc<std::sync::Mutex<HnswIndex>>>, SqliteGraphError> {
    backend.ensure_hnsw_not_in_graph_transaction("get_hnsw_index")?;
    let indexes = backend.hnsw_indexes.read();
    Ok(indexes
        .get(index_name)
        .map(|metadata| metadata.hnsw_index.clone()))
}

pub(super) fn delete_hnsw_index(
    backend: &V3Backend,
    index_name: &str,
) -> Result<(), SqliteGraphError> {
    backend.ensure_hnsw_not_in_graph_transaction("delete_hnsw_index")?;
    let mut indexes = backend.hnsw_indexes.write();
    indexes.remove(index_name);

    let metadata_key = format!("hnsw:{}:metadata", index_name);
    backend.kv_delete_v3(metadata_key.as_bytes());

    let vector_prefix = format!("hnsw:{}:vector:", index_name);
    let snapshot_id = SnapshotId::current();
    let vectors = backend.kv_prefix_scan_v3(snapshot_id, vector_prefix.as_bytes());
    for (key, _) in vectors {
        backend.kv_delete_v3(&key);
    }

    Ok(())
}
