use std::path::PathBuf;

use sqlitegraph::bench_gates::{self, BenchGateResult, BenchThreshold, set_bench_file_path};

#[test]
fn subgraph_gate_uses_recorded_baseline() {
    set_bench_file_path(baseline_path());
    let result = bench_gates::check_thresholds(
        "subgraph_extract",
        BenchThreshold {
            name: "subgraph_extract".into(),
            min_ops_per_sec: 750.0,
            max_ms: 5.0,
        },
    )
    .expect("gate");
    assert_eq!(result, BenchGateResult::Pass);
}

#[test]
fn pipeline_gate_uses_recorded_baseline() {
    set_bench_file_path(baseline_path());
    let result = bench_gates::check_thresholds(
        "pipeline_reason",
        BenchThreshold {
            name: "pipeline_reason".into(),
            min_ops_per_sec: 600.0,
            max_ms: 7.0,
        },
    )
    .expect("gate");
    assert_eq!(result, BenchGateResult::Pass);
}

#[test]
fn migration_shadow_gate_uses_recorded_baseline() {
    set_bench_file_path(baseline_path());
    let result = bench_gates::check_thresholds(
        "migration_shadow_read",
        BenchThreshold {
            name: "migration_shadow_read".into(),
            min_ops_per_sec: 400.0,
            max_ms: 9.0,
        },
    )
    .expect("gate");
    assert_eq!(result, BenchGateResult::Pass);
}

#[test]
fn syncompat_subgraph_gate_uses_recorded_baseline() {
    set_bench_file_path(baseline_path());
    let result = bench_gates::check_thresholds(
        "syncompat_subgraph",
        BenchThreshold {
            name: "syncompat_subgraph".into(),
            min_ops_per_sec: 900.0,
            max_ms: 4.0,
        },
    )
    .expect("gate");
    assert_eq!(result, BenchGateResult::Pass);
}

#[test]
fn syncompat_pipeline_gate_uses_recorded_baseline() {
    set_bench_file_path(baseline_path());
    let result = bench_gates::check_thresholds(
        "syncompat_pipeline",
        BenchThreshold {
            name: "syncompat_pipeline".into(),
            min_ops_per_sec: 650.0,
            max_ms: 6.0,
        },
    )
    .expect("gate");
    assert_eq!(result, BenchGateResult::Pass);
}

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sqlitegraph_bench.json")
}
