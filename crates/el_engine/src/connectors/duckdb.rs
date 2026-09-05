//! DuckDB source connector — the zero-credential way to exercise the whole
//! extract→cast path against a real database. Opens the file read-only.
//!
//! Deliberately row-wise: duckdb-rs's `polars` feature pins polars 0.49
//! (incompatible with ours) and its Arrow output is arrow-rs, so typed
//! getters + manual Series building avoid every interop trap. Types
//! outside the native set are cast to VARCHAR in the SELECT list, where
//! the spec's `cast:` rules can still shape them exactly (DECIMAL text →
//! NUMBER(p,s) stays lossless).

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{NaiveDate, NaiveDateTime};
use duckdb::Connection;
use polars::prelude::*;

/// How each column is fetched from a row.
#[derive(Clone, Copy, PartialEq)]
enum Fetch {
    Bool,
    Int,
    Float,
    Text,
    Date,
    Timestamp,
}

struct Column {
    name: String,
    fetch: Fetch,
    /// The SELECT-list expression (identifier, or a VARCHAR cast for
    /// exotic types).
    select_expr: String,
}

pub struct DuckdbExtractor {
    connection: Connection,
    columns: Vec<Column>,
    relation: String,
    chunk_rows: usize,
    offset: usize,
    exhausted: bool,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl DuckdbExtractor {
    pub fn new(
        db_path: &std::path::Path,
        schema: Option<&str>,
        table: &str,
        chunk_rows: usize,
    ) -> Result<Self> {
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .map_err(|error| anyhow!("duckdb config: {error}"))?;
        let connection = Connection::open_with_flags(db_path, config)
            .with_context(|| format!("opening duckdb file {}", db_path.display()))?;

        let relation = match schema {
            Some(schema) => format!("{}.{}", quote_ident(schema), quote_ident(table)),
            None => quote_ident(table),
        };

        // DESCRIBE gives (column_name, column_type, …) for any relation.
        let mut statement = connection
            .prepare(&format!("DESCRIBE SELECT * FROM {relation}"))
            .with_context(|| format!("describing {relation}"))?;
        let described: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| anyhow!("describe {relation}: {error}"))?
            .collect::<std::result::Result<_, _>>()
            .map_err(|error| anyhow!("describe {relation}: {error}"))?;
        drop(statement);
        if described.is_empty() {
            bail!("{relation} has no columns");
        }

        let columns = described
            .into_iter()
            .map(|(name, type_name)| {
                let fetch = fetch_kind(&type_name);
                let ident = quote_ident(&name);
                let select_expr = match fetch {
                    // Exotic types (DECIMAL, HUGEINT, UUID, JSON, LIST, …)
                    // travel as text; spec casts restore exact types.
                    Fetch::Text if !is_native_text(&type_name) => {
                        format!("CAST({ident} AS VARCHAR) AS {ident}")
                    }
                    _ => ident,
                };
                Column {
                    name,
                    fetch,
                    select_expr,
                }
            })
            .collect();

        Ok(Self {
            connection,
            columns,
            relation,
            chunk_rows: chunk_rows.max(1),
            offset: 0,
            exhausted: false,
        })
    }

    fn polars_dtype(fetch: Fetch) -> DataType {
        match fetch {
            Fetch::Bool => DataType::Boolean,
            Fetch::Int => DataType::Int64,
            Fetch::Float => DataType::Float64,
            Fetch::Text => DataType::String,
            Fetch::Date => DataType::Date,
            Fetch::Timestamp => DataType::Datetime(TimeUnit::Microseconds, None),
        }
    }
}

fn is_native_text(type_name: &str) -> bool {
    let upper = type_name.to_ascii_uppercase();
    upper.starts_with("VARCHAR") || upper == "TEXT" || upper == "STRING"
}

fn fetch_kind(type_name: &str) -> Fetch {
    let upper = type_name.to_ascii_uppercase();
    let base = upper.split('(').next().unwrap_or(&upper).trim();
    match base {
        "BOOLEAN" | "BOOL" => Fetch::Bool,
        "TINYINT" | "SMALLINT" | "INTEGER" | "INT" | "BIGINT" | "UTINYINT" | "USMALLINT"
        | "UINTEGER" => Fetch::Int,
        "FLOAT" | "REAL" | "DOUBLE" => Fetch::Float,
        "DATE" => Fetch::Date,
        "TIMESTAMP" | "DATETIME" | "TIMESTAMP_S" | "TIMESTAMP_MS" | "TIMESTAMP_NS"
        | "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" => Fetch::Timestamp,
        // VARCHAR natively; everything else (DECIMAL, HUGEINT, UBIGINT,
        // UUID, JSON, BLOB, LIST, STRUCT, MAP, ENUM, INTERVAL…) via CAST.
        _ => Fetch::Text,
    }
}

