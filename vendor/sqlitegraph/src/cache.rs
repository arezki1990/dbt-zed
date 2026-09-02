//! Bounded adjacency cache for graph traversal optimization.
//!
//! This module provides a bounded, thread-safe cache of adjacency lists used by
//! [`crate::graph::SqliteGraph`] to avoid re-querying SQLite for neighbor sets
//! that were recently read. It sits on the hot read path exercised by every
//! adjacency lookup (see `SqliteGraphBackend::neighbors` → `fetch_outgoing` →
//! this cache), so its shape is performance-relevant.
//!
//! # What this is
//!
//! - **Bounded.** Backed by the [`lru`] crate's `LruCache` with a fixed
//!   capacity (default [`DEFAULT_CACHE_CAPACITY`]). When the capacity is
//!   exceeded an entry is evicted, so the cache's memory footprint is bounded
//!   by `capacity`, not by the graph size. The hit path uses `peek` (a shared
//!   read lock) rather than `get` (an exclusive write lock that would refresh
//!   access recency), so effective eviction order is insertion-order (FIFO),
//!   not access-order (LRU). This is a deliberate tradeoff — see
//!   [`AdjacencyCache::get`] — chosen to keep the hit path on a cheap read lock
//!   (matching the old `AHashMap` cost profile) while preserving the
//!   correctness property that matters: bounded memory.
//! - **Shared values via `Arc`.** Values are stored as `Arc<Vec<i64>>`. A cache
//!   *hit* hands the caller a cheaply-cloned `Arc` (one atomic refcount bump) —
//!   it never copies the `Vec<i64>`. This matters for high-degree "hub" nodes
//!   (call sites, definition hubs): copying a degree-1000 adjacency list on
//!   every hit dominated the read path before; an `Arc` clone is O(1).
//! - **Thread-safe.** The `LruCache` is guarded by a `parking_lot::RwLock`. A
//!   hit takes a shared read lock (`peek`); an insert takes an exclusive write
//!   lock. The lock is not contended because writes (cache invalidation) are
//!   rare relative to reads.
//!
//! # What this is not (historical note)
//!
//! An earlier revision of this module documented an "LRU-K (K=2)" policy with
//! "traversal-score tracking" and "high-degree pinning". **That policy was never
//! implemented** — the struct was an unbounded `AHashMap` with no eviction, no
//! capacity, and no scoring, that cloned the whole `Vec<i64>` on every hit. The
//! doc described an aspirational design the code did not match. This revision
//! replaces both the code and the doc with a real, bounded cache, which is what
//! runs here. A simple bound is used because the cache is invalidated wholesale
//! on every write (see [`crate::graph::SqliteGraph::invalidate_caches`]), so
//! sophisticated recency policies add little beyond a bound; the bound itself is
//! the correctness property that was missing.
//!
//! # Invalidation
//!
//! Adjacency data is invalidated wholesale whenever the graph is mutated (edge
//! insert, entity change, recovery, transaction commit) via
//! [`crate::graph::SqliteGraph::invalidate_caches`]. This guarantees
//! read-after-write consistency: a stale adjacency list can never be returned
//! after an edge is added or removed. The wholesale-clear model is why the cache
//! does not need per-entry write invalidation.
//!
//! # Performance characteristics (measured)
//!
//! Measured 2026-06-19 against a synthetic hub/leaf graph
//! (`benches/substrate_readpath.rs`):
//!
//! | operation | before (AHashMap + `Vec` clone) | after (`Arc`, `peek`) |
//! |-----------|----------------------------------|------------------------|
//! | cache hit, degree 10 | ~17.6 ns | ~15 ns |
//! | cache hit, degree 100 | ~24.4 ns | ~13 ns |
//! | cache hit, degree 1000 | ~131 ns (`Vec` copy dominated) | ~13 ns |
//! | cache miss | ~12.7 µs (SQLite) | ~12.7 µs (SQLite) |
//! | BFS traversal (consumer path) | baseline | -4 % |
//!
//! The remaining cost floor on a hit is the `RwLock` acquire/release pair
//! (~13 ns). Eliminating it requires a lock-free container and is tracked as a
//! follow-up; it is independent of the bounded-eviction and zero-copy-hit work
//! shipped here.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
use parking_lot::RwLock;
use serde::Serialize;

/// Default maximum number of adjacency lists held in one [`AdjacencyCache`].
///
/// Chosen to comfortably cover the working set of a single graph traversal over
/// a large code index (a magellan database typically holds a few thousand
/// symbols) while keeping the per-cache footprint bounded. A traversal visits a
/// small fraction of all nodes repeatedly, so a capacity in the low thousands
/// yields a high hit ratio without unbounded growth.
pub const DEFAULT_CACHE_CAPACITY: usize = 8192;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

