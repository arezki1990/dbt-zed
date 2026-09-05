//! The cast plan: a stream spec applied to a concrete source schema,
//! producing the polars expressions that shape every chunk — projection,
//! casts (strict or lax), temporal parsing, renames — plus failure
//! accounting for the lax path.
//!
//! Temporal casts from strings MUST go through strptime: plain
//! String→Datetime `cast` is deprecated in polars and removed in 2.0.

use anyhow::{Result, anyhow, bail};
use polars::prelude::*;

use crate::spec::{ColumnSpec, StreamSpec};
use crate::types::SnowflakeType;

pub struct CastPlan {
    columns: Vec<PlannedColumn>,
}

struct PlannedColumn {
    source_name: String,
    target_name: String,
    source_dtype: DataType,
    target_type: SnowflakeType,
    /// The cast expression, or None for pass-through.
    expr: Option<Expr>,
    /// Lax columns get their new NULLs counted per chunk.
    counts_failures: bool,
}

/// Per-column lax-cast failures in one chunk: count plus up to
/// `sample_limit` offending source values (stringified).
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnFailures {
    pub column: String,
    pub count: u64,
    pub samples: Vec<String>,
}

pub struct CastOutcome {
    pub df: DataFrame,
    pub failures: Vec<ColumnFailures>,
}

