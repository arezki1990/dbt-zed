#![cfg(test)]

use super::*;

#[test]
fn test_cache_creation() {
    let cache = TraversalCache::new(16);
    assert_eq!(cache.capacity(), 16);
}

#[test]
fn test_node_store_new() {
    let header = PersistentHeaderV3::new_v3();
    let db_path = PathBuf::from("/tmp/test.db");
    let store = NodeStore::new(&header, db_path);
    assert_eq!(store.root_page_id_pub(), 0);
}

#[test]
fn test_page_offset_calculation() {
    assert_eq!(NodeStore::page_offset(1), V3_HEADER_SIZE);
    assert_eq!(
        NodeStore::page_offset(2),
        V3_HEADER_SIZE + DEFAULT_PAGE_SIZE
    );
}

#[test]
fn test_constants() {
    assert_eq!(MAX_TREE_HEIGHT, 10);
    assert_eq!(PAGE_CACHE_SIZE, 1024);
}

#[test]
fn test_page_loader_creation() {
    let db_path = PathBuf::from("/tmp/test.db");
    let _ = std::fs::File::create(&db_path).unwrap();
    let file = Arc::new(File::open(&db_path).unwrap());
    let page_size = 4096;

    let loader = PageLoader::new(file.clone(), page_size);
    assert_eq!(loader.page_size(), 4096);
    assert_eq!(loader.header_size(), V3_HEADER_SIZE);

    let loader_default = PageLoader::with_default_page_size(file);
    assert_eq!(loader_default.page_size(), 4096);
}

#[test]
fn test_page_loader_offset_calculation() {
    assert_eq!(PageLoader::page_offset(1), V3_HEADER_SIZE);
    assert_eq!(
        PageLoader::page_offset(2),
        V3_HEADER_SIZE + DEFAULT_PAGE_SIZE
    );
    assert_eq!(PageLoader::page_offset(0), 0);
}

#[test]
fn test_traversal_cache_builder() {
    let builder = TraversalCacheBuilder::new();
    assert!(builder.capacity.is_none());

    let cache = builder.with_capacity(32).build().unwrap();
    assert_eq!(cache.capacity(), 32);
}

#[test]
fn test_traversal_cache_builder_invalid_capacity() {
    let builder = TraversalCacheBuilder::new();
    let result = builder.with_capacity(MAX_CACHE_CAPACITY + 1).build();
    assert!(result.is_err());
}

#[test]
fn test_traversal_cache_builder_default() {
    let cache = TraversalCacheBuilder::default().build().unwrap();
    assert_eq!(cache.capacity(), DEFAULT_CACHE_CAPACITY);
}
