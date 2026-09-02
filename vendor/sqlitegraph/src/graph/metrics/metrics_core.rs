use std::sync::atomic::{AtomicU64, Ordering};

use super::metrics_snapshot::GraphMetricsSnapshot;

#[derive(Default)]
pub struct GraphMetrics {
    prepares: AtomicU64,
    executes: AtomicU64,
    tx_begin: AtomicU64,
    tx_commit: AtomicU64,
    tx_rollback: AtomicU64,
    prepare_cache_hits: AtomicU64,
    prepare_cache_misses: AtomicU64,
}

impl GraphMetrics {
    pub fn snapshot(&self) -> GraphMetricsSnapshot {
        GraphMetricsSnapshot {
            prepare_count: self.prepares.load(Ordering::Relaxed),
            execute_count: self.executes.load(Ordering::Relaxed),
            tx_begin_count: self.tx_begin.load(Ordering::Relaxed),
            tx_commit_count: self.tx_commit.load(Ordering::Relaxed),
            tx_rollback_count: self.tx_rollback.load(Ordering::Relaxed),
            prepare_cache_hits: self.prepare_cache_hits.load(Ordering::Relaxed),
            prepare_cache_misses: self.prepare_cache_misses.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.prepares.store(0, Ordering::Relaxed);
        self.executes.store(0, Ordering::Relaxed);
        self.tx_begin.store(0, Ordering::Relaxed);
        self.tx_commit.store(0, Ordering::Relaxed);
        self.tx_rollback.store(0, Ordering::Relaxed);
        self.prepare_cache_hits.store(0, Ordering::Relaxed);
        self.prepare_cache_misses.store(0, Ordering::Relaxed);
    }

    pub fn record_prepare(&self) {
        self.prepares.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_execute(&self, sql: Option<&str>) {
        self.executes.fetch_add(1, Ordering::Relaxed);
        if let Some(keyword) = sql.and_then(super::utils::leading_keyword) {
            if keyword.eq_ignore_ascii_case("BEGIN") {
                self.tx_begin.fetch_add(1, Ordering::Relaxed);
            } else if keyword.eq_ignore_ascii_case("COMMIT") {
                self.tx_commit.fetch_add(1, Ordering::Relaxed);
            } else if keyword.eq_ignore_ascii_case("ROLLBACK") {
                self.tx_rollback.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn record_prepare_cache_hit(&self) {
        self.prepare_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_prepare_cache_miss(&self) {
        self.prepare_cache_misses.fetch_add(1, Ordering::Relaxed);
        self.record_prepare();
    }
}