impl CastPlan {
    /// Plans the stream against the schema the connector probed. Errors
    /// name the column and the reason — they surface directly in the UI.
    pub fn build(source_schema: &Schema, stream: &StreamSpec) -> Result<CastPlan> {
        let selected: Vec<(String, DataType)> = source_schema
            .iter()
            .map(|(name, dtype)| (name.to_string(), dtype.clone()))
            .filter(|(name, _)| match &stream.select {
                Some(select) if !select.include.is_empty() => {
                    select.include.iter().any(|inc| inc == name)
                }
                Some(select) => !select.exclude.iter().any(|ex| ex == name),
                None => true,
            })
            .collect();

        if let Some(select) = &stream.select {
            for wanted in &select.include {
                if !source_schema.iter().any(|(name, _)| name.as_str() == wanted) {
                    bail!(
                        "stream {:?}: selected column {wanted:?} does not exist in the source",
                        stream.name
                    );
                }
            }
        }
        for rule in &stream.columns {
            if !selected.iter().any(|(name, _)| name == &rule.name) {
                bail!(
                    "stream {:?}: column rule for {:?} matches no selected source column",
                    stream.name,
                    rule.name
                );
            }
        }
        if let Some(update_key) = &stream.update_key {
            if !source_schema
                .iter()
                .any(|(name, _)| name.as_str() == update_key)
            {
                bail!(
                    "stream {:?}: update_key {update_key:?} does not exist in the source",
                    stream.name
                );
            }
        }

        let columns = selected
            .into_iter()
            .map(|(source_name, source_dtype)| {
                let rule = stream.columns.iter().find(|rule| rule.name == source_name);
                Self::plan_column(&stream.name, source_name, source_dtype, rule)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(CastPlan { columns })
    }

    fn plan_column(
        stream: &str,
        source_name: String,
        source_dtype: DataType,
        rule: Option<&ColumnSpec>,
    ) -> Result<PlannedColumn> {
        let strict = rule.and_then(|rule| rule.strict).unwrap_or(false);
        let rename = rule.and_then(|rule| rule.rename.clone());
        let target_name = rename.unwrap_or_else(|| source_name.clone());

        let (target_type, expr, counts_failures) = match rule.and_then(|rule| rule.cast.clone()) {
            None => (
                SnowflakeType::from_polars(&source_dtype),
                None,
                false,
            ),
            Some(cast) => match cast.polars_dtype() {
                // VARIANT: keep the source dtype; the loader hands nested
                // data to Snowflake as-is.
                None => (cast, None, false),
                Some(dtype) if dtype == source_dtype => (cast, None, false),
                Some(dtype) => {
                    let parse = rule.and_then(|rule| rule.parse.clone());
                    let expr = build_cast_expr(
                        stream,
                        &source_name,
                        &source_dtype,
                        &dtype,
                        parse.as_deref(),
                        strict,
                    )?;
                    (cast, Some(expr), !strict)
                }
            },
        };

        Ok(PlannedColumn {
            source_name,
            target_name,
            source_dtype,
            target_type,
            expr,
            counts_failures,
        })
    }

    /// Applies the plan to one chunk.
    pub fn apply(&self, df: DataFrame, sample_limit: usize) -> Result<CastOutcome> {
        let nulls_before: Vec<u64> = self
            .columns
            .iter()
            .map(|column| {
                df.column(&column.source_name)
                    .map(|series| series.null_count() as u64)
                    .unwrap_or(0)
            })
            .collect();

        let exprs: Vec<Expr> = self
            .columns
            .iter()
            .map(|column| {
                let base = match &column.expr {
                    Some(expr) => expr.clone(),
                    None => col(&column.source_name),
                };
                base.alias(&column.target_name)
            })
            .collect();

        let out = df
            .clone()
            .lazy()
            .select(exprs)
            .collect()
            .map_err(|error| anyhow!("applying cast plan: {error}"))?;

        let mut failures = Vec::new();
        for (ix, column) in self.columns.iter().enumerate() {
            if !column.counts_failures {
                continue;
            }
            let nulls_after = out
                .column(&column.target_name)
                .map(|series| series.null_count() as u64)
                .unwrap_or(0);
            let count = nulls_after.saturating_sub(nulls_before[ix]);
            if count == 0 {
                continue;
            }
            // Offending source values: rows where the cast produced a NULL
            // the source didn't have.
            let samples = sample_failures(&df, &out, column, sample_limit)?;
            failures.push(ColumnFailures {
                column: column.target_name.clone(),
                count,
                samples,
            });
        }
        Ok(CastOutcome { df: out, failures })
    }

    /// The ordered (target column name, Snowflake type) list — target DDL
    /// order and the schema-drift fingerprint input.
    pub fn target_columns(&self) -> Vec<(String, SnowflakeType)> {
        self.columns
            .iter()
            .map(|column| (column.target_name.clone(), column.target_type.clone()))
            .collect()
    }
}

fn build_cast_expr(
    stream: &str,
    name: &str,
    source: &DataType,
    target: &DataType,
    parse: Option<&str>,
    strict: bool,
) -> Result<Expr> {
    let source_is_string = matches!(source, DataType::String);
    match target {
        DataType::Datetime(time_unit, time_zone) if source_is_string => {
            Ok(col(name).str().to_datetime(
                Some(*time_unit),
                time_zone.clone(),
                StrptimeOptions {
                    format: parse.map(Into::into),
                    strict,
                    ..Default::default()
                },
                lit("raise"),
            ))
        }
        DataType::Date if source_is_string => Ok(col(name).str().to_date(StrptimeOptions {
            format: parse.map(Into::into),
            strict,
            ..Default::default()
        })),
        DataType::Time if source_is_string => Ok(col(name).str().to_time(StrptimeOptions {
            format: parse.map(Into::into),
            strict,
            ..Default::default()
        })),
        DataType::Datetime(..) | DataType::Date | DataType::Time if parse.is_some() => Err(
            anyhow!("stream {stream:?}: column {name:?} has parse: but the source is not a string"),
        ),
        _ if strict => Ok(col(name).strict_cast(target.clone())),
        _ => Ok(col(name).cast(target.clone())),
    }
}

fn sample_failures(
    source: &DataFrame,
    cast: &DataFrame,
    column: &PlannedColumn,
    limit: usize,
) -> Result<Vec<String>> {
    let src = source
        .column(&column.source_name)
        .map_err(|error| anyhow!("{error}"))?;
    let dst = cast
        .column(&column.target_name)
        .map_err(|error| anyhow!("{error}"))?;
    let mut samples = Vec::new();
    for row in 0..source.height() {
        if samples.len() >= limit {
            break;
        }
        let was_null = src.get(row).map(|v| v.is_null()).unwrap_or(true);
        let is_null = dst.get(row).map(|v| v.is_null()).unwrap_or(true);
        if is_null && !was_null {
            if let Ok(value) = src.get(row) {
                samples.push(value.to_string().trim_matches('"').to_owned());
            }
        }
    }
    let _ = &column.source_dtype; // silences dead-field lint until DDL uses it
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ColumnSpec, Select, SourceObject, StreamSpec};
    use indexmap::IndexMap;

    fn stream(columns: Vec<ColumnSpec>, select: Option<Select>) -> StreamSpec {
        StreamSpec {
            name: "orders".into(),
            source: SourceObject::Table {
                schema: Some("public".into()),
                table: "orders".into(),
            },
            mode: None,
            primary_key: vec![],
            update_key: None,
            target_table: None,
            select,
            columns,
            extra: IndexMap::new(),
        }
    }

    fn rule(name: &str, cast: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.into(),
            cast: Some(cast.parse().unwrap()),
            strict: None,
            parse: None,
            rename: None,
        }
    }

