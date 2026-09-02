//! MVCC-lite snapshot system for SQLiteGraph
//!
//! Provides deterministic, read-only snapshots using the S3 Hybrid approach:
//! - SnapshotState stores immutable cloned HashMaps (not Arc-share)
//! - ArcSwap provides lock-free atomic updates
//! - Read-only SQLite connections ensure database consistency
//! - Deterministic behavior with repeatable results
//!
//! # Temporal version chain
//!
//! In addition to the single live snapshot, [`SnapshotManager`] can retain a
//! **bounded version history** for temporal (`as_of`) queries. By default the
//! history is empty — zero overhead. Call [`SnapshotManager::checkpoint`] to
//! capture the current live state as a numbered version; call
//! [`SnapshotManager::as_of`] to retrieve it later. The chain is bounded by a
//! configurable `max_history` (default 64): when full, the oldest version is
//! evicted.

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, Result as SqliteResult};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

pub type NodeId = i64;

/// Default maximum number of versions retained in the history chain.
pub const DEFAULT_MAX_HISTORY: usize = 64;

/// Immutable snapshot state containing cloned adjacency data
///
/// This structure stores complete copies of adjacency maps to ensure
/// true isolation - snapshots are unaffected by subsequent writes.
///
/// # Design boundary: adjacency-only
///
/// The version chain stores **untyped adjacency only** (`HashMap<i64, Vec<i64>>`)
/// — node IDs and their edge lists, with no entity properties (kind/name/data),
/// no edge types, and no key-value pairs. This is by design, not a gap:
/// capturing the full entity/edge/KV payload per version would multiply memory
/// cost roughly by the number of versions, defeating the bounded-retention goal.
///
/// As a consequence, historical reads (`SnapshotId(N)` where N > 0) serve only
/// the two adjacency-only operations — [`SqliteGraphBackend::neighbors`] and
/// [`SqliteGraphBackend::bfs`] — from this chain. Every other operation
/// (`get_node`, `shortest_path`, `node_degree`, `k_hop`, `chain_query`,
/// `pattern_search`, `kv_get`, the typed-edge `query_nodes_by_*`, etc.) needs
/// property/typed-edge data that the chain does not carry, so they **reject
/// historical snapshots by design** (see
/// [`SqliteGraphBackend::require_live`](crate::backend::sqlite::impl_::SqliteGraphBackend))
/// rather than returning incomplete or wrong data. Use `SnapshotId::current()`
/// for those reads.
#[derive(Debug, Clone)]
pub struct SnapshotState {
    /// Immutable copy of outgoing adjacency map
    pub outgoing: HashMap<NodeId, Vec<NodeId>>,
    /// Immutable copy of incoming adjacency map  
    pub incoming: HashMap<NodeId, Vec<NodeId>>,
    /// Snapshot creation timestamp
    pub created_at: std::time::SystemTime,
}

impl SnapshotState {
    /// Create a new snapshot state by cloning adjacency maps
    ///
    /// # Arguments
    /// * `outgoing` - Current outgoing adjacency map to clone
    /// * `incoming` - Current incoming adjacency map to clone
    ///
    /// # Returns
    /// New SnapshotState with immutable cloned data
    pub fn new(
        outgoing: &HashMap<NodeId, Vec<NodeId>>,
        incoming: &HashMap<NodeId, Vec<NodeId>>,
    ) -> Self {
        Self {
            // Use .clone() to create deep copies, not Arc::clone()
            outgoing: outgoing.clone(),
            incoming: incoming.clone(),
            created_at: std::time::SystemTime::now(),
        }
    }

    /// Get the number of nodes in this snapshot
    pub fn node_count(&self) -> usize {
        self.outgoing.len()
    }

    /// Get the number of edges in this snapshot
    pub fn edge_count(&self) -> usize {
        self.outgoing.values().map(|adj| adj.len()).sum()
    }

    /// Check if a node exists in this snapshot
    pub fn contains_node(&self, node_id: NodeId) -> bool {
        self.outgoing.contains_key(&node_id)
    }

    /// Get outgoing neighbors for a node in this snapshot
    pub fn get_outgoing(&self, node_id: NodeId) -> Option<&Vec<NodeId>> {
        self.outgoing.get(&node_id)
    }

