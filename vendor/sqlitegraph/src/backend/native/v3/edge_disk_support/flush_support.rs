use super::*;

pub(super) fn flush(store: &V3EdgeStore) -> NativeResult<()> {
    let db_path = match &store.db_path {
        Some(path) => path.clone(),
        None => {
            store.dirty_clusters.write().clear();
            return Ok(());
        }
    };

    let dirty = store.dirty_clusters.write();
    if dirty.is_empty() {
        return Ok(());
    }

    let clusters_to_flush: Vec<((i64, Direction), V3EdgeCluster)> =
        dirty.iter().map(|(k, v)| (*k, v.clone())).collect();
    drop(dirty);

    let packed_capacity = (store.page_size as usize)
        .checked_sub(PACKED_EDGE_PAGE_HEADER_SIZE + PACKED_EDGE_PAGE_SLOT_SIZE)
        .ok_or_else(|| NativeBackendError::SerializationError {
            context: format!(
                "edge page size {} too small for packed edge encoding",
                store.page_size
            ),
        })?;
    let overflow_capacity = (store.page_size as usize)
        .checked_sub(EDGE_CLUSTER_PAGE_HEADER_SIZE)
        .ok_or_else(|| NativeBackendError::SerializationError {
            context: format!(
                "edge page size {} too small for multi-page encoding",
                store.page_size
            ),
        })?;

    let mut packed_entries: Vec<((i64, Direction), Vec<u8>)> = Vec::new();
    let mut packed_bytes_used = PACKED_EDGE_PAGE_HEADER_SIZE;

    for ((src, dir), cluster) in clusters_to_flush {
        let mut cluster = cluster;
        cluster.sort_for_weighted_queries();
        let cluster_bytes = cluster.serialize()?;
        let packed_size = PACKED_EDGE_PAGE_SLOT_SIZE + cluster_bytes.len();

        if cluster_bytes.len() <= packed_capacity {
            if packed_bytes_used + packed_size > store.page_size as usize {
                flush_packed_entries(store, &db_path, &mut packed_entries, &mut packed_bytes_used)?;
            }
            packed_bytes_used += packed_size;
            packed_entries.push(((src, dir), cluster_bytes));
            continue;
        }

        flush_packed_entries(store, &db_path, &mut packed_entries, &mut packed_bytes_used)?;
        flush_cluster_overflow_pages(store, &db_path, src, dir, cluster_bytes, overflow_capacity)?;
    }

    flush_packed_entries(store, &db_path, &mut packed_entries, &mut packed_bytes_used)?;
    store.dirty_clusters.write().clear();
    checkpoint_and_truncate_wal(store);
    store.persist_btree_metadata()?;

    Ok(())
}

fn flush_packed_entries(
    store: &V3EdgeStore,
    db_path: &Path,
    entries: &mut Vec<((i64, Direction), Vec<u8>)>,
    bytes_used: &mut usize,
) -> NativeResult<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let packed_page_id = {
        let mut allocator = store.allocator.write();
        allocator
            .allocate()
            .map_err(|e| NativeBackendError::IoError {
                context: "Failed to allocate packed edge page".to_string(),
                source: std::io::Error::other(e.to_string()),
            })?
    };

    let page_bytes = encode_packed_edge_page(store.page_size as usize, entries)?;
    store.write_page_to_disk(db_path, packed_page_id, &page_bytes)?;

    let mut btree = store.btree.write();
    for ((src, dir), _) in entries.iter() {
        let key = edge_key(*src, *dir);
        let _ = btree.insert(key, packed_page_id);
    }

    entries.clear();
    *bytes_used = PACKED_EDGE_PAGE_HEADER_SIZE;
    Ok(())
}

fn flush_cluster_overflow_pages(
    store: &V3EdgeStore,
    db_path: &Path,
    src: i64,
    dir: Direction,
    cluster_bytes: Vec<u8>,
    overflow_capacity: usize,
) -> NativeResult<()> {
    let first_page_id = {
        let mut allocator = store.allocator.write();
        allocator
            .allocate()
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to allocate edge page for ({}, {:?})", src, dir),
                source: std::io::Error::other(e.to_string()),
            })?
    };

    let total_pages = cluster_bytes.len().max(1).div_ceil(overflow_capacity);
    let mut page_ids = Vec::with_capacity(total_pages);
    page_ids.push(first_page_id);
    if total_pages > 1 {
        let mut allocator = store.allocator.write();
        for _ in 1..total_pages {
            let overflow_page_id =
                allocator
                    .allocate()
                    .map_err(|e| NativeBackendError::IoError {
                        context: format!(
                            "Failed to allocate overflow edge page for ({}, {:?})",
                            src, dir
                        ),
                        source: std::io::Error::other(e.to_string()),
                    })?;
            page_ids.push(overflow_page_id);
        }
    }

    let encoded_pages =
        encode_edge_cluster_pages(&cluster_bytes, store.page_size as usize, &page_ids)?;
    for (page_id, page_bytes) in page_ids.into_iter().zip(encoded_pages) {
        store.write_page_to_disk(db_path, page_id, &page_bytes)?;
    }

    let mut btree = store.btree.write();
    let key = edge_key(src, dir);
    let _ = btree.insert(key, first_page_id);
    Ok(())
}

fn checkpoint_and_truncate_wal(store: &V3EdgeStore) {
    if let Some(ref wal) = store.wal {
        let mut wal_guard = wal.write();
        let btree = store.btree.read();
        let root_page_id = btree.root_page_id();
        let tree_height = btree.tree_height();

        let _ = wal_guard.checkpoint(
            root_page_id,
            0,
            tree_height,
            0,
            &PersistentHeaderV3::new_v3(),
        );
        let _ = wal_guard.flush();
        let _ = wal_guard.truncate();
    }
}
