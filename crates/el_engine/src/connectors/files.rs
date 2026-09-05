//! File/object-storage extraction via polars scans: CSV, Parquet, NDJSON,
//! from local paths or cloud URLs (s3:// gs:// az://), with glob support.
//!
//! The scan is lazy; chunking is a slice window over the lazy frame so a
//! preview of 200 rows never materializes a 10 GB parquet.

use anyhow::{Context as _, Result, anyhow};
use polars::prelude::*;

use crate::spec::{CsvOptions, FileFormat};

pub struct FileExtractor {
    lazy: LazyFrame,
    chunk_rows: usize,
    offset: i64,
    exhausted: bool,
}

fn is_cloud(path: &str) -> bool {
    ["s3://", "gs://", "az://", "abfs://", "http://", "https://"]
        .iter()
        .any(|scheme| path.starts_with(scheme))
}

impl FileExtractor {
    pub fn new(
        project_root: &std::path::Path,
        path: &str,
        format: FileFormat,
        csv: Option<CsvOptions>,
        chunk_rows: usize,
    ) -> Result<Self> {
        let resolved = if is_cloud(path) || std::path::Path::new(path).is_absolute() {
            path.to_owned()
        } else {
            project_root
                .join(path)
                .to_str()
                .context("non-utf8 path")?
                .to_owned()
        };

        let lazy = match format {
            FileFormat::Csv => {
                let mut reader = LazyCsvReader::new(resolved.as_str().into())
                    .with_infer_schema_length(Some(500));
                if let Some(options) = &csv {
                    if let Some(delimiter) = options.delimiter {
                        reader = reader.with_separator(delimiter as u8);
                    }
                    if let Some(header) = options.header {
                        reader = reader.with_has_header(header);
                    }
                }
                reader
                    .finish()
                    .map_err(|error| anyhow!("opening csv {resolved}: {error}"))?
            }
            FileFormat::Parquet => {
                LazyFrame::scan_parquet(resolved.as_str().into(), ScanArgsParquet::default())
                    .map_err(|error| anyhow!("opening parquet {resolved}: {error}"))?
            }
            FileFormat::Ndjson => LazyJsonLineReader::new(resolved.as_str().into())
                .finish()
                .map_err(|error| anyhow!("opening ndjson {resolved}: {error}"))?,
        };

        Ok(Self {
            lazy,
            chunk_rows: chunk_rows.max(1),
            offset: 0,
            exhausted: false,
        })
    }
}

impl super::Extractor for FileExtractor {
    fn schema(&mut self) -> Result<Schema> {
        let schema = self
            .lazy
            .clone()
            .collect_schema()
            .map_err(|error| anyhow!("probing schema: {error}"))?;
        Ok(Schema::from_iter(
            schema
                .iter()
                .map(|(name, dtype)| (name.clone(), dtype.clone())),
        ))
    }

    fn next_chunk(&mut self) -> Result<Option<DataFrame>> {
        if self.exhausted {
            return Ok(None);
        }
        let chunk = self
            .lazy
            .clone()
            .slice(self.offset, self.chunk_rows as u32)
            .collect()
            .map_err(|error| anyhow!("reading chunk at offset {}: {error}", self.offset))?;
        if chunk.height() == 0 {
            self.exhausted = true;
            return Ok(None);
        }
        if chunk.height() < self.chunk_rows {
            self.exhausted = true;
        }
        self.offset += chunk.height() as i64;
        Ok(Some(chunk))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Extractor as _;
    use super::*;
    use std::io::Write as _;

    #[test]
    fn csv_chunks_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.csv");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "id,name").unwrap();
        for ix in 0..25 {
            writeln!(file, "{ix},row{ix}").unwrap();
        }

        let mut extractor =
            FileExtractor::new(dir.path(), "rows.csv", FileFormat::Csv, None, 10).unwrap();
        let schema = extractor.schema().unwrap();
        assert_eq!(schema.len(), 2);

        let mut total = 0;
        let mut chunks = 0;
        while let Some(chunk) = extractor.next_chunk().unwrap() {
            total += chunk.height();
            chunks += 1;
        }
        assert_eq!(total, 25);
        assert_eq!(chunks, 3, "10 + 10 + 5");
    }

    #[test]
    fn ndjson_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.ndjson");
        std::fs::write(&path, "{\"a\":1}\n{\"a\":2}\n").unwrap();
        let mut extractor =
            FileExtractor::new(dir.path(), "rows.ndjson", FileFormat::Ndjson, None, 100).unwrap();
        let chunk = extractor.next_chunk().unwrap().unwrap();
        assert_eq!(chunk.height(), 2);
    }
}
