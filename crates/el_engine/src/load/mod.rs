//! Loading into Snowflake. The `Loader` trait is transport-independent;
//! v1's implementation drives the ADBC Snowflake driver inside the
//! on-demand worker process (`adbc_sidecar`). Tests use `MockLoader`.

pub mod adbc_sidecar;
pub mod protocol;
pub mod snowflake_sql;

use anyhow::Result;
use polars::prelude::DataFrame;

use crate::types::SnowflakeType;

/// One stream's load plan, resolved from the spec.
pub struct StreamPlan {
    pub database: Option<String>,
    pub schema: String,
    pub target_table: String,
    /// Ordered target columns — OUR DDL, never driver inference.
    pub columns: Vec<(String, SnowflakeType)>,
}

pub struct LoadReport {
    pub rows_written: u64,
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
