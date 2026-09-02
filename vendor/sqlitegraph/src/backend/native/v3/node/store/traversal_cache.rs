use super::{NativeBackendError, NativeResult, NodePage};
use std::collections::HashMap;
use std::sync::Arc;

/// Default capacity for TraversalCache
/// Default of 64 pages was determined by cache capacity sweep benchmark
pub const DEFAULT_CACHE_CAPACITY: usize = 64;

/// Maximum capacity for TraversalCache
pub const MAX_CACHE_CAPACITY: usize = 256;

/// Minimum capacity for TraversalCache
pub const MIN_CACHE_CAPACITY: usize = 1;

/// LRU cache for NodePage instances during graph traversal
#[derive(Debug, Clone)]
pub struct TraversalCache {
    cache: HashMap<u64, Arc<NodePage>>,
    access_order: Vec<u64>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

impl TraversalCache {
    pub fn new(capacity: usize) -> Self {
        assert!((MIN_CACHE_CAPACITY..=MAX_CACHE_CAPACITY).contains(&capacity));
        Self {
            cache: HashMap::with_capacity(capacity),
            access_order: Vec::with_capacity(capacity),
            capacity,
            hits: 0,
            misses: 0,
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CACHE_CAPACITY)
    }

    pub fn get(&mut self, page_id: u64) -> Option<Arc<NodePage>> {
        if let Some(page) = self.cache.remove(&page_id) {
            self.access_order.retain(|&id| id != page_id);
            self.access_order.push(page_id);
            self.cache.insert(page_id, page.clone());
            self.hits += 1;
            Some(page)
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, page_id: u64, page: Arc<NodePage>) {
        if self.cache.contains_key(&page_id) {
            self.access_order.retain(|&id| id != page_id);
        }
        while self.cache.len() >= self.capacity {
            if let Some(oldest_id) = self.access_order.first() {
                self.cache.remove(oldest_id);
                self.access_order.remove(0);
            } else {
                break;
            }
        }
        self.access_order.push(page_id);
        self.cache.insert(page_id, page);
    }

    pub fn invalidate(&mut self, page_id: u64) -> bool {
        let was_present = self.cache.remove(&page_id).is_some();
        self.access_order.retain(|&id| id != page_id);
        was_present
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.access_order.clear();
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn contains(&self, page_id: &u64) -> bool {
        self.cache.contains_key(page_id)
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }

    pub fn reset_stats(&mut self) {
        self.hits = 0;
        self.misses = 0;
    }
}

impl Default for TraversalCache {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

/// Builder for creating TraversalCache with custom configuration
pub struct TraversalCacheBuilder {
    pub(crate) capacity: Option<usize>,
}

impl TraversalCacheBuilder {
    pub fn new() -> Self {
        Self { capacity: None }
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    pub fn build(self) -> NativeResult<TraversalCache> {
        let capacity = self.capacity.unwrap_or(DEFAULT_CACHE_CAPACITY);
        if !(MIN_CACHE_CAPACITY..=MAX_CACHE_CAPACITY).contains(&capacity) {
            return Err(NativeBackendError::InvalidParameter {
                context: "TraversalCache capacity".to_string(),
                source: None,
            });
        }
        Ok(TraversalCache::new(capacity))
    }
}

impl Default for TraversalCacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}
