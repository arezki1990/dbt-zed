//! Dependency health monitoring utilities
//!
//! This module provides runtime monitoring of dependency versions
//! and health status for production deployments.
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::dependency_monitor;
//!
//! // Print dependency health report
//! dependency_monitor::print_dependency_report();
//!
//! // Get dependency info for programmatic access
//! let deps = dependency_monitor::get_dependency_info();
//! for (name, info) in deps {
//!     println!("{}: {} ({:?})", info.name, info.version, info.status);
//! }
//! ```

use std::collections::HashMap;

/// Information about a dependency
#[derive(Debug, Clone)]
pub struct DependencyInfo {
    /// Dependency name
    pub name: &'static str,
    /// Current version
    pub version: &'static str,
    /// Health status
    pub status: DependencyStatus,
    /// Minimum recommended version (if applicable)
    pub minimum_version: Option<&'static str>,
}

/// Dependency health status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyStatus {
    /// Dependency is healthy and actively maintained
    Healthy,
    /// Dependency is deprecated but still functional
    Deprecated,
    /// Security advisory issued for this version
    SecurityAdvisory,
    /// Unknown status (not tracked)
    Unknown,
}

impl DependencyStatus {
    /// Returns true if the dependency status requires action
    pub fn requires_action(&self) -> bool {
        matches!(
            self,
            DependencyStatus::Deprecated | DependencyStatus::SecurityAdvisory
        )
    }
}

/// Get dependency information for all tracked dependencies
///
/// Returns a HashMap mapping dependency names to their information.
pub fn get_dependency_info() -> HashMap<&'static str, DependencyInfo> {
    let mut deps = HashMap::new();

    // rusqlite - Healthy with bundled SQLite
    deps.insert(
        "rusqlite",
        DependencyInfo {
            name: "rusqlite",
            version: "0.31",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // bincode - Deprecated, migration to 2.0 needed
    deps.insert(
        "bincode",
        DependencyInfo {
            name: "bincode",
            version: "1.3",
            status: DependencyStatus::Deprecated,
            minimum_version: Some("2.0"),
        },
    );

    // r2d2_sqlite - Healthy
    deps.insert(
        "r2d2_sqlite",
        DependencyInfo {
            name: "r2d2_sqlite",
            version: "0.24",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // r2d2 - Healthy
    deps.insert(
        "r2d2",
        DependencyInfo {
            name: "r2d2",
            version: "0.8",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // parking_lot - Healthy
    deps.insert(
        "parking_lot",
        DependencyInfo {
            name: "parking_lot",
            version: "0.12",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // serde - Healthy
    deps.insert(
        "serde",
        DependencyInfo {
            name: "serde",
            version: "1.0",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // serde_json - Healthy
    deps.insert(
        "serde_json",
        DependencyInfo {
            name: "serde_json",
            version: "1.0",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // ahash - Healthy
    deps.insert(
        "ahash",
        DependencyInfo {
            name: "ahash",
            version: "0.8",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // rand - Healthy
    deps.insert(
        "rand",
        DependencyInfo {
            name: "rand",
            version: "0.8",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // arc-swap - Healthy
    deps.insert(
        "arc-swap",
        DependencyInfo {
            name: "arc-swap",
            version: "1.0",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // bytemuck - Healthy
    deps.insert(
        "bytemuck",
        DependencyInfo {
            name: "bytemuck",
            version: "1.13",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // binrw - Healthy
    deps.insert(
        "binrw",
        DependencyInfo {
            name: "binrw",
            version: "0.13",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // memmap2 - Healthy
    deps.insert(
        "memmap2",
        DependencyInfo {
            name: "memmap2",
            version: "0.9",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // log - Healthy
    deps.insert(
        "log",
        DependencyInfo {
            name: "log",
            version: "0.4",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    // rayon - Healthy
    deps.insert(
        "rayon",
        DependencyInfo {
            name: "rayon",
            version: "1.10",
            status: DependencyStatus::Healthy,
            minimum_version: None,
        },
    );

    deps
}

/// Print a formatted dependency health report to stdout
///
/// This is useful for debugging and monitoring.
///
/// # Example Output
///
/// ```text
/// === SQLiteGraph Dependency Report ===
/// rusqlite: 0.31 (Healthy)
/// bincode: 1.3 (Deprecated)
///   -> Migration recommended: >= 2.0
/// ...
/// ```
pub fn print_dependency_report() {
    println!("=== SQLiteGraph Dependency Report ===");

    let mut deps: Vec<_> = get_dependency_info().into_iter().collect();
    deps.sort_by(|a, b| a.0.cmp(b.0));

    let mut action_required = false;

    for (_name, info) in deps {
        println!("{}: {} ({:?})", info.name, info.version, info.status);

        if let Some(min_ver) = info.minimum_version {
            println!("  -> Migration recommended: >= {}", min_ver);
            action_required = true;
        }

        if info.status == DependencyStatus::SecurityAdvisory {
            println!("  -> WARNING: Security advisory issued!");
            action_required = true;
        }
    }

    if action_required {
        println!();
        println!("ACTION REQUIRED: Some dependencies require attention.");
    } else {
        println!();
        println!("All dependencies are healthy.");
    }
}

/// Check if any dependencies require action
///
/// Returns true if any dependencies are deprecated or have security advisories.
pub fn requires_action() -> bool {
    get_dependency_info()
        .values()
        .any(|info| info.status.requires_action())
}

/// Get a list of dependencies that require action
///
/// Returns a vector of (name, info) tuples for dependencies that need attention.
pub fn get_action_required() -> Vec<(&'static str, DependencyInfo)> {
    get_dependency_info()
        .into_iter()
        .filter(|(_, info)| info.status.requires_action())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_dependency_info() {
        let deps = get_dependency_info();
        assert!(!deps.is_empty());
        assert!(deps.contains_key("rusqlite"));
        assert!(deps.contains_key("bincode"));
    }

    #[test]
    fn test_bincode_deprecated() {
        let deps = get_dependency_info();
        let bincode = &deps["bincode"];
        assert_eq!(bincode.status, DependencyStatus::Deprecated);
        assert_eq!(bincode.minimum_version, Some("2.0"));
    }

    #[test]
    fn test_rusqlite_healthy() {
        let deps = get_dependency_info();
        let rusqlite = &deps["rusqlite"];
        assert_eq!(rusqlite.status, DependencyStatus::Healthy);
        assert_eq!(rusqlite.version, "0.31");
    }

    #[test]
    fn test_requires_action() {
        // bincode is deprecated, so action should be required
        assert!(requires_action());
    }

    #[test]
    fn test_get_action_required() {
        let action_deps = get_action_required();
        assert!(!action_deps.is_empty());
        // bincode should be in the list
        assert!(action_deps.iter().any(|(name, _)| *name == "bincode"));
    }

    #[test]
    fn test_status_requires_action() {
        assert!(!DependencyStatus::Healthy.requires_action());
        assert!(DependencyStatus::Deprecated.requires_action());
        assert!(DependencyStatus::SecurityAdvisory.requires_action());
        assert!(!DependencyStatus::Unknown.requires_action());
    }
}
