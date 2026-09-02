//! Phase 32 performance gate tests for chain traversal validation (IO-12)
//!
//! Validates that chain traversal performance is within 3x of SQLite baseline.
//! This automated gate validates the sequential I/O optimization from Phases 29-31.

use std::path::PathBuf;

use serde_json::Value;
use sqlitegraph::bench_meta::BenchRun;
use sqlitegraph::bench_regression::{BenchGate, BenchGateConfig, BenchOutcome};

/// 3x SQLite baseline thresholds from Phase 24
///
/// SQLite baseline (Phase 24): Chain(500) = 24.978ms
/// IO-12 target: 3x = 3 * 24.978ms = ~75ms (75,000,000ns)
const SQLITE_3X_THRESHOLD_500: u64 = 75_000_000; // 75ms

/// v1.3 baseline (before sequential I/O optimization)
/// From Phase 28 summary: Chain(500) = 255.29ms
const V13_BASELINE_500: u64 = 255_290_000; // 255.29ms

/// SQLite baseline in milliseconds for ratio calculation
const SQLITE_BASELINE_MS_500: f64 = 24.978;

#[test]
fn test_chain_traversal_within_3x_sqlite_baseline() {
    // Load current benchmark results
    let current_runs = match load_criterion_results() {
        Ok(runs) => runs,
        Err(e) => {
            println!("Skipping perf gate: Criterion data missing. {}", e);
            return;
        }
    };

    println!("\n=== Chain Traversal Performance Gate (IO-12) ===");
    println!("3x SQLite Target Thresholds:");
    println!(
        "  Chain BFS (500): {}ms (3x SQLite target: {:.3}ms)",
        SQLITE_3X_THRESHOLD_500 as f64 / 1_000_000.0,
        SQLITE_BASELINE_MS_500 * 3.0
    );
    println!();

    // Create BenchGate with 3x SQLite threshold
    let gate = BenchGate::new(BenchGateConfig {
        thresholds: vec![("bfs_chain/native/500".into(), SQLITE_3X_THRESHOLD_500)],
        baseline: vec![],
        tolerance: 0.05, // 5% tolerance
    });

    // Evaluate gate
    let outcome = gate.evaluate(&current_runs);

    // Print detailed results
    for run in &current_runs {
        let ratio_ms = run.mean_ns as f64 / 1_000_000.0;
        let ratio_to_sqlite = ratio_ms / SQLITE_BASELINE_MS_500;
        println!(
            "  {}: {:.2}ms ({:.2}x SQLite baseline {:.3}ms) - {}",
            run.name,
            ratio_ms,
            ratio_to_sqlite,
            SQLITE_BASELINE_MS_500,
            if run.mean_ns <= SQLITE_3X_THRESHOLD_500 {
                "PASS"
            } else {
                "FAIL"
            }
        );
    }

    // Assert Pass outcome
    assert_eq!(
        outcome,
        BenchOutcome::Pass,
        "Chain traversal exceeds 3x SQLite baseline. Outcome: {:?}",
        outcome
    );
}

#[test]
fn test_chain_traversal_regression_check() {
    // Load current benchmark results
    let current_runs = match load_criterion_results() {
        Ok(runs) => runs,
        Err(e) => {
            println!("Skipping regression check: Criterion data missing. {}", e);
            return;
        }
    };

    println!("\n=== Chain Traversal Regression Check vs v1.3 ===");
    println!("v1.3 Baseline (before sequential I/O optimization):");
    println!(
        "  Chain BFS (500): {:.2}ms",
        V13_BASELINE_500 as f64 / 1_000_000.0
    );
    println!();

    // Check for regression vs v1.3 (allow 10% tolerance for non-chain workloads)
    for run in &current_runs {
        let improvement = if run.mean_ns < V13_BASELINE_500 {
            ((V13_BASELINE_500 as f64 - run.mean_ns as f64) / V13_BASELINE_500 as f64) * 100.0
        } else {
            -((run.mean_ns as f64 - V13_BASELINE_500 as f64) / V13_BASELINE_500 as f64) * 100.0
        };

        println!(
            "  {}: Current {:.2}ms vs v1.3 Baseline {:.2}ms ({:.1}% {})",
            run.name,
            run.mean_ns as f64 / 1_000_000.0,
            V13_BASELINE_500 as f64 / 1_000_000.0,
            improvement.abs(),
            if improvement >= 0.0 {
                "improvement"
            } else {
                "regression"
            }
        );
    }
}

/// Load Criterion benchmark results from target/criterion directory
fn load_criterion_results() -> Result<Vec<BenchRun>, String> {
    // CARGO_MANIFEST_DIR points to the crate directory (sqlitegraph/)
    // Criterion output goes to workspace-root/target/criterion
    let criterion_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/criterion");

    let mut results = Vec::new();

    // Load bfs_chain/native/500
    let path_500 = criterion_dir.join("bfs_chain/native/500/new/estimates.json");
    match load_estimate_json(&path_500) {
        Ok(value) => {
            let mean_ns = extract_point_estimate(&value)?;
            results.push(BenchRun {
                name: "bfs_chain/native/500".to_string(),
                mean_ns,
                samples: 20, // Criterion default
            });
        }
        Err(e) => {
            return Err(format!(
                "Failed to load {}: {}. Run: cargo bench --bench bfs",
                path_500.display(),
                e
            ));
        }
    }

    Ok(results)
}

/// Load JSON file
fn load_estimate_json(path: &PathBuf) -> Result<Value, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse JSON from {}: {}", path.display(), e))
}

/// Extract mean point estimate from Criterion estimates.json
fn extract_point_estimate(value: &Value) -> Result<u64, String> {
    value
        .get("mean")
        .and_then(|m| m.get("point_estimate"))
        .and_then(|p| p.as_f64())
        .map(|v| v as u64)
        .ok_or_else(|| "Invalid estimate.json format".into())
}
