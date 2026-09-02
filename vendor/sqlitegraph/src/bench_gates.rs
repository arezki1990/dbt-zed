use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::SqliteGraphError;

static BENCH_FILE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_bench_file_path(path: PathBuf) {
    let mut guard = BENCH_FILE_OVERRIDE.lock().expect("bench file lock");
    *guard = Some(path);
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BenchMetric {
    pub name: String,
    pub ops_per_sec: f64,
    pub bytes_per_sec: f64,
    pub notes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BenchThreshold {
    pub name: String,
    pub min_ops_per_sec: f64,
    pub max_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum BenchGateResult {
    Pass,
    Fail { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BenchComparison {
    pub name: String,
    pub baseline_ops_per_sec: f64,
    pub current_ops_per_sec: f64,
    pub delta_ops_per_sec: f64,
    pub improved: bool,
}

pub fn record_bench_run(name: &str, metric: BenchMetric) -> Result<(), SqliteGraphError> {
    if metric.name != name {
        return Err(SqliteGraphError::invalid_input(
            "metric name must match record name",
        ));
    }
    let path = bench_metrics_file();
    let mut runs = load_runs_from(&path)?;
    runs.retain(|m| m.name != name);
    runs.push(metric);
    runs.sort_by(|a, b| a.name.cmp(&b.name));
    let data = serde_json::to_vec_pretty(&runs)
        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
    fs::write(path, data).map_err(|e| SqliteGraphError::query(e.to_string()))
}

pub fn check_thresholds(
    name: &str,
    threshold: BenchThreshold,
) -> Result<BenchGateResult, SqliteGraphError> {
    let path = bench_metrics_file();
    let runs = load_runs_from(&path)?;
    let metric = runs
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| SqliteGraphError::not_found(format!("bench metric {name}")))?;
    if metric.ops_per_sec < threshold.min_ops_per_sec {
        return Ok(BenchGateResult::Fail {
            reason: format!(
                "ops_per_sec {} below minimum {}",
                metric.ops_per_sec, threshold.min_ops_per_sec
            ),
        });
    }
    let ms_per_op = 1000.0 / metric.ops_per_sec;
    if ms_per_op > threshold.max_ms {
        return Ok(BenchGateResult::Fail {
            reason: format!("ms_per_op {ms_per_op:.4} exceeds {}", threshold.max_ms),
        });
    }
    Ok(BenchGateResult::Pass)
}

pub fn load_previous_runs() -> Result<Vec<BenchMetric>, SqliteGraphError> {
    let path = bench_metrics_file();
    load_runs_from(&path)
}

pub fn compare_to_baseline(
    name: &str,
    current: BenchMetric,
) -> Result<BenchComparison, SqliteGraphError> {
    if current.name != name {
        return Err(SqliteGraphError::invalid_input(
            "metric name must match comparison name",
        ));
    }
    let path = bench_metrics_file();
    let baseline = load_runs_from(&path)?
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| SqliteGraphError::not_found(format!("baseline {name}")))?;
    let delta = current.ops_per_sec - baseline.ops_per_sec;
    Ok(BenchComparison {
        name: name.to_string(),
        baseline_ops_per_sec: baseline.ops_per_sec,
        current_ops_per_sec: current.ops_per_sec,
        delta_ops_per_sec: delta,
        improved: delta >= 0.0,
    })
}

fn bench_metrics_file() -> PathBuf {
    if let Some(path) = BENCH_FILE_OVERRIDE.lock().expect("bench file lock").clone() {
        return path;
    }
    if let Ok(path) = env::var("SQLITEGRAPH_BENCH_FILE") {
        return PathBuf::from(path);
    }
    Path::new("sqlitegraph_bench.json").to_path_buf()
}

fn load_runs_from(path: &Path) -> Result<Vec<BenchMetric>, SqliteGraphError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read(path).map_err(|e| SqliteGraphError::query(e.to_string()))?;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&data).map_err(|e| SqliteGraphError::invalid_input(e.to_string()))
}
