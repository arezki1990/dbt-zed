use sqlitegraph::bench_meta::BenchRun;
use sqlitegraph::bench_regression::{BenchGate, BenchGateConfig, BenchOutcome};

fn sample_runs() -> Vec<BenchRun> {
    vec![
        BenchRun {
            name: "insert_small".into(),
            mean_ns: 1_000_000,
            samples: 20,
        },
        BenchRun {
            name: "insert_large".into(),
            mean_ns: 5_000_000,
            samples: 20,
        },
    ]
}

#[test]
fn test_gate_passes_with_thresholds() {
    let gate = BenchGate::new(BenchGateConfig {
        thresholds: vec![
            ("insert_small".into(), 2_000_000),
            ("insert_large".into(), 6_000_000),
        ],
        baseline: vec![],
        tolerance: 0.05,
    });
    let outcome = gate.evaluate(&sample_runs());
    assert_eq!(outcome, BenchOutcome::Pass);
}

#[test]
fn test_gate_detects_threshold_failure() {
    let gate = BenchGate::new(BenchGateConfig {
        thresholds: vec![("insert_small".into(), 500_000)],
        baseline: vec![],
        tolerance: 0.05,
    });
    let outcome = gate.evaluate(&sample_runs());
    assert!(matches!(outcome, BenchOutcome::Fail(ref reasons) if reasons.len() == 1));
}

#[test]
fn test_gate_detects_baseline_regression() {
    let gate = BenchGate::new(BenchGateConfig {
        thresholds: vec![],
        baseline: vec![BenchRun {
            name: "insert_small".into(),
            mean_ns: 900_000,
            samples: 20,
        }],
        tolerance: 0.05,
    });
    let outcome = gate.evaluate(&sample_runs());
    assert!(matches!(outcome, BenchOutcome::Fail(ref reasons) if reasons[0].contains("baseline")));
}