    fn fixture() -> DataFrame {
        df![
            "id" => ["1", "2", "3", "4"],
            "amount" => ["10.5", "20.25", "not_a_number", "40"],
            "placed_at" => ["2026-01-01 10:00:00", "2026-01-02 11:30:00", "2026-01-03 09:15:00", "never"],
            "note" => ["ok", "fine", "bad amount", "bad date"],
        ]
        .unwrap()
    }

    #[test]
    fn plan_projects_casts_renames_and_counts() {
        let df = fixture();
        let mut parse_rule = rule("placed_at", "TIMESTAMP_NTZ");
        parse_rule.parse = Some("%Y-%m-%d %H:%M:%S".into());
        let mut renamed = rule("amount", "FLOAT");
        renamed.rename = Some("AMOUNT_EUR".into());
        let stream = stream(
            vec![rule("id", "NUMBER(10,0)"), renamed, parse_rule],
            Some(Select {
                include: vec!["id".into(), "amount".into(), "placed_at".into()],
                exclude: vec![],
            }),
        );
        let plan = CastPlan::build(&df.schema(), &stream).unwrap();
        let outcome = plan.apply(df, 8).unwrap();

        // Projection dropped "note"; rename applied.
        assert_eq!(
            outcome.df.get_column_names(),
            ["id", "AMOUNT_EUR", "placed_at"]
        );
        assert_eq!(outcome.df.column("id").unwrap().dtype(), &DataType::Int64);

        // Two lax failures, each with the offending source value sampled.
        let by_col: std::collections::HashMap<_, _> = outcome
            .failures
            .iter()
            .map(|failure| (failure.column.as_str(), failure))
            .collect();
        assert_eq!(by_col["AMOUNT_EUR"].count, 1);
        assert_eq!(by_col["AMOUNT_EUR"].samples, ["not_a_number"]);
        assert_eq!(by_col["placed_at"].count, 1);
        assert_eq!(by_col["placed_at"].samples, ["never"]);

        // DDL ordering reflects the plan.
        let targets = plan.target_columns();
        assert_eq!(targets[0].0, "id");
        assert_eq!(targets[1].0, "AMOUNT_EUR");
        assert_eq!(targets[1].1.to_string(), "FLOAT");
    }

    #[test]
    fn strict_cast_fails_with_context() {
        let df = fixture();
        let mut strict = rule("amount", "FLOAT");
        strict.strict = Some(true);
        let stream = stream(vec![strict], None);
        let plan = CastPlan::build(&df.schema(), &stream).unwrap();
        assert!(plan.apply(df, 8).is_err());
    }

    #[test]
    fn unknown_selected_column_errors_by_name() {
        let df = fixture();
        let stream = stream(
            vec![],
            Some(Select {
                include: vec!["id".into(), "ghost".into()],
                exclude: vec![],
            }),
        );
        let error = CastPlan::build(&df.schema(), &stream)
            .err()
            .expect("must fail")
            .to_string();
        assert!(error.contains("ghost"), "{error}");
    }

    #[test]
    fn variant_passthrough_keeps_dtype() {
        let df = df!["meta" => ["{\"a\":1}", "{}"]].unwrap();
        let stream = stream(vec![rule("meta", "VARIANT")], None);
        let plan = CastPlan::build(&df.schema(), &stream).unwrap();
        let outcome = plan.apply(df, 4).unwrap();
        assert_eq!(outcome.df.column("meta").unwrap().dtype(), &DataType::String);
        assert_eq!(plan.target_columns()[0].1.to_string(), "VARIANT");
        assert!(outcome.failures.is_empty());
    }
}
