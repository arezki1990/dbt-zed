//! Snapshot isolation for ACID compliance
//!
//! This module provides the SnapshotId type which enforces that all read operations
//! only observe data committed at or before a specific transaction snapshot.
//!
//! # Hard Rule
//!
//! **No API may observe state not bound to a committed snapshot_id.**
//!
//! If a value cannot be tied to a committed snapshot → it does not exist.
//!
//! # Example
//!
//! ```ignore
//! use sqlitegraph::snapshot::SnapshotId;
//!
//! // Get current snapshot (only committed data visible)
//! let snapshot = SnapshotId::current();
//!
//! // Read from database using snapshot
//! let node = backend.get_node(snapshot, node_id)?;
//!
//! // Create snapshot from specific transaction
//! let snapshot = SnapshotId::from_tx(12345);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for snapshot IDs when no WAL manager is available.
static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Snapshot identifier - points to committed transaction state
///
/// Only data committed at or before this snapshot_id is visible.
/// If a value cannot be tied to a committed snapshot_id → it does not exist.
///
/// # Invariant
///
/// snapshot_id.0 MUST correspond to a committed transaction.
/// Uncommitted transactions do not create valid snapshots.
///
/// # Representation
///
/// Internally, SnapshotId wraps a TransactionId (u64). This is valid because:
/// - TransactionId is allocated at begin_transaction()
/// - SnapshotId is created at commit_transaction()
/// - The 1:1 mapping ensures snapshot_id uniquely identifies committed state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotId(pub u64);

impl SnapshotId {
    /// The "current" snapshot - sees only committed data
    ///
    /// This returns a special sentinel value (0) that represents "all committed data".
    /// All reads using this snapshot are guaranteed to see only data that has been
    /// durably committed.
    ///
    /// # Backend Behavior
    ///
    /// - **SQLite backend**: `SnapshotId(0)` reads the current committed state.
    ///   A non-zero LSN resolves to a checkpointed MVCC version
    ///   (see [`SqliteGraph::checkpoint`](crate::SqliteGraph::checkpoint)) and
    ///   serves adjacency-only reads (`neighbors`, `bfs`) from the version chain.
    /// - **Native-v3 backend**: Uses `SnapshotId(0)` or `new_incrementing()`.
    ///
    /// # Why SnapshotId(0)?
    ///
    /// `SnapshotId(0)` is the live/current state. Non-zero LSNs map to
    /// checkpointed versions of the bounded MVCC version chain — adjacency-only
    /// methods (`neighbors`, `bfs`) serve them; methods needing entity
    /// properties or typed edges read the live graph only.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::snapshot::SnapshotId;
    /// let snapshot = SnapshotId::current();
    /// // snapshot is now SnapshotId(0) - "current committed data"
    /// ```
    pub fn current() -> Self {
        SnapshotId(0)
    }

    /// Create a new incrementing snapshot ID
    ///
    /// This generates a new snapshot ID using a global atomic counter.
    /// Each call returns a unique, monotonically increasing snapshot ID.
    ///
    /// # When to Use
    ///
    /// - **Native-v3 backend**: Use this when you need unique snapshot IDs for MVCC
    /// - **SQLite backend**: Do NOT use this - SQLite only supports SnapshotId(0)
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::snapshot::SnapshotId;
    /// let snapshot1 = SnapshotId::new_incrementing();
    /// let snapshot2 = SnapshotId::new_incrementing();
    /// assert!(snapshot2.as_u64() > snapshot1.as_u64());
    /// ```
    pub fn new_incrementing() -> Self {
        let lsn = SNAPSHOT_COUNTER.fetch_add(1, Ordering::SeqCst);
        SnapshotId(lsn)
    }

    /// Create from explicit transaction ID
    ///
    /// # Arguments
    ///
    /// * `tx_id` - A committed transaction ID
    ///
    /// # Important
    ///
    /// The caller MUST ensure that tx_id corresponds to a committed transaction.
    /// Using an uncommitted transaction ID violates snapshot isolation guarantees.
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use sqlitegraph::snapshot::SnapshotId;
    /// // After commit returns SnapshotId
    /// let snapshot = coordinator.commit_transaction(tx_id)?;
    /// // Later, reuse same snapshot for repeatable reads
    /// let node = backend.get_node(snapshot, node_id)?;
    /// ```
    pub fn from_tx(tx_id: u64) -> Self {
        SnapshotId(tx_id)
    }