    /// Get incoming neighbors for a node in this snapshot
    pub fn get_incoming(&self, node_id: NodeId) -> Option<&Vec<NodeId>> {
        self.incoming.get(&node_id)
    }
}

/// A versioned point-in-time snapshot.
///
/// Produced by [`SnapshotManager::checkpoint`]. The `version` is a monotonic
/// counter assigned at checkpoint time; `created_at` is the wall-clock
/// timestamp. Stored in the bounded history chain inside [`SnapshotManager`].
#[derive(Debug, Clone)]
pub struct VersionedSnapshot {
    /// Monotonic version number assigned at checkpoint time (starts at 1).
    pub version: u64,
    /// Wall-clock timestamp when this version was captured.
    pub created_at: SystemTime,
    /// The immutable adjacency data for this version.
    pub state: Arc<SnapshotState>,
}

/// MVCC snapshot manager using ArcSwap for atomic updates
///
/// Provides lock-free snapshot acquisition and deterministic behavior.
/// Snapshots are completely isolated from write operations.
///
/// # Memory Ordering Guarantees
///
/// This implementation relies on ArcSwap's memory ordering guarantees:
/// - **ArcSwap::load()**: Uses Acquire ordering, ensuring all writes before store are visible
/// - **ArcSwap::store()**: Uses Release ordering, ensuring all writes complete before publication
/// - This provides proper happens-before relationship between writers and readers
///
/// # Thread Safety
///
/// The SnapshotManager is thread-safe and can be shared across threads via Arc:
/// - Multiple readers can acquire snapshots concurrently without blocking
/// - Writers can update state concurrently with readers
/// - No locks or mutexes required (lock-free)
/// - No TOCTOU (time-of-check-time-of-use) issues due to atomic pointer swap
///
/// # Invariants
///
/// 1. **Snapshot State Immutability**: Once a SnapshotState is created, it never changes
/// 2. **Atomic Publication**: State updates are atomic - readers see either old or new state, never partial
/// 3. **Arc Reference Counting**: Each snapshot maintains proper Arc reference counts
/// 4. **No Mutable Aliasing**: Arc<SnapshotState> ensures no mutable access to snapshot data
#[derive(Debug)]
pub struct SnapshotManager {
    /// Atomic reference to current snapshot state
    ///
    /// ArcSwap provides lock-free atomic updates with proper memory ordering:
    /// - Load uses Acquire ordering
    /// - Store uses Release ordering
    /// - Guarantees happens-before relationship
    current: ArcSwap<SnapshotState>,
    /// Bounded version history for temporal queries.
    ///
    /// Empty by default (no retention overhead). Populated by [`checkpoint`].
    /// Guarded by a `parking_lot::Mutex` because checkpoint writes are rare
    /// relative to reads and `as_of` does a read-lock + binary search.
    ///
    /// [`checkpoint`]: SnapshotManager::checkpoint
    history: Mutex<VecDeque<VersionedSnapshot>>,
    /// Maximum versions to retain before evicting the oldest.
    max_history: usize,
    /// Monotonic counter for the next version number (starts at 1).
    next_version: AtomicU64,
}

impl SnapshotManager {
    /// Create a new snapshot manager with empty initial state
    pub fn new() -> Self {
        let initial_state = SnapshotState::new(&HashMap::new(), &HashMap::new());
        Self {
            current: ArcSwap::new(Arc::new(initial_state)),
            history: Mutex::new(VecDeque::new()),
            max_history: DEFAULT_MAX_HISTORY,
            next_version: AtomicU64::new(1),
        }
    }

    /// Create a new snapshot manager with initial state
    pub fn with_state(
        outgoing: &HashMap<NodeId, Vec<NodeId>>,
        incoming: &HashMap<NodeId, Vec<NodeId>>,
    ) -> Self {
        let initial_state = SnapshotState::new(outgoing, incoming);
        Self {
            current: ArcSwap::new(Arc::new(initial_state)),
            history: Mutex::new(VecDeque::new()),
            max_history: DEFAULT_MAX_HISTORY,
            next_version: AtomicU64::new(1),
        }
    }

