#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphMetricsSnapshot {
    pub prepare_count: u64,
    pub execute_count: u64,
    pub tx_begin_count: u64,
    pub tx_commit_count: u64,
    pub tx_rollback_count: u64,
    pub prepare_cache_hits: u64,
    pub prepare_cache_misses: u64,
}
