//! Combined backend configuration.

/// Read policy for combined-mode traversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CombinedReadMode {
    /// Always read through authoritative SQLite execution paths.
    #[default]
    SqliteOnly,
    /// Prefer materialized traversal views when available, otherwise fall back
    /// to authoritative SQLite reads.
    PreferMaterialized,
}

/// Configuration for combined backend operations.
#[derive(Clone, Debug, Default)]
pub struct CombinedConfig {
    /// Live traversal read strategy for combined mode.
    pub read_mode: CombinedReadMode,
}

impl CombinedConfig {
    /// Create a new combined config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Prefer materialized traversal views when available.
    pub fn with_materialized_reads(mut self) -> Self {
        self.read_mode = CombinedReadMode::PreferMaterialized;
        self
    }
}
