use rusqlite::{CachedStatement, Connection};
use std::sync::Arc;

use super::{
    metrics_core::GraphMetrics, statement_tracker::CacheObservation,
    statement_tracker::StatementTracker,
};

/// Instrumented connection wrapper for borrowed connections (used in in-memory mode).
#[derive(Copy, Clone)]
pub struct InstrumentedConnection<'a> {
    conn: &'a Connection,
    metrics: &'a GraphMetrics,
    tracker: &'a StatementTracker,
}

impl<'a> InstrumentedConnection<'a> {
    pub fn new(
        conn: &'a Connection,
        metrics: &'a GraphMetrics,
        tracker: &'a StatementTracker,
    ) -> Self {
        Self {
            conn,
            metrics,
            tracker,
        }
    }

    pub fn execute<P>(&self, sql: &str, params: P) -> Result<usize, rusqlite::Error>
    where
        P: rusqlite::Params,
    {
        self.metrics.record_execute(Some(sql));
        self.conn.execute(sql, params)
    }

    pub fn prepare_cached<'b>(
        &'b self,
        sql: &str,
    ) -> Result<InstrumentedCachedStatement<'b>, rusqlite::Error> {
        match self.tracker.observe(sql) {
            CacheObservation::Hit => self.metrics.record_prepare_cache_hit(),
            CacheObservation::Miss => self.metrics.record_prepare_cache_miss(),
        }
        Ok(InstrumentedCachedStatement {
            stmt: self.conn.prepare_cached(sql)?,
            metrics: self.metrics,
            sql: sql.to_string(),
        })
    }

    pub fn query_row<P, F, R>(&self, sql: &str, params: P, f: F) -> Result<R, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<R>,
    {
        self.metrics.record_prepare();
        self.metrics.record_execute(Some(sql));
        self.conn.query_row(sql, params, f)
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Get access to the underlying connection.
    pub fn inner(&self) -> &Connection {
        self.conn
    }
}

pub struct InstrumentedCachedStatement<'conn> {
    stmt: CachedStatement<'conn>,
    metrics: &'conn GraphMetrics,
    sql: String,
}

impl<'conn> InstrumentedCachedStatement<'conn> {
    pub fn execute<P>(&mut self, params: P) -> Result<usize, rusqlite::Error>
    where
        P: rusqlite::Params,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.execute(params)
    }

    pub fn query_map<P, F, T>(
        &mut self,
        params: P,
        f: F,
    ) -> Result<rusqlite::MappedRows<'_, F>, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.query_map(params, f)
    }

    pub fn query_row<P, F, R>(&mut self, params: P, f: F) -> Result<R, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<R>,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.query_row(params, f)
    }
}

/// Instrumented connection wrapper for owned pooled connections.
///
/// This is used for file-based databases with connection pooling.
/// The connection is automatically returned to the pool when dropped.
pub struct PooledInstrumentedConnection {
    conn: r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    metrics: Arc<GraphMetrics>,
    tracker: Arc<StatementTracker>,
}

impl PooledInstrumentedConnection {
    pub fn new(
        conn: r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        metrics: Arc<GraphMetrics>,
        tracker: Arc<StatementTracker>,
    ) -> Self {
        Self {
            conn,
            metrics,
            tracker,
        }
    }

    pub fn execute<P>(&self, sql: &str, params: P) -> Result<usize, rusqlite::Error>
    where
        P: rusqlite::Params,
    {
        self.metrics.record_execute(Some(sql));
        self.conn.execute(sql, params)
    }

    pub fn prepare_cached<'a>(
        &'a self,
        sql: &str,
    ) -> Result<PooledInstrumentedCachedStatement<'a>, rusqlite::Error> {
        match self.tracker.observe(sql) {
            CacheObservation::Hit => self.metrics.record_prepare_cache_hit(),
            CacheObservation::Miss => self.metrics.record_prepare_cache_miss(),
        }
        Ok(PooledInstrumentedCachedStatement {
            stmt: self.conn.prepare_cached(sql)?,
            metrics: &self.metrics,
            sql: sql.to_string(),
        })
    }

    pub fn query_row<P, F, R>(&self, sql: &str, params: P, f: F) -> Result<R, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<R>,
    {
        self.metrics.record_prepare();
        self.metrics.record_execute(Some(sql));
        self.conn.query_row(sql, params, f)
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Get access to the underlying connection.
    pub fn inner(&self) -> &rusqlite::Connection {
        &self.conn
    }
}

/// Instrumented cached statement for pooled connections.
pub struct PooledInstrumentedCachedStatement<'a> {
    stmt: rusqlite::CachedStatement<'a>,
    metrics: &'a GraphMetrics,
    sql: String,
}

impl<'a> PooledInstrumentedCachedStatement<'a> {
    pub fn execute<P>(&mut self, params: P) -> Result<usize, rusqlite::Error>
    where
        P: rusqlite::Params,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.execute(params)
    }

    pub fn query_map<P, F, T>(
        &mut self,
        params: P,
        f: F,
    ) -> Result<rusqlite::MappedRows<'_, F>, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.query_map(params, f)
    }

    pub fn query_row<P, F, R>(&mut self, params: P, f: F) -> Result<R, rusqlite::Error>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<R>,
    {
        self.metrics.record_execute(Some(self.sql.as_str()));
        self.stmt.query_row(params, f)
    }
}
