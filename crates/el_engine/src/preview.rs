//! The bounded preview the IDE calls: extract up to `limit` rows through
//! the SAME connector and cast code the real run uses, and return
//! pre-stringified rows plus the lax-cast failures — so the mapping editor
//! can show exactly which values a cast would lose, before anything ever
//! touches a warehouse.

use anyhow::{Context as _, Result};
use polars::prelude::*;

use crate::cast::{CastPlan, ColumnFailures};
use crate::progress::CancelFlag;
use crate::spec::{Pipeline, StreamSpec};

#[derive(Clone, Debug)]
pub struct PreviewColumn {
    pub name: SharedStringLike,
    /// The probed source dtype, e.g. "str", "i64".
    pub source_dtype: String,
    /// The resolved Snowflake target type, e.g. "NUMBER(38,0)".
    pub target_type: String,
}

/// Plain String — the engine never depends on gpui, so the UI converts.
pub type SharedStringLike = String;

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub columns: Vec<PreviewColumn>,
    pub rows: Vec<Vec<String>>,
    pub failures: Vec<ColumnFailures>,
}

/// Previews one stream of the pipeline. Blocking — call from a background
/// thread.
pub fn preview_stream(
    project_root: &std::path::Path,
    pipeline: &Pipeline,
    stream_name: &str,
    limit: usize,
    cancel: &CancelFlag,
) -> Result<PreviewResult> {
    let stream = pipeline
        .streams
        .iter()
        .find(|stream| stream.name == stream_name)
        .with_context(|| format!("no stream named {stream_name:?} in the pipeline"))?;
    preview(project_root, stream, limit, cancel)
}

fn preview(
    project_root: &std::path::Path,
    stream: &StreamSpec,
    limit: usize,
    cancel: &CancelFlag,
) -> Result<PreviewResult> {
    let limit = limit.clamp(1, 10_000);
    let mut extractor = crate::connectors::make_extractor(project_root, stream, limit)?;
    let schema = extractor.schema()?;
    let plan = CastPlan::build(&schema, stream)?;

    if cancel.is_cancelled() {
        anyhow::bail!("cancelled");
    }
    let chunk = extractor
        .next_chunk()?
        .unwrap_or_else(|| DataFrame::empty_with_schema(&schema));
    let outcome = plan.apply(chunk, 32)?;

    let source_types: std::collections::HashMap<String, String> = schema
        .iter()
        .map(|(name, dtype)| (name.to_string(), dtype.to_string()))
        .collect();

    let columns = plan
        .target_columns()
        .into_iter()
        .zip(outcome.df.columns())
        .map(|((name, target_type), series)| PreviewColumn {
            source_dtype: source_types
                .get(series.name().as_str())
                .cloned()
                .unwrap_or_else(|| series.dtype().to_string()),
            target_type: target_type.to_string(),
            name,
        })
        .collect();

    let rows = stringify_rows(&outcome.df, limit);
    Ok(PreviewResult {
        columns,
        rows,
        failures: outcome.failures,
    })
}

/// Bounded cell width keeps a pathological value from flooding the UI.
const MAX_CELL: usize = 200;

fn stringify_rows(df: &DataFrame, limit: usize) -> Vec<Vec<String>> {
    let height = df.height().min(limit);
    let mut rows = Vec::with_capacity(height);
    for row in 0..height {
        let mut cells = Vec::with_capacity(df.width());
        for column in df.columns() {
            let value = column
                .get(row)
                .map(|value| {
                    if value.is_null() {
                        String::new()
                    } else {
                        let text = value.to_string();
                        let text = text.trim_matches('"');
                        let mut text = text.to_owned();
                        if text.len() > MAX_CELL {
                            text.truncate(MAX_CELL);
                            text.push('…');
                        }
                        text
                    }
                })
                .unwrap_or_default();
            cells.push(value);
        }
        rows.push(cells);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ColumnSpec, FileFormat, SourceObject};
    use indexmap::IndexMap;
    use std::io::Write as _;

    #[test]
    fn preview_runs_the_real_cast_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = std::fs::File::create(dir.path().join("d.csv")).unwrap();
        writeln!(file, "id,amount").unwrap();
        writeln!(file, "1,10.5").unwrap();
        writeln!(file, "2,oops").unwrap();

        let pipeline = crate::spec::Pipeline {
            version: 1,
            pipeline: "t".into(),
            source: "local".into(),
            target: crate::spec::TargetSpec {
                connection: "wh".into(),
                database: None,
                schema: "RAW".into(),
                table: None,
            },
            defaults: None,
            streams: vec![crate::spec::StreamSpec {
                name: "d".into(),
                source: SourceObject::Path {
                    path: "d.csv".into(),
                    format: FileFormat::Csv,
                    csv: None,
                },
                mode: None,
                primary_key: vec![],
                update_key: None,
                target_table: None,
                select: None,
                columns: vec![ColumnSpec {
                    name: "amount".into(),
                    cast: Some("FLOAT".parse().unwrap()),
                    strict: None,
                    parse: None,
                    rename: None,
                }],
                extra: IndexMap::new(),
            }],
            canvas: None,
            extra: IndexMap::new(),
        };

        let result = preview_stream(
            dir.path(),
            &pipeline,
            "d",
            100,
            &crate::progress::CancelFlag::default(),
        )
        .unwrap();

        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.columns[1].target_type, "FLOAT");
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[1][1], "", "failed cast renders as empty cell");
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].samples, ["oops"]);
    }
}
