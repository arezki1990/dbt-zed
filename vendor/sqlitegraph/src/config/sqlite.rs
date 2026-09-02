//! SQLite backend configuration.

use std::collections::HashMap;

/// Configuration for SQLite backend operations.
#[derive(Clone, Debug, Default)]
pub struct SqliteConfig {
    /// Skip schema initialization during opening
    pub without_migrations: bool,
    /// Optional cache size for prepared statements
    pub cache_size: Option<usize>,
    /// Optional pool size for connection pooling (default: 5)
    pub pool_size: Option<usize>,
    /// Additional SQLite PRAGMA settings
    pub pragma_settings: HashMap<String, String>,
}

impl SqliteConfig {
    /// Create a new SQLite config with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether to skip schema initialization (builder pattern)
    pub fn with_migrations_disabled(mut self, without_migrations: bool) -> Self {
        self.without_migrations = without_migrations;
        self
    }

    /// Set the prepared statement cache size (builder pattern)
    pub fn with_cache_size(mut self, cache_size: usize) -> Self {
        self.cache_size = Some(cache_size);
        self
    }

    /// Add a PRAGMA setting (builder pattern)
    pub fn with_pragma(mut self, key: &str, value: &str) -> Self {
        self.pragma_settings
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Configure for WAL mode (builder pattern convenience method)
    pub fn with_wal_mode(mut self) -> Self {
        self.pragma_settings
            .insert("journal_mode".to_string(), "WAL".to_string());
        self
    }

    /// Configure for better performance with some safety trade-offs
    pub fn with_performance_mode(mut self) -> Self {
        self.pragma_settings
            .insert("journal_mode".to_string(), "WAL".to_string());
        self.pragma_settings
            .insert("synchronous".to_string(), "NORMAL".to_string());
        self
    }

    /// Set the connection pool size (builder pattern)
    ///
    /// # Arguments
    ///
    /// * `size` - Maximum number of connections in the pool
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let cfg = SqliteConfig::new().with_pool_size(10);
    /// ```
    pub fn with_pool_size(mut self, size: usize) -> Self {
        self.pool_size = Some(size);
        self
    }

    /// Set the maximum number of connections (alias for with_pool_size)
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum number of connections in the pool
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let cfg = SqliteConfig::new().with_max_connections(10);
    /// ```
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.pool_size = Some(max);
        self
    }
}