    /// Create snapshot from explicit LSN (Log Sequence Number)
    ///
    /// # Arguments
    ///
    /// * `lsn` - A commit LSN representing a committed transaction
    ///
    /// # Important
    ///
    /// The caller MUST ensure that lsn corresponds to a committed transaction.
    /// Using an uncommitted LSN violates snapshot isolation guarantees.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::snapshot::SnapshotId;
    /// // Create snapshot at specific LSN
    /// let snapshot = SnapshotId::from_lsn(12345);
    /// ```
    pub fn from_lsn(lsn: u64) -> Self {
        SnapshotId(lsn)
    }

    /// Invalid snapshot - used for error cases
    ///
    /// This sentinel value indicates that no valid snapshot exists.
    /// Read operations receiving this snapshot should return an error.
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use sqlitegraph::snapshot::SnapshotId;
    /// fn validate_snapshot(snapshot: SnapshotId) -> Result<(), &'static str> {
    ///     if snapshot == SnapshotId::invalid() {
    ///         return Err("Invalid snapshot");
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn invalid() -> Self {
        SnapshotId(u64::MAX)
    }

    /// Check if this snapshot is valid
    ///
    /// Returns false for the invalid sentinel value.
    pub fn is_valid(&self) -> bool {
        self.0 != u64::MAX
    }

    /// Get the underlying transaction ID
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// Get snapshot as LSN (Log Sequence Number)
    ///
    /// Since SnapshotId wraps a commit LSN, this returns the LSN directly.
    /// Used for WAL record visibility checks.
    pub fn as_lsn(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_id_creation() {
        let snapshot = SnapshotId(12345);
        assert_eq!(snapshot.as_u64(), 12345);
    }

    #[test]
    fn test_snapshot_id_from_tx() {
        let snapshot = SnapshotId::from_tx(999);
        assert_eq!(snapshot.as_u64(), 999);
    }

    #[test]
    fn test_snapshot_id_from_lsn() {
        let snapshot = SnapshotId::from_lsn(12345);
        assert_eq!(snapshot.as_lsn(), 12345);
        assert_eq!(snapshot.as_u64(), 12345);
    }

    #[test]
    fn test_snapshot_id_invalid() {
        let invalid = SnapshotId::invalid();
        assert_eq!(invalid.as_u64(), u64::MAX);
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_snapshot_id_valid() {
        let valid = SnapshotId::from_tx(100);
        assert!(valid.is_valid());
    }

    #[test]
    fn test_snapshot_id_current() {
        let current = SnapshotId::current();
        // Current snapshot should always be SnapshotId(0) - the "current" sentinel
        assert_eq!(current, SnapshotId(0));
        assert!(current.is_valid());
    }

    #[test]
    fn test_snapshot_id_new_incrementing() {
        let snapshot1 = SnapshotId::new_incrementing();
        let snapshot2 = SnapshotId::new_incrementing();
        let snapshot3 = SnapshotId::new_incrementing();

        // Each call should return a unique, incrementing ID
        assert!(snapshot2.as_u64() > snapshot1.as_u64());
        assert!(snapshot3.as_u64() > snapshot2.as_u64());
        assert!(snapshot1.is_valid());
        assert!(snapshot2.is_valid());
        assert!(snapshot3.is_valid());
    }

    #[test]
    fn test_snapshot_id_equality() {
        let s1 = SnapshotId(100);
        let s2 = SnapshotId(100);
        let s3 = SnapshotId(200);

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_snapshot_id_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(SnapshotId(100));
        set.insert(SnapshotId(200));
        set.insert(SnapshotId(100)); // Duplicate

        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_snapshot_id_copy() {
        let s1 = SnapshotId(100);
        let s2 = s1; // Copy, not move

        assert_eq!(s1.as_u64(), 100);
        assert_eq!(s2.as_u64(), 100);
    }
}