const EPOCH: NaiveDate = match NaiveDate::from_ymd_opt(1970, 1, 1) {
    Some(date) => date,
    None => panic!("epoch"),
};

impl super::Extractor for DuckdbExtractor {
    fn schema(&mut self) -> Result<Schema> {
        Ok(Schema::from_iter(self.columns.iter().map(|column| {
            (
                column.name.as_str().into(),
                Self::polars_dtype(column.fetch),
            )
        })))
    }

    fn next_chunk(&mut self) -> Result<Option<DataFrame>> {
        if self.exhausted {
            return Ok(None);
        }
        let select_list = self
            .columns
            .iter()
            .map(|column| column.select_expr.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // ORDER BY ALL keeps LIMIT/OFFSET pagination deterministic.
        let sql = format!(
            "SELECT {select_list} FROM {} ORDER BY ALL LIMIT {} OFFSET {}",
            self.relation, self.chunk_rows, self.offset
        );

        enum Values {
            Bool(Vec<Option<bool>>),
            Int(Vec<Option<i64>>),
            Float(Vec<Option<f64>>),
            Text(Vec<Option<String>>),
            /// Days since epoch, cast to Date after building.
            Date(Vec<Option<i32>>),
            /// Microseconds since epoch, cast to Datetime after building.
            Timestamp(Vec<Option<i64>>),
        }

        let mut buffers: Vec<Values> = self
            .columns
            .iter()
            .map(|column| match column.fetch {
                Fetch::Bool => Values::Bool(Vec::new()),
                Fetch::Int => Values::Int(Vec::new()),
                Fetch::Float => Values::Float(Vec::new()),
                Fetch::Text => Values::Text(Vec::new()),
                Fetch::Date => Values::Date(Vec::new()),
                Fetch::Timestamp => Values::Timestamp(Vec::new()),
            })
            .collect();

        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| anyhow!("preparing chunk query: {error}"))?;
        let mut rows = statement
            .query([])
            .map_err(|error| anyhow!("querying {}: {error}", self.relation))?;
        let mut fetched = 0usize;
        while let Some(row) = rows.next().map_err(|error| anyhow!("row: {error}"))? {
            fetched += 1;
            for (ix, buffer) in buffers.iter_mut().enumerate() {
                match buffer {
                    Values::Bool(values) => values.push(
                        row.get::<_, Option<bool>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?,
                    ),
                    Values::Int(values) => values.push(
                        row.get::<_, Option<i64>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?,
                    ),
                    Values::Float(values) => values.push(
                        row.get::<_, Option<f64>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?,
                    ),
                    Values::Text(values) => values.push(
                        row.get::<_, Option<String>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?,
                    ),
                    Values::Date(values) => values.push(
                        row.get::<_, Option<NaiveDate>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?
                            .map(|date| (date - EPOCH).num_days() as i32),
                    ),
                    Values::Timestamp(values) => values.push(
                        row.get::<_, Option<NaiveDateTime>>(ix)
                            .map_err(|error| anyhow!("column {ix}: {error}"))?
                            .map(|dt| dt.and_utc().timestamp_micros()),
                    ),
                }
            }
        }
        drop(rows);
        drop(statement);

        if fetched == 0 {
            self.exhausted = true;
            return Ok(None);
        }
        if fetched < self.chunk_rows {
            self.exhausted = true;
        }
        self.offset += fetched;

        let series: Vec<polars::prelude::Column> = self
            .columns
            .iter()
            .zip(buffers)
            .map(|(column, buffer)| -> Result<polars::prelude::Column> {
                let name = column.name.as_str();
                let series = match buffer {
                    Values::Bool(values) => Series::new(name.into(), values),
                    Values::Int(values) => Series::new(name.into(), values),
                    Values::Float(values) => Series::new(name.into(), values),
                    Values::Text(values) => Series::new(name.into(), values),
                    Values::Date(values) => Series::new(name.into(), values)
                        .cast(&DataType::Date)
                        .map_err(|error| anyhow!("date column {name}: {error}"))?,
                    Values::Timestamp(values) => Series::new(name.into(), values)
                        .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
                        .map_err(|error| anyhow!("timestamp column {name}: {error}"))?,
                };
                Ok(series.into())
            })
            .collect::<Result<_>>()?;

        let df = DataFrame::new(fetched, series).map_err(|error| anyhow!("building chunk: {error}"))?;
        Ok(Some(df))
    }
}

