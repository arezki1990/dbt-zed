//! zdbt-el-worker: the on-demand connector process. Extracts database
//! sources chunk by chunk, writing each chunk as an Arrow IPC file and
//! announcing it as one JSON line on stdout — the protocol
//! `el_engine::connectors::remote` speaks from the app side.
//!
//! Subcommands:
//!   extract  --kind duckdb --db <path> [--schema <s>] --table <t>
//!            --chunk-rows <n> --out-dir <dir>
//!   seed-demo <path>   create a small demo DuckDB database

mod duckdb_loader;
mod snowflake_loader;

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use el_engine::connectors::remote::{RemoteExtractor, WorkerEvent};
use el_engine::connectors::{Extractor as _, duckdb::DuckdbExtractor};
use polars::prelude::{IpcWriter, SerWriter as _};

// el_engine re-exports polars? It doesn't; depend through its public API.
use el_engine::polars;

fn emit(event: &WorkerEvent) {
    if let Ok(line) = serde_json::to_string(event) {
        println!("{line}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("extract") => extract(&args[1..]),
        Some("seed-demo") => seed_demo(&args[1..]),
        Some("snowflake-loader") => snowflake_loader::serve(),
        Some("duckdb-loader") => duckdb_loader::serve(),
        _ => {
            eprintln!(
                "usage: zdbt-el-worker extract --kind duckdb --db <path> [--schema <s>] \
                 --table <t> --chunk-rows <n> --out-dir <dir>\n       \
                 zdbt-el-worker seed-demo <path>"
            );
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        emit(&WorkerEvent::Error {
            message: format!("{error:#}"),
        });
        std::process::exit(1);
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|ix| args.get(ix + 1).cloned())
}

fn extract(args: &[String]) -> Result<()> {
    let kind = flag(args, "--kind").context("--kind required")?;
    let schema = flag(args, "--schema");
    let table = flag(args, "--table").context("--table required")?;
    let chunk_rows: usize = flag(args, "--chunk-rows")
        .context("--chunk-rows required")?
        .parse()
        .context("--chunk-rows must be a number")?;
    let out_dir = PathBuf::from(flag(args, "--out-dir").context("--out-dir required")?);

    let mut extractor: Box<dyn el_engine::connectors::Extractor> = match kind.as_str() {
        "duckdb" => {
            let db = PathBuf::from(flag(args, "--db").context("--db required")?);
            Box::new(DuckdbExtractor::new(&db, schema.as_deref(), &table, chunk_rows)?)
        }
        "postgres" => {
            let url = std::env::var("ZDBT_EL_SRC_URL")
                .context("ZDBT_EL_SRC_URL is not set in the worker environment")?;
            Box::new(el_engine::connectors::postgres::PostgresExtractor::new(
                &url,
                schema.as_deref(),
                &table,
                chunk_rows,
            )?)
        }
        other => bail!(
            "unsupported source kind {other:?} (this worker build supports: duckdb, postgres)"
        ),
    };
    let schema = extractor.schema()?;
    emit(&WorkerEvent::Schema {
        columns: schema
            .iter()
            .map(|(name, dtype)| {
                (
                    name.to_string(),
                    RemoteExtractor::dtype_to_wire(dtype).to_owned(),
                )
            })
            .collect(),
    });

    let mut index = 0usize;
    while let Some(mut chunk) = extractor.next_chunk()? {
        let path = out_dir.join(format!("chunk-{index:06}.ipc"));
        let file = std::fs::File::create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        IpcWriter::new(file)
            .finish(&mut chunk)
            .context("writing chunk ipc")?;
        emit(&WorkerEvent::Chunk {
            path,
            rows: chunk.height() as u64,
        });
        index += 1;
    }
    emit(&WorkerEvent::Done);
    Ok(())
}

fn seed_demo(args: &[String]) -> Result<()> {
    let path = PathBuf::from(args.first().context("seed-demo <path>")?);
    el_engine::connectors::duckdb::create_demo_db(&path)?;
    emit(&WorkerEvent::Done);
    Ok(())
}
