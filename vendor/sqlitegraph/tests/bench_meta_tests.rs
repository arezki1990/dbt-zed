use sqlitegraph::bench_meta::BenchRun;

#[test]
fn test_bench_run_summarizes_metrics() {
    let run = BenchRun {
        name: "bench_insert".into(),
        mean_ns: 1_234_000,
        samples: 20,
    };
    assert_eq!(run.summary(), "bench_insert: mean=1234000ns samples=20");
}

#[test]
fn test_bench_run_within_threshold() {
    let run = BenchRun {
        name: "bench_traversal".into(),
        mean_ns: 2_000_000,
        samples: 30,
    };
    assert!(run.within_threshold(2_500_000));
    assert!(!run.within_threshold(1_500_000));
}

#[test]
fn test_bench_run_regression_check() {
    let baseline = BenchRun {
        name: "bench_insert".into(),
        mean_ns: 1_000_000,
        samples: 20,
    };
    let current = BenchRun {
        name: "bench_insert".into(),
        mean_ns: 1_050_000,
        samples: 25,
    };
    assert!(current.within_regression(&baseline, 0.10));
    assert!(!current.within_regression(&baseline, 0.04));
}
