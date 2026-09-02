//! Transaction control methods for native-v3.

use super::{V3Backend, V3SavepointGuard, V3TransactionGuard};
use crate::SqliteGraphError;

impl V3Backend {
    /// Begin a new transaction
    ///
    /// Returns a transaction guard that auto-rolls back on drop unless explicitly committed.
    /// Uses DEFERRED behavior (transaction starts on first SQL statement).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let tx = backend.begin_transaction()?;
    /// backend.execute_sql("INSERT INTO node_properties (node_id, kind, name, data, created_at, created_version) VALUES (1, 'User', 'alice', '{}', 0, 1)")?;
    /// backend.execute_sql("INSERT INTO edge_attributes (src, dst, attr_name, attr_value, created_version) VALUES (1, 2, 'LINKS', '{}', 1)")?;
    /// tx.commit()?; // Commits both inserts atomically
    /// ```
    pub fn begin_transaction(&self) -> Result<V3TransactionGuard<'_>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        conn.execute("BEGIN DEFERRED", []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to begin transaction: {}", e))
        })?;
        drop(conn);

        if let Err(err) = self.begin_graph_transaction_frame() {
            let conn = self.sqlite_conn.lock();
            let _ = conn.execute("ROLLBACK", []);
            return Err(err);
        }

        Ok(V3TransactionGuard::new(self))
    }

    /// Create a nested transaction savepoint
    ///
    /// Savepoints allow nested transactions within a parent transaction.
    /// Auto-rolls back on drop unless explicitly committed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let tx = backend.begin_transaction()?;
    /// {
    ///     let sp = backend.savepoint("sp1")?;
    ///     // ... operations ...
    ///     sp.commit()?; // Commits savepoint but not transaction
    /// }
    /// tx.commit()?; // Commits entire transaction
    /// ```
    pub fn savepoint(&self, name: &str) -> Result<V3SavepointGuard<'_>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        let savepoint_sql = format!("SAVEPOINT {}", name);
        conn.execute(&savepoint_sql, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to create savepoint: {}", e))
        })?;
        drop(conn);

        if let Err(err) = self.begin_graph_transaction_frame() {
            let conn = self.sqlite_conn.lock();
            let rollback_sql = format!("ROLLBACK TO SAVEPOINT {}", name);
            let _ = conn.execute(&rollback_sql, []);
            let release_sql = format!("RELEASE SAVEPOINT {}", name);
            let _ = conn.execute(&release_sql, []);
            return Err(err);
        }

        Ok(V3SavepointGuard::new(self, name.to_string()))
    }

    /// Explicitly commit current transaction (guard-based preferred)
    ///
    /// This low-level method commits the current SQLite transaction.
    /// Prefer using transaction guards for automatic rollback.
    pub fn commit_transaction(&self) -> Result<(), SqliteGraphError> {
        self.flush_to_disk()?;
        let conn = self.sqlite_conn.lock();

        conn.execute("COMMIT", []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to commit transaction: {}", e))
        })?;
        drop(conn);
        self.discard_graph_transaction_frame()?;
        Ok(())
    }

    /// Explicitly rollback current transaction (guard-based preferred)
    ///
    /// This low-level method rolls back the current SQLite transaction.
    /// Prefer using transaction guards for automatic rollback.
    pub fn rollback_transaction(&self) -> Result<(), SqliteGraphError> {
        let conn = self.sqlite_conn.lock();

        conn.execute("ROLLBACK", []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to rollback transaction: {}", e))
        })?;
        drop(conn);
        self.rollback_graph_transaction_frame()?;
        Ok(())
    }

    pub(super) fn release_savepoint(&self, name: &str) -> Result<(), SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        let release_sql = format!("RELEASE SAVEPOINT {}", name);
        conn.execute(&release_sql, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to release savepoint: {}", e))
        })?;
        drop(conn);
        self.discard_graph_transaction_frame()?;
        Ok(())
    }

    pub(super) fn rollback_savepoint(&self, name: &str) -> Result<(), SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        let rollback_sql = format!("ROLLBACK TO SAVEPOINT {}", name);
        conn.execute(&rollback_sql, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to rollback to savepoint: {}", e))
        })?;
        let release_sql = format!("RELEASE SAVEPOINT {}", name);
        conn.execute(&release_sql, []).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to release rolled back savepoint: {}", e))
        })?;
        drop(conn);
        self.rollback_graph_transaction_frame()?;
        Ok(())
    }
}