    /// Create a snapshot manager with a custom maximum history length.
    ///
    /// When the history chain exceeds `max_history` versions, the oldest is
    /// evicted on the next [`checkpoint`](Self::checkpoint).
    pub fn with_max_history(max_history: usize) -> Self {
        let initial_state = SnapshotState::new(&HashMap::new(), &HashMap::new());
        Self {
            current: ArcSwap::new(Arc::new(initial_state)),
            history: Mutex::new(VecDeque::new()),
            max_history: max_history.max(1),
            next_version: AtomicU64::new(1),
        }
    }

    /// Atomically update the snapshot state
    ///
    /// This method creates a new immutable snapshot state and publishes it atomically
    /// using ArcSwap's store operation with Release memory ordering.
    ///
    /// # Memory Ordering
    ///
    /// - All writes to the new SnapshotState complete **before** the store
    /// - The store operation uses Release ordering
    /// - Readers with Acquire ordering see the complete, consistent state
    ///
    /// # Thread Safety
    ///
    /// This method is thread-safe and can be called concurrently with snapshot acquisition:
    /// - Multiple writers can call this (though serialization happens at ArcSwap level)
    /// - Readers continue to see old state until this store completes
    /// - No partial updates visible to readers (atomic pointer swap)
    ///
    /// # Arguments
    /// * `outgoing` - New outgoing adjacency map to clone
    /// * `incoming` - New incoming adjacency map to clone
    ///
    /// # Invariants Preserved
    ///
    /// 1. The new SnapshotState is fully constructed before store
    /// 2. No mutable references to the state exist after publication
    /// 3. Arc reference count starts at 1 (this ArcSwap reference)
    pub fn update_snapshot(
        &self,
        outgoing: &HashMap<NodeId, Vec<NodeId>>,
        incoming: &HashMap<NodeId, Vec<NodeId>>,
    ) {
        // Create new state with cloned HashMaps
        // This is a deep copy, ensuring complete isolation
        let new_state = SnapshotState::new(outgoing, incoming);

        // Verify invariants before publication
        // These checks run in debug mode to catch bugs early
        #[cfg(debug_assertions)]
        {
            // Verify state is fully constructed
            assert_eq!(
                new_state.node_count(),
                outgoing.len(),
                "Snapshot state node count mismatch"
            );
            assert_eq!(
                new_state.edge_count(),
                outgoing.values().map(|v| v.len()).sum::<usize>(),
                "Snapshot state edge count mismatch"
            );
        }

        // Atomic publication with Release ordering
        // All writes to new_state happen-before this store
        self.current.store(Arc::new(new_state));
    }

    /// Acquire a deterministic snapshot of current state
    ///
    /// This method atomically loads the current snapshot state using ArcSwap's
    /// load operation with Acquire memory ordering.
    ///
    /// # Memory Ordering
    ///
    /// - The load operation uses Acquire ordering
    /// - All writes from the corresponding store are visible
    /// - Provides happens-before relationship with writer
    ///
    /// # Thread Safety
    ///
    /// This method is thread-safe and can be called concurrently with state updates:
    /// - Multiple readers can acquire snapshots concurrently without blocking
    /// - Acquiring a snapshot never blocks a writer
    /// - The returned Arc<SnapshotState> provides immutable access
    ///
    /// # Returns
    ///
    /// `Arc<SnapshotState>` containing immutable snapshot data.
    /// The Arc ensures the snapshot data remains valid as long as needed.
    ///
    /// # Invariants Guaranteed
    ///
    /// 1. The returned snapshot state is immutable (no mutable access possible)
    /// 2. The snapshot is complete and consistent (no torn reads)
    /// 3. Arc reference count is >= 1 during snapshot lifetime
    /// 4. No TOCTOU issues (atomic load, not check-then-use)
    pub fn acquire_snapshot(&self) -> Arc<SnapshotState> {
        // Atomic load with Acquire ordering
        // This sees either the old state or the new state, never partial
        let state = self.current.load();

        // Clone the Arc (increases reference count, atomic operation)
        // This is cheap (just an atomic increment) and doesn't copy the data
        let snapshot = Arc::clone(&state);

        #[cfg(debug_assertions)]
        {
            // Verify snapshot consistency
            // These checks validate that we got a complete, consistent snapshot
            let node_count = snapshot.node_count();
            let edge_count = snapshot.edge_count();

            // Verify internal consistency
            // (node_count + edge_count should be reasonable for the state)
            assert!(node_count <= 10_000_000, "Suspiciously large node count");
            assert!(edge_count <= 100_000_000, "Suspiciously large edge count");
        }

        snapshot
    }

