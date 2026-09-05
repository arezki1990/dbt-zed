//! zdbt's embedded EL engine (P0 spike).
//!
//! This crate will grow the full extract→cast→load pipeline; the spike
//! proves the two load-bearing claims before anything else is built:
//! that polars' casting maps onto the per-column spec we designed
//! (strict vs lax, failure counting), and — behind the `adbc` feature —
//! that the ADBC Snowflake driver dylib loads and hands over a working
//! function table in a plain process.

use anyhow::{Context as _, Result};
use polars::prelude::*;

/// How one column should be cast, the spike-sized subset of the planned
/// `ColumnSpec`.
pub struct CastRule {
    pub column: String,
    pub to: DataType,
    /// Strict fails the whole stream on the first uncastable value; lax
    /// turns failures into NULLs, which the engine counts and reports.
    pub strict: bool,
    /// chrono format for string→temporal parsing (the spec's `parse:`).
    /// Plain `cast` from String to Datetime is deprecated in polars and
    /// parses almost nothing — temporal casts MUST go through strptime.
    pub parse: Option<String>,
}

/// The outcome of casting one chunk: the shaped frame plus, per lax-cast
/// column, how many values could not be represented in the target type.
pub struct CastOutcome {
    pub df: DataFrame,
    pub lax_failures: Vec<(String, u64)>,
}

/// Applies the rules to one chunk. Mirrors the planned engine shape:
/// strict rules go through `strict_cast` (error), lax rules through
/// `cast` (NULL on failure) with the new NULLs counted against the
/// source column's own NULL count.
pub fn apply_casts(df: DataFrame, rules: &[CastRule]) -> Result<CastOutcome> {
    let before: Vec<(String, u64)> = rules
        .iter()
        .filter(|rule| !rule.strict)
        .map(|rule| {
            let nulls = df
                .column(&rule.column)
                .map(|column| column.null_count() as u64)
                .unwrap_or(0);
            (rule.column.clone(), nulls)
        })
        .collect();

    let exprs: Vec<Expr> = rules
        .iter()
        .map(|rule| match (&rule.to, &rule.parse) {
            (DataType::Datetime(time_unit, time_zone), Some(format)) => col(&rule.column)
                .str()
                .to_datetime(
                    Some(*time_unit),
                    time_zone.clone(),
                    StrptimeOptions {
                        format: Some(format.as_str().into()),
                        strict: rule.strict,
                        ..Default::default()
                    },
                    lit("raise"),
                ),
            _ if rule.strict => col(&rule.column).strict_cast(rule.to.clone()),
            _ => col(&rule.column).cast(rule.to.clone()),
        })
        .collect();

    let out = df
        .lazy()
        .with_columns(exprs)
        .collect()
        .map_err(|error| anyhow::anyhow!("applying cast plan: {error}"))?;

    let lax_failures = before
        .into_iter()
        .map(|(name, nulls_before)| {
            let nulls_after = out
                .column(&name)
                .map(|column| column.null_count() as u64)
                .unwrap_or(0);
            (name, nulls_after.saturating_sub(nulls_before))
        })
        .filter(|(_, count)| *count > 0)
        .collect();

    Ok(CastOutcome {
        df: out,
        lax_failures,
    })
}

/// Loads a CSV into a DataFrame — the spike's stand-in for the file
/// connector, using the same reader the real one will.
pub fn read_csv(path: &std::path::Path) -> Result<DataFrame> {
    let path = path.to_str().context("non-utf8 path")?;
    LazyCsvReader::new(path.into())
        .with_infer_schema_length(Some(100))
        .finish()
        .map_err(|error| anyhow::anyhow!("opening csv: {error}"))?
        .collect()
        .map_err(|error| anyhow::anyhow!("reading csv: {error}"))
}

#[cfg(feature = "adbc")]
pub mod adbc_check {
    //! Spike 3: prove the Go-built Snowflake driver dylib dlopens in a
    //! plain (non-GPUI) process and exposes a live ADBC function table.
    //! No warehouse and no credentials — constructing the database handle
    //! exercises dlopen, the entrypoint, and the driver's option plumbing.

    use adbc_core::Driver as _;
    use adbc_core::options::AdbcVersion;
    use adbc_driver_manager::ManagedDriver;

    pub fn handshake(driver_path: &std::path::Path) -> anyhow::Result<String> {
        let mut driver =
            ManagedDriver::load_dynamic_from_filename(driver_path, None, AdbcVersion::V110)
                .map_err(|error| anyhow::anyhow!("dlopen/init failed: {error:?}"))?;
        let _database = driver
            .new_database()
            .map_err(|error| anyhow::anyhow!("new_database failed: {error:?}"))?;
        Ok(format!(
            "ADBC driver loaded and database handle constructed from {}",
            driver_path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const CSV: &str = "\
id,amount,placed_at,note
1,10.50,2026-01-01 10:00:00,ok
2,20.25,2026-01-02 11:30:00,fine
3,not_a_number,2026-01-03 09:15:00,bad amount
4,40.00,never,bad date
";

    fn fixture() -> (tempfile::TempDir, DataFrame) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.csv");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(CSV.as_bytes()).unwrap();
        let df = read_csv(&path).unwrap();
        (dir, df)
    }

    #[test]
    fn lax_cast_nulls_and_counts_failures() {
        let (_dir, df) = fixture();
        let outcome = apply_casts(
            df,
            &[
                CastRule {
                    column: "id".into(),
                    to: DataType::Int64,
                    strict: true,
                    parse: None,
                },
                CastRule {
                    column: "amount".into(),
                    to: DataType::Float64,
                    strict: false,
                    parse: None,
                },
            ],
        )
        .unwrap();

        // Row 3's "not_a_number" became NULL and was counted; nothing else.
        assert_eq!(outcome.lax_failures, vec![("amount".into(), 1)]);
        assert_eq!(outcome.df.column("amount").unwrap().null_count(), 1);
        assert_eq!(
            outcome.df.column("id").unwrap().dtype(),
            &DataType::Int64
        );
    }

    #[test]
    fn strict_cast_fails_the_chunk() {
        let (_dir, df) = fixture();
        let result = apply_casts(
            df,
            &[CastRule {
                column: "amount".into(),
                to: DataType::Float64,
                strict: true,
                parse: None,
            }],
        );
        assert!(result.is_err(), "strict cast over bad data must error");
    }

    #[test]
    fn temporal_parse_via_lax_cast() {
        let (_dir, df) = fixture();
        let outcome = apply_casts(
            df,
            &[CastRule {
                column: "placed_at".into(),
                to: DataType::Datetime(TimeUnit::Microseconds, None),
                strict: false,
                parse: Some("%Y-%m-%d %H:%M:%S".into()),
            }],
        )
        .unwrap();
        // "never" fails; three real timestamps survive.
        assert_eq!(outcome.lax_failures, vec![("placed_at".into(), 1)]);
        assert_eq!(
            outcome.df.column("placed_at").unwrap().null_count(),
            1
        );
    }

    /// Spike 3, run manually:
    /// `EL_ADBC_DRIVER_PATH=/path/libadbc_driver_snowflake.dylib \
    ///  cargo test -p el_engine --features adbc -- --ignored adbc`
    #[cfg(feature = "adbc")]
    #[test]
    #[ignore]
    fn adbc_driver_handshake() {
        let path = std::env::var("EL_ADBC_DRIVER_PATH").expect("set EL_ADBC_DRIVER_PATH");
        let message = super::adbc_check::handshake(std::path::Path::new(&path)).unwrap();
        println!("{message}");
    }
}
