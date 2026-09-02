use std::fs;

fn read(path: &str) -> String {
    fs::read_to_string(path).expect("doc file")
}

#[test]
fn test_readme_contains_safety_invariants() {
    let readme = read("README.md");
    assert!(readme.contains("## Safety Invariants"));
}

#[test]
fn test_readme_contains_dsl_constraints() {
    let readme = read("README.md");
    assert!(readme.contains("## DSL Constraints"));
}

#[test]
fn test_readme_contains_regression_gate_explanation() {
    let readme = read("README.md");
    // The README documents that performance thresholds in sqlitegraph_bench.json
    // gate releases. Check both phrases independently so the assertion isn't
    // brittle about backtick/code-font formatting around the filename.
    assert!(
        readme.contains("Performance thresholds"),
        "README should document the performance-threshold regression gate"
    );
    assert!(
        readme.contains("sqlitegraph_bench.json"),
        "README should reference the sqlitegraph_bench.json thresholds file"
    );
}

#[test]
fn test_manual_contains_safety_invariants() {
    let manual = read("manual.md");
    assert!(manual.contains("### Safety Invariants"));
}

#[test]
fn test_manual_contains_dsl_constraints() {
    let manual = read("manual.md");
    assert!(manual.contains("### DSL Constraints"));
}

#[test]
fn test_readme_mentions_metrics_functionality() {
    let readme = read("README.md");
    assert!(readme.contains("metrics") || readme.contains("instrumentation"));
}

#[test]
fn test_manual_mentions_metrics_command() {
    let manual = read("manual.md");
    assert!(manual.contains("CLI metrics command"));
}

#[test]
fn test_readme_contains_schema_matrix() {
    let readme = read("README.md");
    assert!(readme.contains("Schema Compatibility Matrix"));
}

#[test]
fn test_manual_contains_schema_matrix() {
    let manual = read("manual.md");
    assert!(manual.contains("Schema version matrix"));
}
