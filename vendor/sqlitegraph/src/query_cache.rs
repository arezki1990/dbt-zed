//! High-level query cache layer for SQLiteGraph.
//!
//! This module provides deterministic, MVCC-aware caching for expensive graph traversal
//! queries. The cache is transparent to callers and lives entirely inside the SQLiteGraph
//! implementation without requiring any API changes.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::backend::BackendDirection;

/// Cache key for BFS queries
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BfsCacheKey {
    pub start: i64,
    pub depth: u32,
}

/// Cache key for k-hop queries
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KHopCacheKey {
    pub start: i64,
    pub depth: u32,
    pub direction: BackendDirection,
}

/// Cache key for filtered k-hop queries
#[derive(Debug, Clone)]
pub struct KHopFilteredCacheKey {
    pub start: i64,
    pub depth: u32,
    pub direction: BackendDirection,
    pub allowed_edge_types: Vec<String>,
}

/// Cache key for shortest path queries
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortestPathCacheKey {
    pub start: i64,
    pub end: i64,
}

/// Enumeration of all supported query cache keys
#[derive(Debug, Clone)]
pub enum QueryCacheKey {
    Bfs(BfsCacheKey),
    KHop(KHopCacheKey),
    KHopFiltered(KHopFilteredCacheKey),
    ShortestPath(ShortestPathCacheKey),
}

/// Cache entry containing query results
#[derive(Debug, Clone)]
pub struct QueryCacheEntry {
    pub result: QueryResult,
    // Note: In a production system, you might want to add timestamps, TTL, etc.
}

/// Enumeration of cached query results
#[derive(Debug, Clone)]
pub enum QueryResult {
    Bfs(Vec<i64>),
    KHop(Vec<i64>),
    ShortestPath(Option<Vec<i64>>),
}

impl QueryCacheKey {
    /// Create a deterministic hash for the cache key
    pub fn hash(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();
        match self {
            QueryCacheKey::Bfs(key) => {
                0u8.hash(&mut hasher);
                key.start.hash(&mut hasher);
                key.depth.hash(&mut hasher);
            }
            QueryCacheKey::KHop(key) => {
                1u8.hash(&mut hasher);
                key.start.hash(&mut hasher);
                key.depth.hash(&mut hasher);
                (match key.direction {
                    BackendDirection::Outgoing => 0u8,
                    BackendDirection::Incoming => 1u8,
                })
                .hash(&mut hasher);
            }
            QueryCacheKey::KHopFiltered(key) => {
                2u8.hash(&mut hasher);
                key.start.hash(&mut hasher);
                key.depth.hash(&mut hasher);
                (match key.direction {
                    BackendDirection::Outgoing => 0u8,
                    BackendDirection::Incoming => 1u8,
                })
                .hash(&mut hasher);
                key.allowed_edge_types.len().hash(&mut hasher);
                for edge_type in &key.allowed_edge_types {
                    edge_type.hash(&mut hasher);
                }
            }
            QueryCacheKey::ShortestPath(key) => {
                3u8.hash(&mut hasher);
                key.start.hash(&mut hasher);
                key.end.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

impl PartialEq for QueryCacheKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (QueryCacheKey::Bfs(a), QueryCacheKey::Bfs(b)) => a == b,
            (QueryCacheKey::KHop(a), QueryCacheKey::KHop(b)) => a == b,
            (QueryCacheKey::KHopFiltered(a), QueryCacheKey::KHopFiltered(b)) => {
                a.start == b.start
                    && a.depth == b.depth
                    && a.direction == b.direction
                    && a.allowed_edge_types == b.allowed_edge_types
            }
            (QueryCacheKey::ShortestPath(a), QueryCacheKey::ShortestPath(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for QueryCacheKey {}

impl Hash for QueryCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash().hash(state);
    }
}

impl PartialEq for KHopFilteredCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.depth == other.depth
            && self.direction == other.direction
            && self.allowed_edge_types == other.allowed_edge_types
    }
}

impl Eq for KHopFilteredCacheKey {}

impl Hash for KHopFilteredCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.depth.hash(state);
        (match self.direction {
            BackendDirection::Outgoing => 0u8,
            BackendDirection::Incoming => 1u8,
        })
        .hash(state);
        self.allowed_edge_types.len().hash(state);
        for edge_type in &self.allowed_edge_types {
            edge_type.hash(state);
        }
    }
}

/// Thread-safe query cache storage
#[derive(Debug)]
pub struct QueryCache {
    cache: Arc<RwLock<HashMap<QueryCacheKey, QueryCacheEntry>>>,
}

impl QueryCache {
    /// Create a new query cache
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a cached result for a BFS query
    pub fn get_bfs(&self, start: i64, depth: u32) -> Option<Vec<i64>> {
        let key = QueryCacheKey::Bfs(BfsCacheKey { start, depth });

        let cache = self.cache.read();

        cache.get(&key).and_then(|entry| match &entry.result {
            QueryResult::Bfs(result) => Some(result.clone()),
            _ => None,
        })
    }

    /// Cache a BFS query result
    pub fn put_bfs(&self, start: i64, depth: u32, result: Vec<i64>) {
        let key = QueryCacheKey::Bfs(BfsCacheKey { start, depth });
        let entry = QueryCacheEntry {
            result: QueryResult::Bfs(result),
        };

        self.cache.write().insert(key, entry);
    }

