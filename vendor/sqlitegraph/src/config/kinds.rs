//! Backend selection for graph storage implementations.

use std::env;

/// Backend selection enum for choosing between storage implementations.
///
/// This enum allows runtime selection of the graph storage backend
/// without compile-time dependencies on specific implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BackendKind {
    /// SQLite-based backend with full SQL capabilities
    #[default]
    SQLite,
    /// Native file-based backend with adjacency storage
    Native,
    /// SQLite-authoritative mode with explicit room for derived graph materialization
    Combined,
}

impl BackendKind {
    /// Resolve backend mode from the `GRAPH_BACKEND` environment variable.
    ///
    /// Accepted values:
    /// - `sqlite`
    /// - `native`
    /// - `native-v3`
    /// - `combined`
    ///
    /// Any other value falls back to `SQLite`.
    pub fn from_env() -> Self {
        match env::var("GRAPH_BACKEND").as_deref() {
            Ok("combined") => BackendKind::Combined,
            Ok("native") | Ok("native-v3") => BackendKind::Native,
            _ => BackendKind::SQLite,
        }
    }
}