/// Creates the small demo database the scaffold and tests use: five
/// orders with ints, text, bools, a DECIMAL, dates and timestamps —
/// enough to exercise every fetch kind.
pub fn create_demo_db(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let connection = Connection::open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    connection
        .execute_batch(
            r#"
CREATE TABLE main.demo_orders (
    id BIGINT,
    customer VARCHAR,
    paid BOOLEAN,
    amount DECIMAL(18,2),
    ordered_on DATE,
    created_at TIMESTAMP
);
INSERT INTO main.demo_orders VALUES
  (1, 'acme',    true,  120.50, DATE '2026-01-03', TIMESTAMP '2026-01-03 09:12:00'),
  (2, 'globex',  false,  75.00, DATE '2026-01-04', TIMESTAMP '2026-01-04 14:03:10'),
  (3, 'initech', true,  310.25, DATE '2026-01-05', TIMESTAMP '2026-01-05 08:41:33'),
  (4, 'stark',   true,     NULL, NULL,             TIMESTAMP '2026-01-06 17:20:05'),
  (5, 'wayne',   false,  42.10, DATE '2026-01-07', NULL);
"#,
        )
        .map_err(|error| anyhow!("seeding demo db: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::Extractor as _;
    use super::*;

    fn seed(path: &std::path::Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                r#"
CREATE TABLE main.people (
    id BIGINT,
    name VARCHAR,
    active BOOLEAN,
    balance DECIMAL(18,2),
    score DOUBLE,
    born DATE,
    created TIMESTAMP
);
INSERT INTO main.people VALUES
  (1, 'ada',  true,  1234.56, 9.5, DATE '1990-03-01', TIMESTAMP '2026-01-01 10:00:00'),
  (2, 'grace', false, NULL,   NULL, NULL,             TIMESTAMP '2026-01-02 11:30:00'),
  (3, 'alan', true,  -10.00,  3.25, DATE '1985-12-31', NULL);
"#,
            )
            .unwrap();
    }

    #[test]
    fn schema_types_and_values() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.duckdb");
        seed(&db);

        let mut extractor = DuckdbExtractor::new(&db, Some("main"), "people", 100).unwrap();
        let schema = extractor.schema().unwrap();
        let dtype = |name: &str| schema.get(name).unwrap().clone();
        assert_eq!(dtype("id"), DataType::Int64);
        assert_eq!(dtype("name"), DataType::String);
        assert_eq!(dtype("active"), DataType::Boolean);
        assert_eq!(dtype("balance"), DataType::String, "DECIMAL travels as text");
        assert_eq!(dtype("score"), DataType::Float64);
        assert_eq!(dtype("born"), DataType::Date);
        assert_eq!(
            dtype("created"),
            DataType::Datetime(TimeUnit::Microseconds, None)
        );

        let chunk = extractor.next_chunk().unwrap().unwrap();
        assert_eq!(chunk.height(), 3);
        assert_eq!(chunk.column("balance").unwrap().null_count(), 1);
        assert_eq!(chunk.column("born").unwrap().null_count(), 1);
        // ORDER BY ALL → id ascending.
        let ids: Vec<Option<i64>> = chunk
            .column("id")
            .unwrap()
            .i64()
            .unwrap()
            .iter()
            .collect();
        assert_eq!(ids, [Some(1), Some(2), Some(3)]);
        assert!(extractor.next_chunk().unwrap().is_none());
    }

    #[test]
    fn chunked_pagination_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.duckdb");
        seed(&db);

        let mut extractor = DuckdbExtractor::new(&db, None, "people", 2).unwrap();
        let first = extractor.next_chunk().unwrap().unwrap();
        let second = extractor.next_chunk().unwrap().unwrap();
        assert_eq!(first.height(), 2);
        assert_eq!(second.height(), 1);
        assert!(extractor.next_chunk().unwrap().is_none());
    }

    #[test]
    fn decimal_text_casts_exactly_through_the_plan() {
        use crate::spec::{ColumnSpec, SourceObject, StreamSpec};
        use indexmap::IndexMap;

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.duckdb");
        seed(&db);
        let mut extractor = DuckdbExtractor::new(&db, None, "people", 100).unwrap();
        let schema = extractor.schema().unwrap();
        let chunk = extractor.next_chunk().unwrap().unwrap();

        let stream = StreamSpec {
            name: "people".into(),
            source: SourceObject::Table {
                schema: None,
                table: "people".into(),
            },
            mode: None,
            primary_key: vec![],
            update_key: None,
            target_table: None,
            select: None,
            columns: vec![ColumnSpec {
                name: "balance".into(),
                cast: Some("NUMBER(18,2)".parse().unwrap()),
                strict: None,
                parse: None,
                rename: None,
            }],
            extra: IndexMap::new(),
        };
        let plan = crate::cast::CastPlan::build(&schema, &stream).unwrap();
        let outcome = plan.apply(chunk, 8).unwrap();
        assert!(
            outcome.failures.is_empty(),
            "decimal text → NUMBER must be lossless: {:?}",
            outcome.failures
        );
        assert_eq!(
            outcome.df.column("balance").unwrap().dtype(),
            &DataType::Decimal(18, 2)
        );
    }
}
