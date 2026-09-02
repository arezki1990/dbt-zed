//! Regression test for native backend edge insertion JSON parsing panic
//!
//! This test reproduces the ConnectionError("expected value at line 1 column 1")
//! that occurs during edge insertion in the native backend when V2 atomic commit
//! protocol is enabled (which is the default for new files).

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, open_graph};
use tempfile::tempdir;

#[test]
fn test_native_edge_insertion_regression() -> Result<(), Box<dyn std::error::Error>> {
    // This test reproduces the exact panic that occurs in the benchmark
    // when inserting edges into a native backend graph.

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("regression_test.db");

    // Create native backend graph (V2 atomic commit is enabled by default)
    let graph = open_graph(&db_path, &GraphConfig::native())?;

    // Create two nodes
    let node1 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node1".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 1}),
    })?;

    let node2 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node2".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 2}),
    })?;

    // This edge insertion should NOT fail with:
    // ConnectionError("expected value at line 1 column 1")
    let edge_id = graph.insert_edge(EdgeSpec {
        from: node1,
        to: node2,
        edge_type: "test".to_string(),
        data: serde_json::json!({"order": 1}),
    })?;

    // If we get here, the bug is fixed
    assert!(edge_id > 0);

    Ok(())
}
