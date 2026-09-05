//! Per-project incremental state: the cursor (watermark) each stream last
//! loaded through, plus a run journal. Plain rusqlite — never truncated,
//! never in `target/` (dbt clean wipes that), keyed by a hash of the
//! project root under the app data dir.
//!
//! The TARGET is the source of truth: after every commit the engine reads
//! `MAX(update_key)` back from the warehouse and stores it here. Losing
//! this file is harmless — the next run re-extracts rows the MERGE
//! absorbs idempotently.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// A typed cursor value. Stream identity (pipeline, stream name) owns it;
/// renaming a stream deliberately resets its cursor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum WatermarkValue {
    Int(i64),
    Float(f64),
    /// Microseconds since epoch.
    Timestamp(i64),
    /// Days since epoch.
    Date(i32),
    Text(String),
}

impl fmt::Display for WatermarkValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatermarkValue::Int(value) => write!(f, "{value}"),
            WatermarkValue::Float(value) => write!(f, "{value}"),
            WatermarkValue::Timestamp(micros) => {
                let secs = micros.div_euclid(1_000_000);
                let sub = micros.rem_euclid(1_000_000) as u32;
                match chrono_free_format(secs, sub) {
                    Some(text) => f.write_str(&text),
                    None => write!(f, "{micros}"),
                }
            }
            WatermarkValue::Date(days) => match days_to_ymd(*days) {
                Some((y, m, d)) => write!(f, "{y:04}-{m:02}-{d:02}"),
                None => write!(f, "{days}"),
            },
            WatermarkValue::Text(value) => f.write_str(value),
        }
    }
}

impl WatermarkValue {
    /// An ANSI-ish SQL literal understood by DuckDB, Postgres and
    /// Snowflake for cursor comparisons.
    pub fn to_sql_literal(&self) -> String {
        match self {
            WatermarkValue::Int(value) => value.to_string(),
            WatermarkValue::Float(value) => value.to_string(),
            WatermarkValue::Timestamp(_) => format!("TIMESTAMP '{self}'"),
            WatermarkValue::Date(_) => format!("DATE '{self}'"),
            WatermarkValue::Text(value) => format!("'{}'", value.replace('\'', "''")),
        }
    }

    /// Parses a scalar the loader read back from the target, guided by the
    /// cursor column's target type.
    pub fn parse_scalar(text: &str, sf_type: &crate::types::SnowflakeType) -> Option<Self> {
        use crate::types::SfBase;
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        Some(match sf_type.base {
            SfBase::Number if sf_type.scale.unwrap_or(0) == 0 => {
                WatermarkValue::Int(text.parse().ok()?)
            }
            SfBase::Number | SfBase::Float => WatermarkValue::Float(text.parse().ok()?),
            SfBase::Date => {
                let (y, m, d) = split_ymd(text)?;
                WatermarkValue::Date(ymd_to_days(y, m, d)?)
            }
            SfBase::TimestampNtz | SfBase::TimestampTz => {
                WatermarkValue::Timestamp(parse_timestamp_micros(text)?)
            }
            _ => WatermarkValue::Text(text.to_owned()),
        })
    }
}

// -- tiny date math (no chrono in the default build) ------------------------

fn days_to_ymd(days: i32) -> Option<(i32, u32, u32)> {
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    Some(((y + i64::from(m <= 2)) as i32, m, d))
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> Option<i32> {
    let y = y as i64 - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) as i32)
}

fn split_ymd(text: &str) -> Option<(i32, u32, u32)> {
    let mut parts = text.splitn(3, '-');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.split(&[' ', 'T'][..]).next()?.parse().ok()?,
    ))
}

fn parse_timestamp_micros(text: &str) -> Option<i64> {
    // "YYYY-MM-DD[ T]HH:MM:SS[.ffffff][+00[:00]|Z]"
    let (date_part, time_part) = text.split_once(&[' ', 'T'][..])?;
    let (y, m, d) = split_ymd(date_part)?;
    let days = ymd_to_days(y, m, d)? as i64;
    let time_part = time_part
        .trim_end_matches('Z')
        .split(&['+'][..])
        .next()?
        .trim();
    let mut hms = time_part.splitn(3, ':');
    let hours: i64 = hms.next()?.parse().ok()?;
    let minutes: i64 = hms.next()?.parse().ok()?;
    let rest = hms.next().unwrap_or("0");
    let (secs_text, frac_text) = rest.split_once('.').unwrap_or((rest, ""));
    let seconds: i64 = secs_text.parse().ok()?;
    let mut micros_frac: i64 = 0;
    if !frac_text.is_empty() {
        let mut padded = frac_text.to_owned();
        padded.truncate(6);
        while padded.len() < 6 {
            padded.push('0');
        }
        micros_frac = padded.parse().ok()?;
    }
    Some(((days * 24 + hours) * 60 + minutes) * 60_000_000 + seconds * 1_000_000 + micros_frac)
}

fn chrono_free_format(secs: i64, sub_micros: u32) -> Option<String> {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = days_to_ymd(days as i32)?;
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    Some(if sub_micros == 0 {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{sub_micros:06}")
    })
}

// -- the store --------------------------------------------------------------

pub struct StateStore {
    connection: rusqlite::Connection,
}

