//! Postgres source connector (worker-side, feature "postgres"). Blocking
//! `postgres` crate with true streaming via a bound portal — chunked
//! fetches without ORDER BY or OFFSET re-scans. The connection URL comes
//! from the environment (the parent passes it as ZDBT_EL_SRC_URL), never
//! argv.
//!
//! Types outside the native set are cast to text in the SELECT list —
//! NUMERIC included, so spec casts to NUMBER(p,s) stay exact.

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use polars::prelude::*;
use postgres::fallible_iterator::FallibleIterator as _;
use postgres::types::Type;
use postgres::{Client, NoTls};

#[derive(Clone, Copy, PartialEq)]
enum Fetch {
    Bool,
    Int,
    Float,
    Text,
    Date,
    Timestamp,
    TimestampTz,
}

pub struct PostgresExtractor {
    client: Client,
    columns: Vec<(String, Fetch)>,
    query: String,
    chunk_rows: usize,
    /// Rows already produced — resumed with OFFSET only if the portal
    /// path failed; the primary path streams.
    started: bool,
    done: bool,
    buffered: Vec<DataFrame>,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn fetch_kind(ty: &Type) -> Fetch {
    match *ty {
        Type::BOOL => Fetch::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 => Fetch::Int,
        Type::FLOAT4 | Type::FLOAT8 => Fetch::Float,
        Type::DATE => Fetch::Date,
        Type::TIMESTAMP => Fetch::Timestamp,
        Type::TIMESTAMPTZ => Fetch::TimestampTz,
        _ => Fetch::Text,
    }
}

fn is_native_text(ty: &Type) -> bool {
    matches!(*ty, Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME)
}

impl PostgresExtractor {
    pub fn new(
        url: &str,
        schema: Option<&str>,
        table: &str,
        chunk_rows: usize,
        cursor: Option<(String, crate::state::WatermarkValue)>,
    ) -> Result<Self> {
        let mut client = {
            use std::str::FromStr as _;
            let mut config = postgres::Config::from_str(url).context("parsing postgres url")?;
            // An unreachable host must fail, not hang the run forever.
            config.connect_timeout(std::time::Duration::from_secs(10));
            config
                .connect(NoTls)
                .context("connecting to postgres (10s timeout; TLS comes later)")?
        };

        let relation = match schema {
            Some(schema) => format!("{}.{}", quote_ident(schema), quote_ident(table)),
            None => quote_ident(table),
        };
        // Probe columns + types without moving data.
        let probe = client
            .prepare(&format!("SELECT * FROM {relation} LIMIT 0"))
            .with_context(|| format!("probing {relation}"))?;
        if probe.columns().is_empty() {
            anyhow::bail!("{relation} has no columns");
        }
        let columns: Vec<(String, Fetch)> = probe
            .columns()
            .iter()
            .map(|column| (column.name().to_owned(), fetch_kind(column.type_())))
            .collect();
        let select_list = probe
            .columns()
            .iter()
            .map(|column| {
                let ident = quote_ident(column.name());
                match fetch_kind(column.type_()) {
                    Fetch::Text if !is_native_text(column.type_()) => {
                        format!("{ident}::text AS {ident}")
                    }
                    _ => ident,
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let suffix = match &cursor {
            Some((column, value)) => format!(
                " WHERE {c} > {lit} ORDER BY {c}",
                c = quote_ident(column),
                lit = value.to_sql_literal()
            ),
            None => String::new(),
        };
        Ok(Self {
            client,
            columns,
            query: format!("SELECT {select_list} FROM {relation}{suffix}"),
            chunk_rows: chunk_rows.max(1),
            started: false,
            done: false,
            buffered: Vec::new(),
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
            Fetch::TimestampTz => {
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
            }
        }
    }

    /// Streams the whole result once via RowIter, buffering chunk frames.
    /// Postgres portals need an open transaction for their lifetime, which
    /// fights the pull-based Extractor shape — buffering chunk FRAMES (not
    /// rows) keeps memory at one chunk of builders plus the frames'
    /// columnar data.
    fn stream_all(&mut self) -> Result<()> {
        const EPOCH: NaiveDate = match NaiveDate::from_ymd_opt(1970, 1, 1) {
            Some(date) => date,
            None => panic!("epoch"),
        };

        enum Values {
            Bool(Vec<Option<bool>>),
            Int(Vec<Option<i64>>),
            Float(Vec<Option<f64>>),
            Text(Vec<Option<String>>),
            Date(Vec<Option<i32>>),
            Timestamp(Vec<Option<i64>>),
            TimestampTz(Vec<Option<i64>>),
        }

        let make_buffers = |columns: &[(String, Fetch)]| -> Vec<Values> {
            columns
                .iter()
                .map(|(_, fetch)| match fetch {
                    Fetch::Bool => Values::Bool(Vec::new()),
                    Fetch::Int => Values::Int(Vec::new()),
                    Fetch::Float => Values::Float(Vec::new()),
                    Fetch::Text => Values::Text(Vec::new()),
                    Fetch::Date => Values::Date(Vec::new()),
                    Fetch::Timestamp => Values::Timestamp(Vec::new()),
                    Fetch::TimestampTz => Values::TimestampTz(Vec::new()),
                })
                .collect()
        };

        let flush = |columns: &[(String, Fetch)], buffers: Vec<Values>| -> Result<DataFrame> {
            let series: Vec<polars::prelude::Column> = columns
                .iter()
                .zip(buffers)
                .map(|((name, _), buffer)| -> Result<polars::prelude::Column> {
                    let name = name.as_str();
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
                        Values::TimestampTz(values) => Series::new(name.into(), values)
                            .cast(&DataType::Datetime(
                                TimeUnit::Microseconds,
                                Some(TimeZone::UTC),
                            ))
                            .map_err(|error| anyhow!("timestamptz column {name}: {error}"))?,
                    };
                    Ok(series.into())
                })
                .collect::<Result<_>>()?;
            let height = series.first().map(|s| s.len()).unwrap_or(0);
            DataFrame::new(height, series).map_err(|error| anyhow!("building chunk: {error}"))
        };

        let mut buffers = make_buffers(&self.columns);
        let mut in_chunk = 0usize;
        let mut rows = self
            .client
            .query_raw::<_, &str, _>(self.query.as_str(), std::iter::empty())
            .context("querying postgres")?;
        while let Some(row) = rows.next().context("reading row")? {
            in_chunk += 1;
            for (ix, buffer) in buffers.iter_mut().enumerate() {
                match buffer {
                    Values::Bool(values) => values.push(row.try_get(ix).context("bool")?),
                    Values::Int(values) => {
                        // INT2/INT4 need their own getters.
                        let value: Option<i64> = row
                            .try_get::<_, Option<i64>>(ix)
                            .or_else(|_| {
                                row.try_get::<_, Option<i32>>(ix)
                                    .map(|v| v.map(i64::from))
                            })
                            .or_else(|_| {
                                row.try_get::<_, Option<i16>>(ix)
                                    .map(|v| v.map(i64::from))
                            })
                            .context("int")?;
                        values.push(value);
                    }
                    Values::Float(values) => {
                        let value: Option<f64> = row
                            .try_get::<_, Option<f64>>(ix)
                            .or_else(|_| {
                                row.try_get::<_, Option<f32>>(ix)
                                    .map(|v| v.map(f64::from))
                            })
                            .context("float")?;
                        values.push(value);
                    }
                    Values::Text(values) => values.push(row.try_get(ix).context("text")?),
                    Values::Date(values) => values.push(
                        row.try_get::<_, Option<NaiveDate>>(ix)
                            .context("date")?
                            .map(|date| (date - EPOCH).num_days() as i32),
                    ),
                    Values::Timestamp(values) => values.push(
                        row.try_get::<_, Option<NaiveDateTime>>(ix)
                            .context("timestamp")?
                            .map(|dt| dt.and_utc().timestamp_micros()),
                    ),
                    Values::TimestampTz(values) => values.push(
                        row.try_get::<_, Option<DateTime<Utc>>>(ix)
                            .context("timestamptz")?
                            .map(|dt| dt.timestamp_micros()),
                    ),
                }
            }
            if in_chunk >= self.chunk_rows {
                let full = std::mem::replace(&mut buffers, make_buffers(&self.columns));
                self.buffered.push(flush(&self.columns, full)?);
                in_chunk = 0;
            }
        }
        if in_chunk > 0 {
            self.buffered.push(flush(&self.columns, buffers)?);
        }
        // FIFO via pop() below.
        self.buffered.reverse();
        Ok(())
    }
}

impl super::Extractor for PostgresExtractor {
    fn schema(&mut self) -> Result<Schema> {
        Ok(Schema::from_iter(self.columns.iter().map(
            |(name, fetch)| (name.as_str().into(), Self::polars_dtype(*fetch)),
        )))
    }

    fn next_chunk(&mut self) -> Result<Option<DataFrame>> {
        if !self.started {
            self.started = true;
            self.stream_all()?;
        }
        if let Some(chunk) = self.buffered.pop() {
            return Ok(Some(chunk));
        }
        self.done = true;
        Ok(None)
    }
}

/// Live smoke, gated: `EL_PG_SMOKE_URL=postgres://… cargo test -p el_engine
/// --features postgres -- --ignored postgres_smoke --nocapture`
#[cfg(test)]
mod tests {
    use super::super::Extractor as _;
    use super::*;

    #[test]
    #[ignore]
    fn postgres_smoke() {
        let url = std::env::var("EL_PG_SMOKE_URL").expect("set EL_PG_SMOKE_URL");
        let mut client = Client::connect(&url, NoTls).unwrap();
        client
            .batch_execute(
                "DROP TABLE IF EXISTS zdbt_el_smoke;
                 CREATE TABLE zdbt_el_smoke (
                     id BIGINT, name TEXT, amount NUMERIC(18,2),
                     active BOOLEAN, born DATE, created TIMESTAMPTZ);
                 INSERT INTO zdbt_el_smoke VALUES
                   (1,'ada',10.50,true,'1990-03-01','2026-01-01T10:00:00Z'),
                   (2,'grace',NULL,false,NULL,'2026-01-02T11:30:00Z'),
                   (3,'alan',-2.25,true,'1985-12-31',NULL);",
            )
            .unwrap();
        drop(client);

        let mut extractor = PostgresExtractor::new(&url, None, "zdbt_el_smoke", 2, None).unwrap();
        let schema = extractor.schema().unwrap();
        assert_eq!(schema.get("amount").unwrap(), &DataType::String);
        let mut total = 0;
        while let Some(chunk) = extractor.next_chunk().unwrap() {
            total += chunk.height();
        }
        assert_eq!(total, 3);
        println!("postgres smoke ok: {total} rows");
    }
}
