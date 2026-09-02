use crate::{
    SqliteGraphError,
    backend::SqliteGraphBackend,
    config::{GraphConfig, open_graph},
};
use std::path::Path;

pub use crate::config::BackendKind;

pub struct GraphBackendFactory {
    kind: BackendKind,
}

impl GraphBackendFactory {
    pub fn new(kind: BackendKind) -> Self {
        Self { kind }
    }

    pub fn from_env() -> Self {
        Self::new(BackendKind::from_env())
    }

    pub fn new_sqlite(&self) -> Result<SqliteGraphBackend, SqliteGraphError> {
        match self.kind {
            BackendKind::SQLite => SqliteGraphBackend::in_memory(),
            BackendKind::Native | BackendKind::Combined => Err(SqliteGraphError::invalid_input(
                "new_sqlite() only supports BackendKind::SQLite; use new_backend(path) for canonical backend selection",
            )),
        }
    }

    pub fn new_backend<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<Box<dyn crate::backend::GraphBackend>, SqliteGraphError> {
        let cfg = GraphConfig::new(self.kind);
        open_graph(path, &cfg)
    }
}
