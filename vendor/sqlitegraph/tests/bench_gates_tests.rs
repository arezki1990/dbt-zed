use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use sqlitegraph::bench_gates::{
    BenchGateResult, BenchMetric, BenchThreshold, check_thresholds, compare_to_baseline,
    load_previous_runs, record_bench_run, set_bench_file_path,
};

fn set_bench_file(test_name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("sqlitegraph_{test_name}.json"));
    let _ = fs::remove_file(&path);
    set_bench_file_path(path.clone());
    path
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("bench lock")
}

fn metric(name: &str, ops: f64) -> BenchMetric {
    BenchMetric {
        name: name.into(),
        ops_per_sec: ops,
        bytes_per_sec: 0.0,
        notes: String::new(),
    }
}

#[test]
fn test_record_and_reload_roundtrip() {
    let _guard = test_lock();
    let path = set_bench_file("record_roundtrip");
    record_bench_run("bench_round", metric("bench_round", 100.0)).unwrap();
    let runs = load_previous_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].name, "bench_round");
    fs::remove_file(path).ok();
}

#[test]
fn test_threshold_passes() {
    let _guard = test_lock();
    let path = set_bench_file("threshold_pass");
    record_bench_run("bench_thr", metric("bench_thr", 250.0)).unwrap();
    let threshold = BenchThreshold {
        name: "bench_thr".into(),
        min_ops_per_sec: 200.0,
        max_ms: 5.0,
    };
    let result = check_thresholds("bench_thr", threshold).unwrap();
    assert!(matches!(result, BenchGateResult::Pass));
    fs::remove_file(path).ok();
}

#[test]
fn test_threshold_fails() {
    let _guard = test_lock();
    let path = set_bench_file("threshold_fail");
    record_bench_run("bench_thr", metric("bench_thr", 50.0)).unwrap();
    let threshold = BenchThreshold {
        name: "bench_thr".into(),
        min_ops_per_sec: 100.0,
        max_ms: 5.0,
    };
    let result = check_thresholds("bench_thr", threshold).unwrap();
    assert!(matches!(result, BenchGateResult::Fail { .. }));
    fs::remove_file(path).ok();
}

#[test]
fn test_baseline_comparison_improved() {
    let _guard = test_lock();
    let path = set_bench_file("baseline_improved");
    record_bench_run("bench_cmp", metric("bench_cmp", 100.0)).unwrap();
    let comparison = compare_to_baseline("bench_cmp", metric("bench_cmp", 125.0)).unwrap();
    assert!(comparison.improved);
    assert!(comparison.delta_ops_per_sec > 0.0);
    fs::remove_file(path).ok();
}

#[test]
fn test_baseline_comparison_regressed() {
    let _guard = test_lock();
    let path = set_bench_file("baseline_regressed");
    record_bench_run("bench_cmp", metric("bench_cmp", 120.0)).unwrap();
    let comparison = compare_to_baseline("bench_cmp", metric("bench_cmp", 80.0)).unwrap();
    assert!(!comparison.improved);
    assert!(comparison.delta_ops_per_sec < 0.0);
    fs::remove_file(path).ok();
}
