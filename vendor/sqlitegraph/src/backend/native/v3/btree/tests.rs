use super::*;
use crate::backend::native::v3::header::PersistentHeaderV3;

fn create_test_allocator() -> Arc<RwLock<PageAllocator>> {
    let header = PersistentHeaderV3::new_v3();
    Arc::new(RwLock::new(PageAllocator::new(&header)))
}

#[test]
fn test_btree_manager_new() {
    let allocator = create_test_allocator();
    let manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    assert_eq!(manager.root_page_id(), EMPTY_TREE_ROOT);
    assert_eq!(manager.tree_height(), 0);
    assert!(manager.is_empty());
}

#[test]
fn test_btree_manager_with_root() {
    let allocator = create_test_allocator();
    let manager = BTreeManager::with_root(allocator, None, 1, 1, None::<PathBuf>);

    assert_eq!(manager.root_page_id(), 1);
    assert_eq!(manager.tree_height(), 1);
    assert!(!manager.is_empty());
}

#[test]
fn test_insert_into_empty_tree() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let result = manager.insert(1, 100);
    assert!(result.is_ok());

    assert!(!manager.is_empty());
    assert_eq!(manager.tree_height(), 1);
    assert!(manager.root_page_id() != EMPTY_TREE_ROOT);
}

#[test]
fn test_lookup_empty_tree() {
    let allocator = create_test_allocator();
    let manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let result = manager.lookup(1);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_insert_and_lookup_single() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    manager.insert(42, 100).unwrap();

    let result = manager.lookup(42);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Some(100));

    let result = manager.lookup(99);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_insert_and_lookup_multiple() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    for i in 1..=10 {
        manager.insert(i, i as u64 * 100).unwrap();
    }

    for i in 1..=10 {
        let result = manager.lookup(i);
        assert!(result.is_ok(), "Failed to lookup key {}", i);
        assert_eq!(
            result.unwrap(),
            Some(i as u64 * 100),
            "Wrong value for key {}",
            i
        );
    }

    let result = manager.lookup(999);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_update_existing_key() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    manager.insert(1, 100).unwrap();
    manager.insert(1, 200).unwrap();

    let result = manager.lookup(1);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Some(200));
}

#[test]
fn test_delete_existing_key() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    manager.insert(1, 100).unwrap();
    let deleted = manager.delete(1).unwrap();

    assert!(deleted);

    let result = manager.lookup(1);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_delete_nonexistent_key() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let deleted = manager.delete(999).unwrap();
    assert!(!deleted);
}

#[test]
fn test_delete_from_empty_tree() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let deleted = manager.delete(1).unwrap();
    assert!(!deleted);
}

#[test]
fn test_cache_stats() {
    let allocator = create_test_allocator();
    let manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let (len, capacity) = manager.cache_stats();
    assert_eq!(len, 0);
    assert_eq!(capacity, 1024);
}

#[test]
fn test_clear_cache() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    manager.insert(1, 100).unwrap();
    manager.clear_cache();

    let (len, _) = manager.cache_stats();
    assert_eq!(len, 0);
}

#[test]
fn test_write_batch_basic() {
    let allocator = create_test_allocator();
    let manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let mut batch = manager.create_write_batch();
    let page = IndexPage::new_leaf(1);
    manager.stage_page_to_batch(&mut batch, page).unwrap();

    assert_eq!(batch.len(), 1);
}

#[test]
fn test_insert_simple_to_batch_empty_tree() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let mut batch = manager.create_write_batch();
    let result = manager.insert_simple_to_batch(&mut batch, 1, 100).unwrap();

    assert!(result, "Insert should succeed for empty tree");
    assert_eq!(batch.len(), 1);
    assert!(!manager.is_empty(), "Manager should now have root");
}

#[test]
fn test_insert_simple_to_batch_single() {
    let allocator = create_test_allocator();
    let mut manager = BTreeManager::new(allocator, None, None::<PathBuf>);

    let mut batch = manager.create_write_batch();
    let result = manager.insert_simple_to_batch(&mut batch, 1, 100).unwrap();
    assert!(result, "Insert should succeed for empty tree");
    assert_eq!(batch.len(), 1);
}
