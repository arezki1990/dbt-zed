use sqlitegraph::bench_meta::BenchRun;
use sqlitegraph::bench_regression::{BenchGate, BenchGateConfig, GateEnforcer};

fn config() -> BenchGateConfig {
    BenchGateConfig {
        thresholds: vec![("insert_small".into(), 2_000_000)],
        baseline: vec![BenchRun {
            name: "insert_small".into(),
            mean_ns: 1_500_000,
            samples: 20,
        }],
        tolerance: 0.10,
    }
}

#[test]
fn test_gate_report_collects_failures() {
    let enforcer = GateEnforcer::new(BenchGate::new(config()));
    let runs = vec![BenchRun {
        name: "insert_small".into(),
        mean_ns: 2_500_000,
        samples: 20,
    }];
    let report = enforcer.evaluate(&runs);
    assert!(!report.passed);
    assert_eq!(report.reasons.len(), 2);
    assert!(report.reasons.iter().any(|r| r.contains("threshold")));
    assert!(report.reasons.iter().any(|r| r.contains("baseline")));
}

#[test]
fn test_gate_report_passes_when_conditions_met() {
    let enforcer = GateEnforcer::new(BenchGate::new(config()));
    let runs = vec![BenchRun {
        name: "insert_small".into(),
        mean_ns: 1_600_000,
        samples: 20,
    }];
    let report = enforcer.evaluate(&runs);
    assert!(report.passed);
    assert!(report.reasons.is_empty());
}
