//! Native backend configuration.

use crate::backend::native::CpuProfile;

/// Configuration for native backend operations.
#[derive(Clone, Debug)]
pub struct NativeConfig {
    /// Whether to create the graph file if it doesn't exist
    pub create_if_missing: bool,
    /// Optional capacity pre-allocation for nodes (performance optimization)
    pub reserve_node_capacity: Option<usize>,
    /// Optional capacity pre-allocation for edges (performance optimization)
    pub reserve_edge_capacity: Option<usize>,
    /// CPU Profile for performance optimizations
    pub cpu_profile: Option<CpuProfile>,
    /// Maximum number of parallel WAL recovery transactions (default: 4)
    pub max_parallel_transactions: usize,
}

impl Default for NativeConfig {
    fn default() -> Self {
        Self {
            create_if_missing: true,
            reserve_node_capacity: None,
            reserve_edge_capacity: None,
            cpu_profile: None,
            max_parallel_transactions: 4,
        }
    }
}

impl NativeConfig {
    /// Get the effective CPU profile, considering environment variables and defaults
    pub fn effective_cpu_profile(&self) -> CpuProfile {
        // Check environment variable first
        if let Ok(env_profile) = std::env::var("SQLITEGRAPH_NATIVE_CPU_PROFILE") {
            if let Ok(profile) = env_profile.parse() {
                return crate::backend::native::cpu_tuning::resolve_cpu_profile(profile);
            }
            eprintln!(
                "Warning: Invalid SQLITEGRAPH_NATIVE_CPU_PROFILE '{}', using default profile",
                env_profile
            );
        }

        // Use config field if set
        if let Some(profile) = self.cpu_profile {
            return crate::backend::native::cpu_tuning::resolve_cpu_profile(profile);
        }

        // Default to Generic
        CpuProfile::Generic
    }

    /// Set the CPU profile (builder pattern)
    pub fn with_cpu_profile(mut self, profile: CpuProfile) -> Self {
        self.cpu_profile = Some(profile);
        self
    }

    /// Set the maximum parallel WAL recovery transactions (builder pattern)
    pub fn with_parallel_recovery(mut self, degree: usize) -> Self {
        self.max_parallel_transactions = degree;
        self
    }
}
