use super::{Arc, BTreeManager, DEFAULT_PAGE_SIZE, LruCache, NodePage, NodeRecordV3, RwLock};

/// Standalone async node lookup function that takes references to avoid holding
/// `RwLockReadGuard` across awaits.
pub async fn lookup_node_async(
    btree_manager: &Option<BTreeManager>,
    page_cache: &Arc<RwLock<LruCache<u64, Vec<u8>>>>,
    unpacked_page_cache: &Arc<RwLock<LruCache<u64, Arc<NodePage>>>>,
    node_id: i64,
    async_coordinator: &crate::backend::native::v3::AsyncFileCoordinator,
) -> Result<Option<NodeRecordV3>, crate::errors::SqliteGraphError> {
    let page_id = match lookup_page_async(btree_manager, node_id, async_coordinator).await? {
        Some(pid) => pid,
        None => return Ok(None),
    };

    {
        let cache = unpacked_page_cache.read();
        if let Some(cached_page) = cache.peek(&page_id) {
            return match cached_page.find_node(node_id) {
                Some(node_ref) => Ok(Some(node_ref.clone())),
                None => Ok(None),
            };
        }
    }

    let page_data = load_page_cache_async(page_cache, page_id, async_coordinator).await?;
    let page = NodePage::unpack(&page_data).map_err(|e| {
        crate::errors::SqliteGraphError::validation(format!("Failed to unpack node page: {}", e))
    })?;

    let result = match page.find_node(node_id) {
        Some(node_ref) => Ok(Some(node_ref.clone())),
        None => Ok(None),
    };

    unpacked_page_cache.write().put(page_id, Arc::new(page));
    result
}

async fn lookup_page_async(
    btree_manager: &Option<BTreeManager>,
    node_id: i64,
    async_coordinator: &crate::backend::native::v3::AsyncFileCoordinator,
) -> Result<Option<u64>, crate::errors::SqliteGraphError> {
    if let Some(btree) = btree_manager {
        return btree.lookup_async(node_id, async_coordinator).await;
    }
    Ok(None)
}

async fn load_page_cache_async(
    page_cache: &Arc<RwLock<LruCache<u64, Vec<u8>>>>,
    page_id: u64,
    async_coordinator: &crate::backend::native::v3::AsyncFileCoordinator,
) -> Result<Vec<u8>, crate::errors::SqliteGraphError> {
    {
        let cache = page_cache.read();
        if let Some(cached) = cache.peek(&page_id) {
            return Ok(cached.clone());
        }
    }

    let buf = vec![0u8; DEFAULT_PAGE_SIZE as usize];
    let (buf, _) = async_coordinator.read_page(page_id, buf).await?;

    page_cache.write().put(page_id, buf.clone());
    Ok(buf)
}
