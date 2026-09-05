//! Loading into Snowflake. The `Loader` trait is transport-independent;
//! v1's implementation drives the ADBC Snowflake driver inside the
//! on-demand worker process (`adbc_sidecar`). Tests use `MockLoader`.

pub mod adbc_sidecar;
pub mod duckdb_sidecar;
pub mod duckdb_sql;
pub mod protocol;
pub mod snowflake_sql;

use anyhow::Result;
use polars::prelude::DataFrame;

use crate::spec::Mode;
use crate::types::SnowflakeType;

/// One stream's load plan, resolved from the spec.
pub struct StreamPlan {
    pub database: Option<String>,
    pub schema: String,
    pub target_table: String,
    /// Ordered target columns — OUR DDL, never driver inference.
    pub columns: Vec<(String, SnowflakeType)>,
    pub mode: Mode,
    /// Target-side names (post-rename).
    pub primary_key: Vec<String>,
    /// Target-side cursor column and its type, for MERGE ordering and the
    /// post-commit MAX() read-back.
    pub update_key: Option<(String, SnowflakeType)>,
}

pub struct LoadReport {
    pub rows_written: u64,
    /// The cursor read back from the TARGET after an incremental commit,
    /// as the warehouse printed it.
    pub watermark_scalar: Option<String>,
}

pub trait Loader: Send {
    /// Creates the staging table for a stream.
    fn begin(&mut self, plan: &StreamPlan) -> Result<()>;

    /// Appends one cast chunk into staging.
    fn stage_chunk(&mut self, plan: &StreamPlan, chunk: &mut DataFrame) -> Result<u64>;

    /// Atomically publishes staging as the target and cleans up.
    fn commit(&mut self, plan: &StreamPlan) -> Result<LoadReport>;

    /// Best-effort cleanup after a failed or cancelled stream.
    fn abort(&mut self, plan: &StreamPlan) -> Result<()>;
}

/// A loader that records SQL and rows instead of talking to a warehouse —
/// the orchestrator's test double.
#[derive(Default)]
pub struct MockLoader {
    pub statements: Vec<String>,
    pub staged_rows: u64,
    pub fail_on_commit: bool,
}

impl Loader for MockLoader {
    fn begin(&mut self, plan: &StreamPlan) -> Result<()> {
        self.statements.push(snowflake_sql::create_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
            &plan.columns,
        ));
        Ok(())
    }

    fn stage_chunk(&mut self, _plan: &StreamPlan, chunk: &mut DataFrame) -> Result<u64> {
        let rows = chunk.height() as u64;
        self.staged_rows += rows;
        self.statements.push(format!("INGEST {rows}"));
        Ok(rows)
    }

    fn commit(&mut self, plan: &StreamPlan) -> Result<LoadReport> {
        if self.fail_on_commit {
            anyhow::bail!("mock commit failure");
        }
        if plan.mode == Mode::Incremental {
            let (update_key, _) = plan.update_key.as_ref().expect("validated");
            self.statements.push(snowflake_sql::merge(
                plan.database.as_deref(),
                &plan.schema,
                &plan.target_table,
                &plan.columns,
                &plan.primary_key,
                update_key,
            ));
            self.statements.push(snowflake_sql::drop_staging(
                plan.database.as_deref(),
                &plan.schema,
                &plan.target_table,
            ));
            return Ok(LoadReport {
                rows_written: self.staged_rows,
                watermark_scalar: Some("2026-01-07".to_owned()),
            });
        }
        self.statements.push(snowflake_sql::clone_swap(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
        ));
        self.statements.push(snowflake_sql::drop_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
        ));
        Ok(LoadReport {
            rows_written: self.staged_rows,
            watermark_scalar: None,
        })
    }

    fn abort(&mut self, plan: &StreamPlan) -> Result<()> {
        self.statements.push(snowflake_sql::drop_staging(
            plan.database.as_deref(),
            &plan.schema,
            &plan.target_table,
        ));
        Ok(())
    }
}