    /// Get current snapshot state without cloning (for internal use)
    pub fn current_snapshot(&self) -> Arc<SnapshotState> {
        self.current.load().clone()
    }

    // ── Temporal version chain ────────────────────────────────────────────

    /// Capture the current live state as a numbered version in the history chain.
    ///
    /// Returns the assigned version number. This is the explicit "commit"
    /// operation for temporal tracking — by default nothing is retained, so
    /// `checkpoint()` is the opt-in that makes [`as_of`](Self::as_of)
    /// meaningful.
    ///
    /// If the chain is at capacity (`max_history`), the oldest version is
    /// evicted (FIFO) before the new one is pushed.
    ///
    /// # Thread Safety
    ///
    /// Takes a short-lived exclusive lock on the history deque. Concurrent
    /// `as_of` queries are blocked only for the duration of the push.
    pub fn checkpoint(&self) -> u64 {
        let state = self.current.load();
        let version = self.next_version.fetch_add(1, Ordering::SeqCst);
        let snapshot = VersionedSnapshot {
            version,
            created_at: SystemTime::now(),
            state: Arc::clone(&state),
        };
        let mut history = self.history.lock();
        if history.len() >= self.max_history {
            history.pop_front();
        }
        history.push_back(snapshot);
        version
    }

    /// Look up a specific version in the history chain.
    ///
    /// Returns the [`VersionedSnapshot`] whose version number equals `version`,
    /// or `None` if no such version was retained (either never checkpointed or
    /// evicted by the bounded-retention policy).
    ///
    /// Performs a binary search — O(log N) where N = current chain length.
    pub fn as_of(&self, version: u64) -> Option<VersionedSnapshot> {
        let history = self.history.lock();
        // Versions are monotonically increasing and contiguous in the deque,
        // but may have gaps at the front if evicted. Binary search by version.
        history
            .binary_search_by_key(&version, |v| v.version)
            .ok()
            .map(|i| history[i].clone())
    }

    /// Look up the most recent version at or before `timestamp`.
    ///
    /// Returns the [`VersionedSnapshot`] whose `created_at` is the latest
    /// timestamp ≤ `timestamp`, or `None` if no version was captured at or
    /// before that time.
    ///
    /// Performs a binary search by timestamp — O(log N).
    pub fn as_of_at(&self, timestamp: SystemTime) -> Option<VersionedSnapshot> {
        let history = self.history.lock();
        if history.is_empty() {
            return None;
        }
        // partition_point returns the count of elements whose created_at <= timestamp.
        // 0 means no version qualifies → None. Otherwise the rightmost qualifying
        // element is at idx - 1.
        let idx = history.partition_point(|v| v.created_at <= timestamp);
        if idx == 0 {
            return None;
        }
        Some(history[idx - 1].clone())
    }

    /// Number of versions currently retained in the history chain.
    pub fn version_count(&self) -> usize {
        self.history.lock().len()
    }

    /// The oldest version number still retained, or `None` if the chain is empty.
    pub fn oldest_version(&self) -> Option<u64> {
        self.history.lock().front().map(|v| v.version)
    }

    /// The newest version number still retained, or `None` if the chain is empty.
    pub fn newest_version(&self) -> Option<u64> {
        self.history.lock().back().map(|v| v.version)
    }

    /// Iterate over all retained versions (oldest first).
    pub fn versions(&self) -> Vec<VersionedSnapshot> {
        self.history.lock().iter().cloned().collect()
    }

    /// Clear the version history (keeps the live state intact).
    pub fn clear_history(&self) {
        self.history.lock().clear();
    }
}

impl Default for SnapshotManager {
    fn default() -> Self {
        Self::new()
    }
}

/// MVCC-lite read-only snapshot for graph data isolation.
///
/// Provides safe, read-only access to a point-in-time view of the graph
/// with its own SQLite connection to ensure database consistency.
pub struct GraphSnapshot {
    /// Immutable snapshot state
    state: Arc<SnapshotState>,
    /// Read-only SQLite connection for database queries
    conn: Connection,
}

