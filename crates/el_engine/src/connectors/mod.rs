//! Source connectors. Files run in-process (polars scans). Database
//! sources run in the on-demand `zdbt-el-worker` binary so their drivers
//! never bloat the main app — unless this crate is built with the
//! matching feature (the worker itself, and feature-gated tests), in
//! which case they run in-process.

#[cfg(feature = "duckdb")]
pub mod duckdb;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod files;
pub mod remote;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use polars::prelude::{DataFrame, Schema};

use crate::env::EnvMap;
use crate::spec::{Connection, SourceObject, StreamSpec};

pub trait Extractor: Send {
    /// The source schema, probed without moving data where possible.
    fn schema(&mut self) -> Result<Schema>;

    /// The next chunk, bounded by the configured chunk size; `None` when
    /// exhausted.
    fn next_chunk(&mut self) -> Result<Option<DataFrame>>;
}

/// Everything extractor construction needs besides the stream itself.
pub struct SourceContext<'a> {
    pub project_root: &'a Path,
    /// The pipeline's source connection, when it resolves.
    pub connection: Option<&'a Connection>,
    pub env: &'a EnvMap,
    /// The installed connector worker binary, when present. Database
    /// sources without it fail with an actionable install message.
    pub worker: Option<&'a Path>,
}

impl SourceContext<'_> {
    fn resolve_path(&self, templated: &str) -> Result<PathBuf> {
        let resolved = crate::env::resolve_templates(templated, self.env)
            .map_err(|missing| anyhow::anyhow!("{missing}"))?;
        let path = PathBuf::from(resolved.expose());
        Ok(if path.is_absolute() {
            path
        } else {
            self.project_root.join(path)
        })
    }
}

/// Builds the extractor for a stream.
pub fn make_extractor(
    ctx: &SourceContext,
    stream: &StreamSpec,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    match &stream.source {
        SourceObject::Path { path, format, csv } => Ok(Box::new(files::FileExtractor::new(
            ctx.project_root,
            path,
            *format,
            csv.clone(),
            chunk_rows,
        )?)),
        SourceObject::Table { schema, table } => match ctx.connection {
            Some(Connection::Duckdb(conn)) => {
                let db_path = ctx
                    .resolve_path(&conn.path)
                    .context("resolving duckdb path")?;
                duckdb_extractor(ctx, &db_path, schema.as_deref(), table, chunk_rows)
            }
            Some(Connection::Postgres(conn)) => {
                let url = crate::env::resolve_templates(&conn.url, ctx.env)
                    .map_err(|missing| anyhow::anyhow!("{missing}"))?;
                postgres_extractor(ctx, url, schema.as_deref(), table, chunk_rows)
            }
            Some(other) if matches!(other.kind(), "mysql" | "mssql") => bail!(
                "stream {:?}: {} sources are not implemented yet — coming in the next phase",
                stream.name,
                other.kind()
            ),
            Some(other) => bail!(
                "stream {:?}: table sources need a database connection, but the source is {}",
                stream.name,
                other.kind()
            ),
            None => bail!(
                "stream {:?}: the pipeline's source connection is not defined in connections.yml",
                stream.name
            ),
        },
    }
}

#[cfg(feature = "duckdb")]
fn duckdb_extractor(
    _ctx: &SourceContext,
    db_path: &Path,
    schema: Option<&str>,
    table: &str,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    Ok(Box::new(duckdb::DuckdbExtractor::new(
        db_path, schema, table, chunk_rows,
    )?))
}

#[cfg(not(feature = "duckdb"))]
fn duckdb_extractor(
    ctx: &SourceContext,
    db_path: &Path,
    schema: Option<&str>,
    table: &str,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    let Some(worker) = ctx.worker else {
        bail!(
            "database connector support is not installed — install the zdbt connector \
             worker to extract from DuckDB/Postgres sources"
        );
    };
    Ok(Box::new(remote::RemoteExtractor::spawn_duckdb(
        worker, db_path, schema, table, chunk_rows,
    )?))
}

#[cfg(feature = "postgres")]
fn postgres_extractor(
    _ctx: &SourceContext,
    url: crate::env::Secret,
    schema: Option<&str>,
    table: &str,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    Ok(Box::new(postgres::PostgresExtractor::new(
        url.expose(),
        schema,
        table,
        chunk_rows,
    )?))
}

#[cfg(not(feature = "postgres"))]
fn postgres_extractor(
    ctx: &SourceContext,
    url: crate::env::Secret,
    schema: Option<&str>,
    table: &str,
    chunk_rows: usize,
) -> Result<Box<dyn Extractor>> {
    let Some(worker) = ctx.worker else {
        bail!(
            "database connector support is not installed — install the zdbt connector \
             worker to extract from Postgres sources"
        );
    };
    Ok(Box::new(remote::RemoteExtractor::spawn_postgres(
        worker, url, schema, table, chunk_rows,
    )?))
}
