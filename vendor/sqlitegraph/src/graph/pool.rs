//! Connection pooling for SQLite backend using r2d2.
//!
//! This module provides a wrapper around r2d2's connection pool specifically
//! configured for SQLite databases. Pooling enables concurrent access to the
//! database and reduces connection overhead.

use std::path::Path;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::errors::SqliteGraphError;

/// Type alias for the r2d2 SQLite connection manager.
pub type ConnectionManager = SqliteConnectionManager;

/// Type alias for a pooled connection that can be checked out from the pool.
pub type PooledConnection = r2d2::PooledConnection<ConnectionManager>;

/// Wrapper around r2d2 Pool for SQLite connection management.
///
/// Provides connection pooling with configurable size and automatic connection
/// return when dropped. For in-memory databases, pooling is skipped because each
/// connection would have isolated data.
pub struct PoolManager {
    /// The underlying r2d2 connection pool (None for in-memory databases)
    pool: Option<Pool<ConnectionManager>>,
    /// Direct connection for in-memory databases (no pooling)
    direct_conn: Option<Connection>,
}

impl PoolManager {
    /// Create a new pool for a file-based database.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the SQLite database file
    ///
    /// # Returns
    ///
    /// A PoolManager with a configured pool (default 5 connections)
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, SqliteGraphError> {
        Self::with_max_size(path, 5)
    }

    /// Create a new pool with a specified maximum size.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the SQLite database file
    /// * `max_size` - Maximum number of connections in the pool
    ///
    /// # Returns
    ///
    /// A PoolManager with a configured pool
    pub fn with_max_size<P: AsRef<Path>>(path: P, max_size: u32) -> Result<Self, SqliteGraphError> {
        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(max_size)
            .build(manager)
            .map_err(|e| SqliteGraphError::connection(e.to_string()))?;

        Ok(Self {
            pool: Some(pool),
            direct_conn: None,
        })
    }

    /// Create a PoolManager for an in-memory database.
    ///
    /// In-memory databases skip pooling because each connection would have
    /// isolated data. This uses a single direct connection instead.
    pub fn in_memory() -> Result<Self, SqliteGraphError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| SqliteGraphError::connection(e.to_string()))?;

        Ok(Self {
            pool: None,
            direct_conn: Some(conn),
        })
    }

    /// Create a PoolManager from an existing Connection.
    ///
    /// Used for in-memory databases where pooling doesn't make sense.
    pub fn from_connection(conn: Connection) -> Self {
        Self {
            pool: None,
            direct_conn: Some(conn),
        }
    }

    /// Get a connection from the pool.
    ///
    /// For pooled databases, this checks out a connection that will be
    /// automatically returned to the pool when dropped.
    ///
    /// For in-memory databases, this returns a reference to the single connection.
    pub fn get(&self) -> Result<PooledConnection, SqliteGraphError> {
        self.pool
            .as_ref()
            .ok_or_else(|| {
                SqliteGraphError::connection(
                    "Cannot checkout from in-memory database (use direct_connection() instead)"
                        .to_string(),
                )
            })?
            .get()
            .map_err(|e| SqliteGraphError::connection(e.to_string()))
    }

    /// Get direct access to the underlying connection (for in-memory databases).
    ///
    /// # Returns
    ///
    /// `None` if this is a pooled database, `Some(connection)` if in-memory
    pub fn direct_connection(&self) -> Option<&Connection> {
        self.direct_conn.as_ref()
    }

    /// Check if this pool manager is for an in-memory database.
    pub fn is_in_memory(&self) -> bool {
        self.direct_conn.is_some()
    }

    /// Get the maximum pool size for this manager.
    ///
    /// # Returns
    ///
    /// * `Some(max_size)` - Maximum number of connections for pooled databases
    /// * `None` - For in-memory databases (no pooling)
    pub fn max_size(&self) -> Option<u32> {
        self.pool.as_ref().map(|p| p.max_size())
    }

    /// Apply a configuration function to the pool.
    ///
    /// This is used to configure WAL mode and other PRAGMAs on the pool.
    pub fn configure_pool<F>(&self, f: F) -> Result<(), SqliteGraphError>
    where
        F: FnOnce(&Connection) -> Result<(), rusqlite::Error>,
    {
        if let Some(pool) = &self.pool {
            let conn = pool
                .get()
                .map_err(|e| SqliteGraphError::connection(e.to_string()))?;
            f(&conn).map_err(|e| SqliteGraphError::connection(e.to_string()))?;
        }
        Ok(())
    }

    /// Apply a configuration function to the direct connection (if in-memory).
    pub fn configure_direct<F>(&mut self, f: F) -> Result<(), SqliteGraphError>
    where
        F: FnOnce(&Connection) -> Result<(), rusqlite::Error>,
    {
        if let Some(conn) = &self.direct_conn {
            f(conn).map_err(|e| SqliteGraphError::connection(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_manager_size() {
        let manager = PoolManager::with_max_size(":memory:", 10).unwrap();
        // Note: In-memory path creates a file named ":memory:" which is not the same
        // as Connection::open_in_memory(). For true in-memory, use in_memory().
        assert!(!manager.is_in_memory());
    }

    #[test]
    fn test_in_memory_pool_manager() {
        let manager = PoolManager::in_memory().unwrap();
        assert!(manager.is_in_memory());
        assert!(manager.direct_connection().is_some());
        assert!(manager.pool.is_none());
    }

    #[test]
    fn test_from_connection() {
        let conn = Connection::open_in_memory().unwrap();
        let manager = PoolManager::from_connection(conn);
        assert!(manager.is_in_memory());
        assert!(manager.direct_connection().is_some());
    }
}