    /// Get a cached result for a k-hop query
    pub fn get_k_hop(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Option<Vec<i64>> {
        let key = QueryCacheKey::KHop(KHopCacheKey {
            start,
            depth,
            direction,
        });

        let cache = self.cache.read();

        cache.get(&key).and_then(|entry| match &entry.result {
            QueryResult::KHop(result) => Some(result.clone()),
            _ => None,
        })
    }

    /// Cache a k-hop query result
    pub fn put_k_hop(&self, start: i64, depth: u32, direction: BackendDirection, result: Vec<i64>) {
        let key = QueryCacheKey::KHop(KHopCacheKey {
            start,
            depth,
            direction,
        });
        let entry = QueryCacheEntry {
            result: QueryResult::KHop(result),
        };

        self.cache.write().insert(key, entry);
    }

    /// Get a cached result for a filtered k-hop query
    pub fn get_k_hop_filtered(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Option<Vec<i64>> {
        let edge_types = allowed_edge_types.iter().map(|s| s.to_string()).collect();
        let key = QueryCacheKey::KHopFiltered(KHopFilteredCacheKey {
            start,
            depth,
            direction,
            allowed_edge_types: edge_types,
        });

        let cache = self.cache.read();

        cache.get(&key).and_then(|entry| match &entry.result {
            QueryResult::KHop(result) => Some(result.clone()),
            _ => None,
        })
    }

    /// Cache a filtered k-hop query result
    pub fn put_k_hop_filtered(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
        result: Vec<i64>,
    ) {
        let edge_types = allowed_edge_types.iter().map(|s| s.to_string()).collect();
        let key = QueryCacheKey::KHopFiltered(KHopFilteredCacheKey {
            start,
            depth,
            direction,
            allowed_edge_types: edge_types,
        });
        let entry = QueryCacheEntry {
            result: QueryResult::KHop(result),
        };

        self.cache.write().insert(key, entry);
    }

    /// Get a cached result for a shortest path query
    pub fn get_shortest_path(&self, start: i64, end: i64) -> Option<Option<Vec<i64>>> {
        let key = QueryCacheKey::ShortestPath(ShortestPathCacheKey { start, end });

        let cache = self.cache.read();

        cache.get(&key).and_then(|entry| match &entry.result {
            QueryResult::ShortestPath(result) => Some(result.clone()),
            _ => None,
        })
    }

    /// Cache a shortest path query result
    pub fn put_shortest_path(&self, start: i64, end: i64, result: Option<Vec<i64>>) {
        let key = QueryCacheKey::ShortestPath(ShortestPathCacheKey { start, end });
        let entry = QueryCacheEntry {
            result: QueryResult::ShortestPath(result),
        };

        self.cache.write().insert(key, entry);
    }

    /// Clear all cached queries (MVCC invalidation)
    pub fn invalidate_all(&self) {
        self.cache.write().clear();
    }

    /// Get cache statistics for monitoring
    pub fn size(&self) -> usize {
        self.cache.read().len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.read().is_empty()
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for QueryCache {
    fn clone(&self) -> Self {
        Self {
            cache: Arc::clone(&self.cache),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_hashing() {
        // Test that identical keys produce identical hashes
        let key1 = QueryCacheKey::Bfs(BfsCacheKey {
            start: 42,
            depth: 3,
        });
        let key2 = QueryCacheKey::Bfs(BfsCacheKey {
            start: 42,
            depth: 3,
        });
        assert_eq!(key1.hash(), key2.hash());

        // Test that different keys produce different hashes
        let key3 = QueryCacheKey::Bfs(BfsCacheKey {
            start: 42,
            depth: 4,
        });
        assert_ne!(key1.hash(), key3.hash());
    }

    #[test]
    fn test_cache_basic_operations() {
        let cache = QueryCache::new();

        // Test cache miss
        assert_eq!(cache.get_bfs(1, 2), None);

        // Test cache put and hit
        cache.put_bfs(1, 2, vec![3, 4, 5]);
        assert_eq!(cache.get_bfs(1, 2), Some(vec![3, 4, 5]));

        // Test cache size
        assert_eq!(cache.size(), 1);
        assert!(!cache.is_empty());

        // Test cache invalidation
        cache.invalidate_all();
        assert_eq!(cache.get_bfs(1, 2), None);
        assert_eq!(cache.size(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_k_hop_filtered_cache() {
        let cache = QueryCache::new();
        let edge_types = vec!["friend", "colleague"];

        // Test cache miss
        assert_eq!(
            cache.get_k_hop_filtered(1, 2, BackendDirection::Outgoing, &edge_types),
            None
        );

        // Test cache put and hit
        cache.put_k_hop_filtered(1, 2, BackendDirection::Outgoing, &edge_types, vec![3, 4]);
        assert_eq!(
            cache.get_k_hop_filtered(1, 2, BackendDirection::Outgoing, &edge_types),
            Some(vec![3, 4])
        );

        // Test that different edge types don't interfere
        assert_eq!(
            cache.get_k_hop_filtered(1, 2, BackendDirection::Outgoing, &["enemy"]),
            None
        );
    }

    #[test]
    fn test_shortest_path_cache() {
        let cache = QueryCache::new();

        // Test caching None result
        cache.put_shortest_path(1, 5, None);
        assert_eq!(cache.get_shortest_path(1, 5), Some(None));

        // Test caching Some result
        cache.put_shortest_path(1, 3, Some(vec![1, 2, 3]));
        assert_eq!(cache.get_shortest_path(1, 3), Some(Some(vec![1, 2, 3])));

        // Test cache size
        assert_eq!(cache.size(), 2);
    }
}