/// Where a project's state db lives. Overridable for tests via
/// ZDBT_EL_STATE_DIR.
pub fn state_db_path(project_root: &Path, profile: Option<&str>) -> PathBuf {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(project_root.to_string_lossy().as_bytes());
    // Watermarks are PER ENVIRONMENT: a dev cursor must never make a
    // recette/prod run skip rows (or vice versa). Each profile gets its
    // own state database.
    if let Some(profile) = profile {
        hasher.update([0u8]);
        hasher.update(profile.as_bytes());
    }
    let digest = hasher.finalize();
    let short = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let base = std::env::var_os("ZDBT_EL_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths_data_dir().join("el-state"));
    base.join(format!("{short}.sqlite"))
}

fn paths_data_dir() -> PathBuf {
    // Mirrors zed's data dir without depending on the paths crate:
    // ~/Library/Application Support/Zed on macOS, XDG on Linux.
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/Application Support/Zed")
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join(".local/share")
            })
            .join("zed")
    }
}

impl StateStore {
    pub fn open(project_root: &Path, profile: Option<&str>) -> Result<Self> {
        let path = state_db_path(project_root, profile);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating el-state dir")?;
        }
        let connection = rusqlite::Connection::open(&path)
            .with_context(|| format!("opening state db {}", path.display()))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS stream_state (
                     pipeline TEXT NOT NULL,
                     stream TEXT NOT NULL,
                     watermark_json TEXT,
                     updated_at TEXT,
                     PRIMARY KEY (pipeline, stream)
                 );
                 CREATE TABLE IF NOT EXISTS load_runs (
                     id INTEGER PRIMARY KEY,
                     pipeline TEXT NOT NULL,
                     stream TEXT NOT NULL,
                     status TEXT NOT NULL,
                     rows_read INTEGER,
                     rows_written INTEGER,
                     finished_at TEXT
                 );",
            )
            .context("initializing state schema")?;
        Ok(Self { connection })
    }

    pub fn watermark(&self, pipeline: &str, stream: &str) -> Option<WatermarkValue> {
        self.connection
            .query_row(
                "SELECT watermark_json FROM stream_state WHERE pipeline = ?1 AND stream = ?2",
                (pipeline, stream),
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
    }

    pub fn set_watermark(
        &self,
        pipeline: &str,
        stream: &str,
        watermark: &WatermarkValue,
    ) -> Result<()> {
        let json = serde_json::to_string(watermark)?;
        self.connection
            .execute(
                "INSERT INTO stream_state (pipeline, stream, watermark_json, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'))
                 ON CONFLICT (pipeline, stream)
                 DO UPDATE SET watermark_json = ?3, updated_at = datetime('now')",
                (pipeline, stream, json),
            )
            .context("writing watermark")?;
        Ok(())
    }

    pub fn record_run(
        &self,
        pipeline: &str,
        stream: &str,
        status: &str,
        rows_read: u64,
        rows_written: u64,
    ) -> Result<()> {
        self.connection
            .execute(
                "INSERT INTO load_runs (pipeline, stream, status, rows_read, rows_written, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                (
                    pipeline,
                    stream,
                    status,
                    rows_read as i64,
                    rows_written as i64,
                ),
            )
            .context("recording run")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_round_trip_and_literals() {
        let ts = WatermarkValue::parse_scalar(
            "2026-01-05 08:41:33",
            &"TIMESTAMP_NTZ".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(ts.to_string(), "2026-01-05 08:41:33");
        assert_eq!(ts.to_sql_literal(), "TIMESTAMP '2026-01-05 08:41:33'");

        let date =
            WatermarkValue::parse_scalar("2026-01-07", &"DATE".parse().unwrap()).unwrap();
        assert_eq!(date, WatermarkValue::Date(ymd_to_days(2026, 1, 7).unwrap()));
        assert_eq!(date.to_sql_literal(), "DATE '2026-01-07'");

        let int =
            WatermarkValue::parse_scalar("42", &"NUMBER(38,0)".parse().unwrap()).unwrap();
        assert_eq!(int, WatermarkValue::Int(42));

        let quoted = WatermarkValue::Text("o'clock".into());
        assert_eq!(quoted.to_sql_literal(), "'o''clock'");
    }

    #[test]
    fn timestamp_parsing_variants() {
        for (text, expect_display) in [
            ("2026-01-05T08:41:33Z", "2026-01-05 08:41:33"),
            ("2026-01-05 08:41:33.250000", "2026-01-05 08:41:33.250000"),
            ("2026-01-05 08:41:33+00", "2026-01-05 08:41:33"),
        ] {
            let value = WatermarkValue::parse_scalar(text, &"TIMESTAMP_NTZ".parse().unwrap())
                .unwrap_or_else(|| panic!("parse {text}"));
            assert_eq!(value.to_string(), expect_display, "{text}");
        }
    }

    #[test]
    fn store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-local env var, no threads racing it here.
        unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", dir.path()) };
        let store = StateStore::open(Path::new("/some/project"), None).unwrap();
        assert!(store.watermark("p", "s").is_none());
        let wm = WatermarkValue::Timestamp(1_234_567_890);
        store.set_watermark("p", "s", &wm).unwrap();
        assert_eq!(store.watermark("p", "s"), Some(wm.clone()));
        // Profiles get distinct databases.
        assert_ne!(
            state_db_path(Path::new("/p"), None),
            state_db_path(Path::new("/p"), Some("prod"))
        );
        // Update wins.
        let wm2 = WatermarkValue::Timestamp(9_999_999_999);
        store.set_watermark("p", "s", &wm2).unwrap();
        assert_eq!(store.watermark("p", "s"), Some(wm2));
        store.record_run("p", "s", "ok", 10, 10).unwrap();
        unsafe { std::env::remove_var("ZDBT_EL_STATE_DIR") };
    }
}
