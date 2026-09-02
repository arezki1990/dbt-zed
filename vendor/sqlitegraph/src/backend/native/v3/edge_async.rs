use super::AsyncFileCoordinator;
use super::edge_compat::{
    Direction, V3EdgeCluster, decode_edge_cluster_page_header, decode_packed_edge_page, edge_key,
};
use crate::backend::native::v3::btree::BTreeManager;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

type NeighborCache = RwLock<HashMap<(i64, Direction), Arc<[i64]>>>;
type WeightedNeighborCache = RwLock<HashMap<(i64, Direction), Arc<[(i64, f32)]>>>;
type EdgeTypeCache = RwLock<HashMap<(i64, i64, Direction), String>>;

/// Standalone async neighbor retrieval function that takes references to avoid
/// holding RwLockReadGuard across awaits.
pub async fn neighbors_async(
    btree_lock: &RwLock<BTreeManager>,
    cache_lock: &NeighborCache,
    edge_types_lock: &EdgeTypeCache,
    page_size: u64,
    src: i64,
    dir: Direction,
    async_coordinator: &AsyncFileCoordinator,
) -> Result<Arc<[i64]>, crate::errors::SqliteGraphError> {
    let key = (src, dir);

    {
        let cache = cache_lock.read();
        if let Some(neighbors) = cache.get(&key) {
            return Ok(neighbors.clone());
        }
    }

    let neighbors = load_neighbors_from_disk_async(
        btree_lock,
        edge_types_lock,
        page_size,
        src,
        dir,
        async_coordinator,
    )
    .await?;

    if !neighbors.is_empty() {
        let mut cache = cache_lock.write();
        cache.insert(key, neighbors.clone());
    }

    Ok(neighbors)
}

/// Standalone async weighted neighbor retrieval function that takes references
/// to avoid holding RwLockReadGuard across awaits.
pub async fn neighbors_weighted_async(
    btree_lock: &RwLock<BTreeManager>,
    cache_weighted_lock: &WeightedNeighborCache,
    edge_types_lock: &EdgeTypeCache,
    page_size: u64,
    src: i64,
    dir: Direction,
    async_coordinator: &AsyncFileCoordinator,
) -> Result<Arc<[(i64, f32)]>, crate::errors::SqliteGraphError> {
    let key = (src, dir);

    {
        let cache = cache_weighted_lock.read();
        if let Some(neighbors) = cache.get(&key) {
            return Ok(neighbors.clone());
        }
    }

    let neighbors = load_neighbors_weighted_from_disk_async(
        btree_lock,
        edge_types_lock,
        page_size,
        src,
        dir,
        async_coordinator,
    )
    .await?;

    if !neighbors.is_empty() {
        let mut cache = cache_weighted_lock.write();
        cache.insert(key, neighbors.clone());
    }

    Ok(neighbors)
}

async fn load_neighbors_from_disk_async(
    btree_lock: &RwLock<BTreeManager>,
    edge_types_lock: &EdgeTypeCache,
    page_size: u64,
    src: i64,
    dir: Direction,
    async_coordinator: &AsyncFileCoordinator,
) -> Result<Arc<[i64]>, crate::errors::SqliteGraphError> {
    let key = edge_key(src, dir);
    let btree = btree_lock.read().clone();

    let page_id = match btree.lookup_async(key, async_coordinator).await {
        Ok(Some(pid)) => pid,
        _ => return Ok(Arc::from([])),
    };

    match load_cluster_async(page_id, src, dir, page_size, async_coordinator).await {
        Ok(cluster) => {
            let edges_with_types = cluster.edges_with_types();
            let mut edge_types = edge_types_lock.write();
            for (dst, edge_type) in edges_with_types {
                if let Some(et) = edge_type {
                    edge_types.insert((src, dst, dir), et);
                }
            }

            let neighbors: Vec<i64> = cluster.dsts();
            Ok(Arc::from(neighbors.into_boxed_slice()))
        }
        Err(_) => Ok(Arc::from([])),
    }
}

async fn load_neighbors_weighted_from_disk_async(
    btree_lock: &RwLock<BTreeManager>,
    edge_types_lock: &EdgeTypeCache,
    page_size: u64,
    src: i64,
    dir: Direction,
    async_coordinator: &AsyncFileCoordinator,
) -> Result<Arc<[(i64, f32)]>, crate::errors::SqliteGraphError> {
    let key = edge_key(src, dir);
    let btree = btree_lock.read().clone();

    let page_id = match btree.lookup_async(key, async_coordinator).await {
        Ok(Some(pid)) => pid,
        _ => return Ok(Arc::from([])),
    };

    match load_cluster_async(page_id, src, dir, page_size, async_coordinator).await {
        Ok(cluster) => {
            let mut edge_types = edge_types_lock.write();
            let mut neighbors = Vec::with_capacity(cluster.edges.len());
            for edge in &cluster.edges {
                let edge_type = V3EdgeCluster::extract_edge_type(&edge.edge_data);
                if let Some(et) = edge_type {
                    edge_types.insert((src, edge.neighbor_id, dir), et);
                }
                let weight = V3EdgeCluster::extract_edge_weight(&edge.edge_data);
                neighbors.push((edge.neighbor_id, weight));
            }
            neighbors.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
            Ok(Arc::from(neighbors.into_boxed_slice()))
        }
        Err(_) => Ok(Arc::from([])),
    }
}

async fn load_cluster_async(
    page_id: u64,
    src: i64,
    dir: Direction,
    page_size: u64,
    async_coordinator: &AsyncFileCoordinator,
) -> Result<V3EdgeCluster, crate::errors::SqliteGraphError> {
    let mut current_page_id = page_id;
    let mut cluster_bytes = Vec::new();

    let page_buffer = vec![0u8; page_size as usize];
    let (mut buffer, _) = async_coordinator
        .read_page(current_page_id, page_buffer)
        .await?;

    loop {
        if let Some(decoded_bytes) = decode_packed_edge_page(&buffer, src, dir).ok().flatten() {
            let mut cluster = V3EdgeCluster::deserialize(&decoded_bytes, page_id).map_err(|e| {
                crate::errors::SqliteGraphError::validation(format!(
                    "Failed to deserialize edge cluster: {}",
                    e
                ))
            })?;
            cluster.page_id = 0;
            return Ok(cluster);
        }

        if let Some((payload_len, next_page_id)) = decode_edge_cluster_page_header(&buffer) {
            let payload_end = payload_len + super::edge_compat::EDGE_CLUSTER_PAGE_HEADER_SIZE;
            cluster_bytes.extend_from_slice(
                &buffer[super::edge_compat::EDGE_CLUSTER_PAGE_HEADER_SIZE..payload_end],
            );
            if next_page_id == 0 {
                break;
            }
            current_page_id = next_page_id;
            let next_page_buffer = vec![0u8; page_size as usize];
            let (next_buffer, _) = async_coordinator
                .read_page(current_page_id, next_page_buffer)
                .await?;
            buffer = next_buffer;
        } else {
            return V3EdgeCluster::deserialize(&buffer, page_id).map_err(|e| {
                crate::errors::SqliteGraphError::validation(format!(
                    "Failed to deserialize edge cluster fallback: {}",
                    e
                ))
            });
        }
    }

    V3EdgeCluster::deserialize(&cluster_bytes, page_id).map_err(|e| {
        crate::errors::SqliteGraphError::validation(format!(
            "Failed to deserialize edge cluster end: {}",
            e
        ))
    })
}
