//! Backend factory for unified graph creation.

use std::path::Path;

use super::graph_config::GraphConfig;
use crate::SqliteGraphError;
use crate::backend::{CombinedGraphBackend, GraphBackend, SqliteGraphBackend};

/// Open a graph with the specified configuration.
///
/// This is the unified factory function that allows runtime backend selection.
pub fn open_graph<P: AsRef<Path>>(
    path: P,
    cfg: &GraphConfig,
) -> Result<Box<dyn GraphBackend>, SqliteGraphError> {
    match cfg.backend {
        super::kinds::BackendKind::SQLite => {
            let sqlite_graph = if cfg.sqlite.without_migrations {
                crate::graph::SqliteGraph::open_without_migrations(&path)?
            } else {
                crate::graph::SqliteGraph::open(&path)?
            };

            // Apply PRAGMA settings
            for (key, value) in &cfg.sqlite.pragma_settings {
                let pragma_sql = format!("PRAGMA {} = {}", key, value);
                match sqlite_graph.connection().execute(&pragma_sql, []) {
                    Ok(_) => {}
                    Err(rusqlite::Error::ExecuteReturnedResults) => {}
                    Err(e) => {
                        return Err(SqliteGraphError::connection(format!(
                            "PRAGMA {} = {}: {}",
                            key, value, e
                        )));
                    }
                }
            }

            Ok(Box::new(SqliteGraphBackend::from_graph(sqlite_graph)))
        }
        super::kinds::BackendKind::Combined => {
            let sqlite_graph = if cfg.sqlite.without_migrations {
                crate::graph::SqliteGraph::open_without_migrations(&path)?
            } else {
                crate::graph::SqliteGraph::open(&path)?
            };

            for (key, value) in &cfg.sqlite.pragma_settings {
                let pragma_sql = format!("PRAGMA {} = {}", key, value);
                match sqlite_graph.connection().execute(&pragma_sql, []) {
                    Ok(_) => {}
                    Err(rusqlite::Error::ExecuteReturnedResults) => {}
                    Err(e) => {
                        return Err(SqliteGraphError::connection(format!(
                            "PRAGMA {} = {}: {}",
                            key, value, e
                        )));
                    }
                }
            }

            Ok(Box::new(CombinedGraphBackend::from_graph_with_config(
                sqlite_graph,
                cfg.combined.clone(),
            )))
        }
        super::kinds::BackendKind::Native => {
            let path_ref = path.as_ref();
            let file_exists = path_ref.exists();
            let native_graph = if file_exists {
                crate::backend::native::v3::V3Backend::open(&path)?
            } else if cfg.native.create_if_missing {
                crate::backend::native::v3::V3Backend::create(&path)?
            } else {
                return Err(SqliteGraphError::connection(format!(
                    "Database file does not exist and create_if_missing is false: {}",
                    path_ref.display()
                )));
            };

            Ok(Box::new(native_graph))
        }
    }
}
