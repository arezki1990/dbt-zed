use std::collections::HashMap;

use crate::bench_meta::BenchRun;

#[derive(Clone, Debug, PartialEq)]
pub struct BenchGateConfig {
    pub thresholds: Vec<(String, u64)>,
    pub baseline: Vec<BenchRun>,
    pub tolerance: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BenchOutcome {
    Pass,
    Fail(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct BenchGate {
    thresholds: HashMap<String, u64>,
    baseline: HashMap<String, BenchRun>,
    tolerance: f64,
}

impl BenchGate {
    pub fn new(config: BenchGateConfig) -> Self {
        let thresholds = config.thresholds.into_iter().collect();
        let baseline = config
            .baseline
            .into_iter()
            .map(|run| (run.name.clone(), run))
            .collect();
        Self {
            thresholds,
            baseline,
            tolerance: config.tolerance,
        }
    }

    pub fn evaluate(&self, runs: &[BenchRun]) -> BenchOutcome {
        let mut failures = Vec::new();
        for run in runs {
            if let Some(max_ns) = self.thresholds.get(&run.name)
                && !run.within_threshold(*max_ns)
            {
                failures.push(format!("{} exceeds threshold {}ns", run.name, max_ns));
            }
            if let Some(baseline) = self.baseline.get(&run.name)
                && !run.within_regression(baseline, self.tolerance)
            {
                failures.push(format!(
                    "{} regression: current={}ns baseline={}ns tolerance={}",
                    run.name, run.mean_ns, baseline.mean_ns, self.tolerance
                ));
            }
        }
        if failures.is_empty() {
            BenchOutcome::Pass
        } else {
            BenchOutcome::Fail(failures)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    pub passed: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct GateEnforcer {
    gate: BenchGate,
}

impl GateEnforcer {
    pub fn new(gate: BenchGate) -> Self {
        Self { gate }
    }

    pub fn evaluate(&self, runs: &[BenchRun]) -> GateReport {
        match self.gate.evaluate(runs) {
            BenchOutcome::Pass => GateReport {
                passed: true,
                reasons: Vec::new(),
            },
            BenchOutcome::Fail(reasons) => GateReport {
                passed: false,
                reasons,
            },
        }
    }
}