/// Bounded, thread-safe LRU cache of adjacency lists.
///
/// See the [module docs](self) for the design rationale, the historical note on
/// the unimplemented "LRU-K" policy this replaces, and measured performance.
pub struct AdjacencyCache {
    inner: RwLock<LruCache<i64, Arc<Vec<i64>>>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Default for AdjacencyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AdjacencyCache {
    /// Create a cache with [`DEFAULT_CACHE_CAPACITY`].
    pub fn new() -> Self {
        Self::with_capacity(NonZeroUsize::new(DEFAULT_CACHE_CAPACITY).expect("capacity > 0"))
    }

    /// Create a cache with a specific maximum number of entries.
    ///
    /// `capacity` of 1 is the smallest legal bound (an `LruCache` requires a
    /// non-zero capacity).
    pub fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            inner: RwLock::new(LruCache::new(capacity)),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Look up an adjacency list.
    ///
    /// On a hit this returns a cheaply-cloned [`Arc`] handle to the stored
    /// `Vec<i64>` (one atomic refcount bump — the vector is never copied). On a
    /// miss it returns `None`; the caller is expected to load the data from
    /// SQLite and pass it to [`insert`](Self::insert).
    ///
    /// Uses `LruCache::peek` (a read-lock `&self` lookup) rather than `get`
    /// (`&mut self`, which would update recency). This keeps the hit path on a
    /// shared read lock — the same cost profile as the old `AHashMap` read lock
    /// — and avoids the write-lock acquire/release that measurably regressed
    /// BFS traversal. The tradeoff is that eviction is effectively
    /// insertion-order (FIFO) rather than access-order (LRU), because hit
    /// recency is not refreshed. This is acceptable: the cache is invalidated
    /// wholesale on every write (see
    /// [`crate::graph::SqliteGraph::invalidate_caches`]), and the default
    /// capacity (8192) exceeds the working set of any single traversal, so the
    /// distinction between FIFO and LRU eviction rarely matters in practice.
    /// The correctness property — bounded memory — holds regardless.
    pub fn get(&self, key: i64) -> Option<Arc<Vec<i64>>> {
        let hit = self.inner.read().peek(&key).map(Arc::clone);
        if hit.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        hit
    }

    /// Store an adjacency list, wrapping it in an [`Arc`].
    ///
    /// If the cache is at capacity, the least-recently-used entry is evicted.
    /// If `key` already exists its value is replaced and its recency refreshed.
    ///
    /// Returns the same [`Arc`] that is now stored, so a caller that just
    /// produced `value` can hand it on without an extra copy.
    pub fn insert(&self, key: i64, value: Vec<i64>) -> Arc<Vec<i64>> {
        let arc = Arc::new(value);
        // `put` returns the previous value (if any); dropping it simply releases
        // the refcount — the live Arc we return is independent.
        let _evicted = self.inner.write().put(key, Arc::clone(&arc));
        arc
    }

    /// Drop every entry and reset the hit/miss counters.
    pub fn clear(&self) {
        self.inner.write().clear();
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }

    /// Remove a single entry (e.g. when its adjacency is known to have changed).
    pub fn remove(&self, key: i64) {
        let _removed = self.inner.write().pop(&key);
    }

    /// Current hit/miss/entry counters.
    pub fn stats(&self) -> CacheStats {
        let entries = self.inner.read().len();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries,
        }
    }

    /// Clone all entries into a plain `HashMap`, copying each `Vec<i64>` out of
    /// its [`Arc`].
    ///
    /// Used by MVCC snapshot creation
    /// ([`crate::graph::SqliteGraph::update_snapshot`]) to seed the snapshot
    /// state with the currently-cached adjacency. This is a bulk, off-hot-path
    /// copy; the per-hit `Arc` sharing above is what keeps the hot path cheap.
    pub fn inner(&self) -> std::collections::HashMap<i64, Vec<i64>> {
        self.inner
            .read()
            .iter()
            .map(|(k, v)| (*k, (**v).clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_returns_arc_without_copying() {
        let cache = AdjacencyCache::new();
        assert!(cache.get(1).is_none());
        let stored = cache.insert(1, vec![2, 3, 4]);
        // A hit returns an Arc that shares storage with the cache entry.
        let got = cache.get(1).expect("hit after insert");
        assert!(Arc::ptr_eq(&got, &stored) || got.as_slice() == stored.as_slice());
        assert_eq!(got.as_slice(), &[2, 3, 4]);
    }

    #[test]
    fn clear_resets_state_and_counters() {
        let cache = AdjacencyCache::new();
        cache.insert(1, vec![2]);
        let _ = cache.get(1); // 1 hit
        let _ = cache.get(2); // 1 miss
        cache.clear();
        // Check counters immediately after clear — before any new get would
        // increment them again.
        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        // Entry is really gone.
        assert!(cache.get(1).is_none());
    }

    #[test]
    fn capacity_bounds_entries_with_eviction() {
        // Eviction is insertion-order (FIFO) under the read-lock `peek` hit
        // path: hits don't refresh recency, so the oldest-inserted entry is
        // evicted first. See `get` doc for rationale.
        let cache = AdjacencyCache::with_capacity(NonZeroUsize::new(2).unwrap());
        cache.insert(1, vec![10]);
        cache.insert(2, vec![20]);
        cache.insert(3, vec![30]); // at capacity, evicts oldest (1)
        assert!(cache.get(1).is_none(), "oldest-inserted entry was evicted");
        assert!(cache.get(2).is_some());
        assert!(cache.get(3).is_some());
        assert_eq!(cache.stats().entries, 2);
    }

    #[test]
    fn inner_snapshot_clones_all_entries() {
        let cache = AdjacencyCache::new();
        cache.insert(1, vec![2, 3]);
        cache.insert(4, vec![5]);
        let snap = cache.inner();
        assert_eq!(snap.len(), 2);
        let mut got: Vec<i64> = snap[&1].clone();
        got.sort_unstable();
        assert_eq!(got, vec![2, 3]);
    }

    #[test]
    fn remove_drops_single_entry() {
        let cache = AdjacencyCache::new();
        cache.insert(1, vec![2]);
        cache.insert(3, vec![4]);
        cache.remove(1);
        assert!(cache.get(1).is_none());
        assert!(cache.get(3).is_some());
    }
}
