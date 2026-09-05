//! Source connectors. P1 ships files; database connectors follow in E2/E4.
//!
//! The trait is synchronous chunk-pulling for now — the files connector is
//! naturally sync, and callers run extraction on a background thread. The
//! database connectors added in E2 revisit this (tokio drivers wrapped
//! per-chunk).

pub mod files;

use anyhow::Result;
use polars::prelude::{DataFrame, Schema};

use crate::spec::{SourceObject, StreamSpec};

pub trait Extractor: Send {
    /// The source schema, probed without moving data where possible.
    fn schema(&mut self) -> Result<Schema>;

    /// The next chunk, bounded by the configured chunk size; `None` when
    /// exhausted.
    fn next_chunk(&mut self) -> Result<Option<DataFrame>>;
}

/// Builds the extractor for a stream. `project_root` anchors relative file
/// paths.
pub fn make_extractor(
    project_root: &std::path::Path,
    stream: &StreamSpec,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    match &stream.source {
        SourceObject::Path { path, format, csv } => Ok(Box::new(files::FileExtractor::new(
            project_root,
            path,
            *format,
            csv.clone(),
            chunk_rows,
        )?)),
        SourceObject::Table { .. } => anyhow::bail!(
            "stream {:?}: database sources are not implemented yet — coming in the next phase",
            stream.name
        ),
    }
}