impl GraphSnapshot {
    /// Create a new graph snapshot
    ///
    /// # Arguments
    /// * `state` - Immutable snapshot state
    /// * `db_path` - Path to SQLite database
    ///
    /// # Returns
    /// Result containing GraphSnapshot or error
    pub fn new(state: Arc<SnapshotState>, db_path: &str) -> SqliteResult<Self> {
        // Create read-only connection to ensure database consistency
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        Ok(Self { state, conn })
    }

    /// Get the snapshot state
    pub fn state(&self) -> &Arc<SnapshotState> {
        &self.state
    }

    /// Get the read-only database connection
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get the number of nodes in this snapshot
    pub fn node_count(&self) -> usize {
        self.state.node_count()
    }

    /// Get the number of edges in this snapshot
    pub fn edge_count(&self) -> usize {
        self.state.edge_count()
    }

    /// Check if a node exists in this snapshot
    pub fn contains_node(&self, node_id: NodeId) -> bool {
        self.state.contains_node(node_id)
    }

    /// Get outgoing neighbors for a node in this snapshot
    pub fn get_outgoing(&self, node_id: NodeId) -> Option<&Vec<NodeId>> {
        self.state.get_outgoing(node_id)
    }

    /// Get incoming neighbors for a node in this snapshot
    pub fn get_incoming(&self, node_id: NodeId) -> Option<&Vec<NodeId>> {
        self.state.get_incoming(node_id)
    }

    /// Get snapshot creation timestamp
    pub fn created_at(&self) -> std::time::SystemTime {
        self.state.created_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_state_creation() {
        let mut outgoing = HashMap::new();
        let mut incoming = HashMap::new();

        outgoing.insert(1, vec![2, 3]);
        incoming.insert(1, vec![]);

        let state = SnapshotState::new(&outgoing, &incoming);

        assert_eq!(state.node_count(), 1);
        assert_eq!(state.edge_count(), 2);
        assert!(state.contains_node(1));
        assert!(!state.contains_node(2));
    }

    #[test]
    fn test_snapshot_manager() {
        let mut outgoing = HashMap::new();
        let mut incoming = HashMap::new();

        outgoing.insert(1, vec![2]);
        incoming.insert(1, vec![]);

        let manager = SnapshotManager::with_state(&outgoing, &incoming);

        let snapshot = manager.acquire_snapshot();
        assert_eq!(snapshot.node_count(), 1);
        assert!(snapshot.contains_node(1));

        // Update state
        outgoing.insert(2, vec![]);
        incoming.insert(2, vec![1]);
        manager.update_snapshot(&outgoing, &incoming);

        let new_snapshot = manager.acquire_snapshot();
        assert_eq!(new_snapshot.node_count(), 2);

        // Original snapshot should be unchanged
        assert_eq!(snapshot.node_count(), 1);
    }

    // ── Version chain tests ───────────────────────────────────────────────

    #[test]
    fn test_checkpoint_assigns_monotonic_versions() {
        let manager = SnapshotManager::new();
        assert_eq!(manager.version_count(), 0);

        let v1 = manager.checkpoint();
        let v2 = manager.checkpoint();
        let v3 = manager.checkpoint();

        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(v3, 3);
        assert_eq!(manager.version_count(), 3);
        assert_eq!(manager.oldest_version(), Some(1));
        assert_eq!(manager.newest_version(), Some(3));
    }

    #[test]
    fn test_as_of_finds_correct_version() {
        let manager = SnapshotManager::with_state(&HashMap::from([(1, vec![2])]), &HashMap::new());

        let v1 = manager.checkpoint();

        // Mutate and checkpoint again
        manager.update_snapshot(&HashMap::from([(1, vec![2, 3])]), &HashMap::new());
        let v2 = manager.checkpoint();

        let snap1 = manager.as_of(v1).expect("version 1 should exist");
        assert_eq!(snap1.version, v1);
        assert_eq!(snap1.state.get_outgoing(1), Some(&vec![2]));

        let snap2 = manager.as_of(v2).expect("version 2 should exist");
        assert_eq!(snap2.version, v2);
        assert_eq!(snap2.state.get_outgoing(1), Some(&vec![2, 3]));

        // Historical read must not see the mutation
        let snap1_again = manager.as_of(v1).expect("version 1 still exists");
        assert_eq!(snap1_again.state.get_outgoing(1), Some(&vec![2]));
    }

    #[test]
    fn test_as_of_returns_none_for_unknown_version() {
        let manager = SnapshotManager::new();
        manager.checkpoint();
        manager.checkpoint();

        assert!(manager.as_of(999).is_none());
        assert!(manager.as_of(0).is_none());
    }

    #[test]
    fn test_bounded_eviction_keeps_max_history() {
        let manager = SnapshotManager::with_max_history(3);
        let v1 = manager.checkpoint();
        let v2 = manager.checkpoint();
        let v3 = manager.checkpoint();

        // At capacity: 3 versions, oldest = v1, newest = v3
        assert_eq!(manager.version_count(), 3);
        assert_eq!(manager.oldest_version(), Some(v1));
        assert_eq!(manager.newest_version(), Some(v3));
        assert!(manager.as_of(v3).is_some());

        // Push one more → v1 evicted
        let v4 = manager.checkpoint();
        assert_eq!(manager.version_count(), 3);
        assert_eq!(manager.oldest_version(), Some(v2));
        assert_eq!(manager.newest_version(), Some(v4));
        assert!(manager.as_of(v1).is_none(), "v1 should have been evicted");
        assert!(manager.as_of(v4).is_some());
    }

    #[test]
    fn test_versions_iteration_is_oldest_first() {
        let manager = SnapshotManager::new();
        manager.checkpoint();
        manager.checkpoint();
        manager.checkpoint();

        let versions = manager.versions();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[2].version, 3);
    }

