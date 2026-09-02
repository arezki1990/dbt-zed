//! Test V2 neighbors cache performance and correctness
use sqlitegraph::{
    BackendDirection, EdgeSpec, GraphConfig, NeighborQuery, NodeSpec, SnapshotId, open_graph,
};
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== V2 NEIGHBORS CACHE VERIFICATION ===\n");

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let graph = open_graph(&db_path, &GraphConfig::native())?;

    // Insert nodes
    let node_0 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node_0".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 0}),
    })?;

    for i in 1..=20 {
        let id = graph.insert_node(NodeSpec {
            kind: "Test".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"id": i}),
        })?;

        graph.insert_edge(EdgeSpec {
            from: node_0,
            to: id,
            edge_type: "test".to_string(),
            data: serde_json::Value::Null,
        })?;
    }

    let snapshot = SnapshotId::current();
    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };

    // Test 1: Cold query
    let result = graph.neighbors(snapshot, node_0, query.clone())?;
    println!("Cold query: {} neighbors", result.len());

    // Test 2: Warm queries (should be cached)
    const ITERATIONS: usize = 10000;
    let start = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        let result = graph.neighbors(snapshot, node_0, query.clone())?;
        assert_eq!(result.len(), 20, "Should have 20 neighbors");
    }
    let elapsed = start.elapsed();
    let ns_per_query = elapsed.as_nanos() / ITERATIONS as u128;

    println!("Warm queries: {} ns/query", ns_per_query);

    if ns_per_query < 1000 {
        println!("✅ EXCELLENT: Cache is working! (< 1 µs)");
    } else if ns_per_query < 10000 {
        println!("✅ GOOD: Cache is working (< 10 µs)");
    } else {
        println!("❌ FAIL: Cache not working ({} ns/query)", ns_per_query);
    }

    // Test 3: Edge insertion should invalidate cache
    let node_21 = graph.insert_node(NodeSpec {
        kind: "Test".to_string(),
        name: "node_21".to_string(),
        file_path: None,
        data: serde_json::json!({"id": 21}),
    })?;

    graph.insert_edge(EdgeSpec {
        from: node_0,
        to: node_21,
        edge_type: "test".to_string(),
        data: serde_json::Value::Null,
    })?;

    let result = graph.neighbors(snapshot, node_0, query.clone())?;
    println!("After insert: {} neighbors", result.len());
    assert_eq!(result.len(), 21, "Should have 21 neighbors after insert");

    println!("✅ Cache invalidation works!");

    Ok(())
}
