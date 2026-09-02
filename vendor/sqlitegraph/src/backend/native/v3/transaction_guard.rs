//! Transaction and savepoint RAII guards for V3 backend.

use super::V3Backend;
use crate::SqliteGraphError;

/// Transaction guard for V3Backend
///
/// Automatically rolls back on drop unless explicitly committed.
/// Use with RAII pattern for exception safety.
pub struct V3TransactionGuard<'a> {
    backend: &'a V3Backend,
    committed: bool,
}

impl<'a> V3TransactionGuard<'a> {
    pub(crate) fn new(backend: &'a V3Backend) -> Self {
        Self {
            backend,
            committed: false,
        }
    }

    /// Commit the transaction
    ///
    /// Commits all changes made within this transaction.
    /// Subsequent operations will start a new auto-transaction.
    pub fn commit(mut self) -> Result<(), SqliteGraphError> {
        self.committed = true;
        self.backend.commit_transaction()
    }

    /// Rollback the transaction
    ///
    /// Rolls back all changes made within this transaction.
    /// Transaction is also rolled back on drop if not committed.
    pub fn rollback(mut self) -> Result<(), SqliteGraphError> {
        self.committed = true;
        self.backend.rollback_transaction()
    }
}

impl<'a> Drop for V3TransactionGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.backend.rollback_transaction();
        }
    }
}

/// Savepoint guard for nested transactions
///
/// Automatically rolls back savepoint on drop unless explicitly committed.
pub struct V3SavepointGuard<'a> {
    backend: &'a V3Backend,
    name: String,
    committed: bool,
}

impl<'a> V3SavepointGuard<'a> {
    pub(crate) fn new(backend: &'a V3Backend, name: String) -> Self {
        Self {
            backend,
            name,
            committed: false,
        }
    }

    /// Commit the savepoint
    ///
    /// Commits all changes made within this savepoint.
    /// Parent transaction remains active.
    pub fn commit(mut self) -> Result<(), SqliteGraphError> {
        self.committed = true;
        self.backend.release_savepoint(&self.name)
    }

    /// Rollback the savepoint
    ///
    /// Rolls back all changes made within this savepoint.
    /// Parent transaction remains active.
    pub fn rollback(mut self) -> Result<(), SqliteGraphError> {
        self.committed = true;
        self.backend.rollback_savepoint(&self.name)
    }
}

impl<'a> Drop for V3SavepointGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.backend.rollback_savepoint(&self.name);
        }
    }
}