    #[test]
    fn test_clear_history_keeps_live_state() {
        let manager = SnapshotManager::with_state(&HashMap::from([(1, vec![2])]), &HashMap::new());
        manager.checkpoint();
        manager.checkpoint();

        assert_eq!(manager.version_count(), 2);
        manager.clear_history();
        assert_eq!(manager.version_count(), 0);
        assert!(manager.oldest_version().is_none());

        // Live state intact
        let snap = manager.acquire_snapshot();
        assert_eq!(snap.node_count(), 1);
    }

    #[test]
    fn test_as_of_at_finds_nearest_timestamp() {
        // We build a history with controlled timestamps by pushing directly
        // into the private history deque (accessible from this child module).
        use std::time::{Duration, UNIX_EPOCH};

        let manager = SnapshotManager::new();

        let mut history = manager.history.lock();
        for i in 0u64..5 {
            let state = SnapshotState::new(
                &HashMap::from([(i as i64, vec![(i + 1) as i64])]),
                &HashMap::new(),
            );
            history.push_back(VersionedSnapshot {
                version: i + 1,
                created_at: UNIX_EPOCH + Duration::from_secs(100 * (i + 1)),
                state: Arc::new(state),
            });
        }
        // Fix next_version so future checkpoints don't collide.
        drop(history);
        manager.next_version.store(6, Ordering::SeqCst);

        // Query between v2 (t=200) and v3 (t=300) → should return v2.
        let t250 = UNIX_EPOCH + Duration::from_secs(250);
        let snap = manager.as_of_at(t250).expect("should find v2");
        assert_eq!(snap.version, 2);

        // Query exactly at v3's timestamp → should return v3.
        let t300 = UNIX_EPOCH + Duration::from_secs(300);
        let snap = manager.as_of_at(t300).expect("should find v3");
        assert_eq!(snap.version, 3);

        // Query before all versions → None.
        let t50 = UNIX_EPOCH + Duration::from_secs(50);
        assert!(manager.as_of_at(t50).is_none());

        // Query after all versions → returns the newest (v5).
        let t999 = UNIX_EPOCH + Duration::from_secs(999);
        let snap = manager.as_of_at(t999).expect("should find v5");
        assert_eq!(snap.version, 5);
    }

    #[test]
    fn test_empty_history_queries() {
        let manager = SnapshotManager::new();

        assert_eq!(manager.version_count(), 0);
        assert!(manager.oldest_version().is_none());
        assert!(manager.newest_version().is_none());
        assert!(manager.as_of(1).is_none());
        assert!(manager.as_of_at(SystemTime::now()).is_none());
        assert!(manager.versions().is_empty());
    }
}
