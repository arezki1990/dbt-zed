//! Algorithm Benchmark: SQLite vs V3 Backend
//!
//! Tests backend-agnostic algorithms on both backends.

use sqlitegraph::algo::backend::{
    bfs, bfs_traversal, dfs_traversal, k_hop_neighbors, shortest_path,
};
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NodeSpec, SqliteGraphBackend};
use sqlitegraph::snapshot::SnapshotId;
use std::time::Instant;
use tempfile::TempDir;

/// Benchmark result
#[derive(Debug, Clone)]
struct BenchmarkResult {
    algorithm: String,
    backend: String,
    graph_nodes: usize,
    graph_edges: usize,
    elapsed_ms: f64,
    success: bool,
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║     Algorithm Benchmark: SQLite vs Native V3 Backend             ║");
    println!("║              35+ Tests Across Multiple Graph Sizes               ║");
    println!("╚══════════════════════════════════════════════════════════════════╝\n");

    let mut all_results: Vec<BenchmarkResult> = Vec::new();

    // Test configurations
    let test_graphs = vec![
        ("Small", 1000, 5000),
        ("Medium", 10000, 50000),
        ("Large", 50000, 250000),
    ];

    for (graph_name, node_count, edge_count) in test_graphs {
        println!("\n{}", "=".repeat(70));
        println!(
            "Graph: {} ({} nodes, {} edges)",
            graph_name, node_count, edge_count
        );
        println!("{}", "=".repeat(70));

        // 1. Traversal Algorithms (3 tests)
        println!("\n--- 1. Traversal Algorithms ---");
        all_results.extend(benchmark_bfs_traversal(node_count, edge_count));
        all_results.extend(benchmark_dfs_traversal(node_count, edge_count));
        all_results.extend(benchmark_k_hop(node_count, edge_count));

        // 2. Path Algorithms (3 tests)
        println!("\n--- 2. Path Algorithms ---");
        all_results.extend(benchmark_shortest_path(node_count, edge_count));
        all_results.extend(benchmark_bfs_levels(node_count, edge_count));
        all_results.extend(benchmark_k_hop_neighbors(node_count, edge_count));

        // 3. Node Operations (5 tests)
        println!("\n--- 3. Node Operations ---");
        all_results.extend(benchmark_get_node(node_count, edge_count));
        all_results.extend(benchmark_fetch_outgoing(node_count, edge_count));
        all_results.extend(benchmark_fetch_incoming(node_count, edge_count));
        all_results.extend(benchmark_node_degree(node_count, edge_count));
        all_results.extend(benchmark_insert_node(node_count));

        // 4. Edge Operations (2 tests)
        println!("\n--- 4. Edge Operations ---");
        all_results.extend(benchmark_insert_edge(node_count));
        all_results.extend(benchmark_edge_exists(node_count, edge_count));

        // 5. Neighbor Queries (4 tests)
        println!("\n--- 5. Neighbor Queries ---");
        all_results.extend(benchmark_neighbors_outgoing(node_count, edge_count));
        all_results.extend(benchmark_neighbors_incoming(node_count, edge_count));
        all_results.extend(benchmark_neighbors_both(node_count, edge_count));

        // Skip heavy algorithms for large graphs
        if node_count <= 10000 {
            // 6. Centrality Algorithms (2 tests)
            println!("\n--- 6. Centrality Algorithms ---");
            all_results.extend(benchmark_pagerank(node_count, edge_count));

            // 7. Community Detection (2 tests)
            println!("\n--- 7. Community Detection ---");
            all_results.extend(benchmark_label_propagation(node_count, edge_count));

            // 8. Connected Components (2 tests)
            println!("\n--- 8. Connected Components ---");
            all_results.extend(benchmark_connected_components(node_count, edge_count));
            all_results.extend(benchmark_weakly_connected(node_count, edge_count));
        }
    }

    print_summary(&all_results);
}

/// Populate SQLite backend with test data
fn populate_sqlite_backend(backend: &SqliteGraphBackend, node_count: usize, edge_count: usize) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Create nodes
    for i in 0..node_count {
        backend
            .insert_node(NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }

    // Create edges (random connections)
    for i in 0..edge_count {
        let mut hasher = DefaultHasher::new();
        i.hash(&mut hasher);
        let hash = hasher.finish();

        let from = ((hash % node_count as u64) + 1) as i64;
        let to = (((hash >> 32) % node_count as u64) + 1) as i64;

        if from != to {
            backend
                .insert_edge(EdgeSpec {
                    from,
                    to,
                    edge_type: "Edge".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }
}

/// Populate V3 backend with test data (uses batching)
fn populate_v3_backend(backend: &V3Backend, node_count: usize, edge_count: usize) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Create nodes in batch
    let mut batch = backend.begin_batch();
    for i in 0..node_count {
        batch
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind: "Node".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })
            .unwrap();
    }
    batch.commit().unwrap();

    // Create edges (random connections)
    let mut batch = backend.begin_batch();
    for i in 0..edge_count {
        let mut hasher = DefaultHasher::new();
        i.hash(&mut hasher);
        let hash = hasher.finish();

        let from = ((hash % node_count as u64) + 1) as i64;
        let to = (((hash >> 32) % node_count as u64) + 1) as i64;

        if from != to {
            batch
                .insert_edge(sqlitegraph::backend::EdgeSpec {
                    from,
                    to,
                    edge_type: "Edge".to_string(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }
    batch.commit().unwrap();
}

/// Run benchmark on both backends
fn benchmark_both<F>(name: &str, node_count: usize, edge_count: usize, f: F) -> Vec<BenchmarkResult>
where
    F: Fn(&dyn GraphBackend) -> Result<(), String>,
{
    let mut results = Vec::new();

    // SQLite backend
    let sqlite_backend = SqliteGraphBackend::in_memory().unwrap();
    populate_sqlite_backend(&sqlite_backend, node_count, edge_count);

    let sqlite_start = Instant::now();
    let sqlite_result = f(&sqlite_backend);
    let sqlite_elapsed = sqlite_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: name.to_string(),
        backend: "SQLite".to_string(),
        graph_nodes: node_count,
        graph_edges: edge_count,
        elapsed_ms: sqlite_elapsed.as_secs_f64() * 1000.0,
        success: sqlite_result.is_ok(),
    });

    // V3 backend
    let v3_temp = TempDir::new().unwrap();
    let v3_path = v3_temp.path().join("v3.db");
    let v3_backend = V3Backend::create(&v3_path).unwrap();
    populate_v3_backend(&v3_backend, node_count, edge_count);

    let v3_start = Instant::now();
    let v3_result = f(&v3_backend);
    let v3_elapsed = v3_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: name.to_string(),
        backend: "V3".to_string(),
        graph_nodes: node_count,
        graph_edges: edge_count,
        elapsed_ms: v3_elapsed.as_secs_f64() * 1000.0,
        success: v3_result.is_ok(),
    });

    // Print comparison
    let sqlite_ms = sqlite_elapsed.as_secs_f64() * 1000.0;
    let v3_ms = v3_elapsed.as_secs_f64() * 1000.0;
    let speedup = if v3_ms > 0.0 { sqlite_ms / v3_ms } else { 0.0 };

    println!(
        "  {:30} | SQLite: {:8.2}ms | V3: {:8.2}ms | {:6.2}x {}",
        name,
        sqlite_ms,
        v3_ms,
        speedup,
        if sqlite_result.is_ok() && v3_result.is_ok() {
            "✓"
        } else {
            "✗"
        }
    );

    results
}

// ============================================================================
// 1. Traversal Algorithms (3 tests)
// ============================================================================

fn benchmark_bfs_traversal(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("bfs_traversal", nodes, edges, |backend| {
        let result = bfs_traversal(backend, 1).map_err(|e| e.to_string())?;
        if result.is_empty() && nodes > 1 && edges > 0 {
            return Err("BFS returned no results".to_string());
        }
        Ok(())
    })
}

fn benchmark_dfs_traversal(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("dfs_traversal", nodes, edges, |backend| {
        let result = dfs_traversal(backend, 1).map_err(|e| e.to_string())?;
        if result.is_empty() && nodes > 1 && edges > 0 {
            return Err("DFS returned no results".to_string());
        }
        Ok(())
    })
}

fn benchmark_k_hop(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("k_hop (depth=3)", nodes, edges, |backend| {
        let snapshot = SnapshotId::current();
        let result = backend
            .k_hop(
                snapshot,
                1,
                3,
                sqlitegraph::backend::BackendDirection::Outgoing,
            )
            .map_err(|e| e.to_string())?;
        let _ = result;
        Ok(())
    })
}

// ============================================================================
// 2. Path Algorithms (3 tests)
// ============================================================================

fn benchmark_shortest_path(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("shortest_path", nodes, edges, |backend| {
        let target = std::cmp::min(nodes as i64, 100); // Search for node 100 or last
        let _path = shortest_path(backend, 1, target).map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_bfs_levels(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("bfs (depth=5)", nodes, edges, |backend| {
        let result = bfs(backend, 1, 5).map_err(|e| e.to_string())?;
        let _ = result;
        Ok(())
    })
}

fn benchmark_k_hop_neighbors(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("k_hop_neighbors (k=2)", nodes, edges, |backend| {
        let neighbors = k_hop_neighbors(backend, 1, 2).map_err(|e| e.to_string())?;
        let _ = neighbors;
        Ok(())
    })
}

// ============================================================================
// 3. Node Operations (5 tests)
// ============================================================================

fn benchmark_get_node(nodes: usize, _edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("get_node", nodes, 0, |backend| {
        let snapshot = SnapshotId::current();
        let mid = (nodes / 2) as i64;
        let _ = backend.get_node(snapshot, mid).map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_fetch_outgoing(nodes: usize, _edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("fetch_outgoing", nodes, 0, |backend| {
        let _edges = backend.fetch_outgoing(1).map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_fetch_incoming(nodes: usize, _edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("fetch_incoming", nodes, 0, |backend| {
        let _edges = backend.fetch_incoming(1).map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_node_degree(nodes: usize, _edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("node_degree", nodes, 0, |backend| {
        let snapshot = SnapshotId::current();
        let _degree = backend
            .node_degree(snapshot, 1)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_insert_node(nodes: usize) -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    // SQLite backend
    let sqlite_backend = SqliteGraphBackend::in_memory().unwrap();
    populate_sqlite_backend(&sqlite_backend, nodes, 0);

    let sqlite_start = Instant::now();
    let sqlite_result: Result<(), String> = (|| {
        for i in 0..100 {
            sqlite_backend
                .insert_node(NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("test_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let sqlite_elapsed = sqlite_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: "insert_node (100)".to_string(),
        backend: "SQLite".to_string(),
        graph_nodes: nodes,
        graph_edges: 0,
        elapsed_ms: sqlite_elapsed.as_secs_f64() * 1000.0,
        success: sqlite_result.is_ok(),
    });

    // V3 backend
    let v3_temp = TempDir::new().unwrap();
    let v3_path = v3_temp.path().join("v3.db");
    let v3_backend = V3Backend::create(&v3_path).unwrap();
    populate_v3_backend(&v3_backend, nodes, 0);

    let v3_start = Instant::now();
    let v3_result: Result<(), String> = (|| {
        let mut batch = v3_backend.begin_batch();
        for i in 0..100 {
            batch
                .insert_node(sqlitegraph::backend::NodeSpec {
                    kind: "Test".to_string(),
                    name: format!("test_{}", i),
                    file_path: None,
                    data: serde_json::json!({}),
                })
                .map_err(|e| e.to_string())?;
        }
        batch.commit().map_err(|e| e.to_string())
    })();
    let v3_elapsed = v3_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: "insert_node (100)".to_string(),
        backend: "V3".to_string(),
        graph_nodes: nodes,
        graph_edges: 0,
        elapsed_ms: v3_elapsed.as_secs_f64() * 1000.0,
        success: v3_result.is_ok(),
    });

    // Print comparison
    let sqlite_ms = sqlite_elapsed.as_secs_f64() * 1000.0;
    let v3_ms = v3_elapsed.as_secs_f64() * 1000.0;
    let speedup = if v3_ms > 0.0 { sqlite_ms / v3_ms } else { 0.0 };

    println!(
        "  {:30} | SQLite: {:8.2}ms | V3: {:8.2}ms | {:6.2}x {}",
        "insert_node (100)",
        sqlite_ms,
        v3_ms,
        speedup,
        if sqlite_result.is_ok() && v3_result.is_ok() {
            "✓"
        } else {
            "✗"
        }
    );

    results
}

// ============================================================================
// 4. Edge Operations (2 tests)
// ============================================================================

fn benchmark_insert_edge(nodes: usize) -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    // SQLite backend
    let sqlite_backend = SqliteGraphBackend::in_memory().unwrap();
    populate_sqlite_backend(&sqlite_backend, nodes, 0);

    let sqlite_start = Instant::now();
    let sqlite_result: Result<(), String> = (|| {
        for i in 0..100 {
            let from = ((i % nodes) + 1) as i64;
            let to = (((i + 1) % nodes) + 1) as i64;
            sqlite_backend
                .insert_edge(EdgeSpec {
                    from,
                    to,
                    edge_type: "Test".to_string(),
                    data: serde_json::json!({}),
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let sqlite_elapsed = sqlite_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: "insert_edge (100)".to_string(),
        backend: "SQLite".to_string(),
        graph_nodes: nodes,
        graph_edges: 0,
        elapsed_ms: sqlite_elapsed.as_secs_f64() * 1000.0,
        success: sqlite_result.is_ok(),
    });

    // V3 backend
    let v3_temp = TempDir::new().unwrap();
    let v3_path = v3_temp.path().join("v3.db");
    let v3_backend = V3Backend::create(&v3_path).unwrap();
    populate_v3_backend(&v3_backend, nodes, 0);

    let v3_start = Instant::now();
    let v3_result: Result<(), String> = (|| {
        let mut batch = v3_backend.begin_batch();
        for i in 0..100 {
            let from = ((i % nodes) + 1) as i64;
            let to = (((i + 1) % nodes) + 1) as i64;
            batch
                .insert_edge(sqlitegraph::backend::EdgeSpec {
                    from,
                    to,
                    edge_type: "Test".to_string(),
                    data: serde_json::json!({}),
                })
                .map_err(|e| e.to_string())?;
        }
        batch.commit().map_err(|e| e.to_string())
    })();
    let v3_elapsed = v3_start.elapsed();

    results.push(BenchmarkResult {
        algorithm: "insert_edge (100)".to_string(),
        backend: "V3".to_string(),
        graph_nodes: nodes,
        graph_edges: 0,
        elapsed_ms: v3_elapsed.as_secs_f64() * 1000.0,
        success: v3_result.is_ok(),
    });

    // Print comparison
    let sqlite_ms = sqlite_elapsed.as_secs_f64() * 1000.0;
    let v3_ms = v3_elapsed.as_secs_f64() * 1000.0;
    let speedup = if v3_ms > 0.0 { sqlite_ms / v3_ms } else { 0.0 };

    println!(
        "  {:30} | SQLite: {:8.2}ms | V3: {:8.2}ms | {:6.2}x {}",
        "insert_edge (100)",
        sqlite_ms,
        v3_ms,
        speedup,
        if sqlite_result.is_ok() && v3_result.is_ok() {
            "✓"
        } else {
            "✗"
        }
    );

    results
}

fn benchmark_edge_exists(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("edge_exists", nodes, edges, |backend| {
        let _snapshot = SnapshotId::current();
        // Check if edge from 1 to 2 exists
        let outgoing = backend.fetch_outgoing(1).map_err(|e| e.to_string())?;
        let _exists = outgoing.contains(&2);
        Ok(())
    })
}

// ============================================================================
// 5. Neighbor Queries (3 tests)
// ============================================================================

fn benchmark_neighbors_outgoing(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("neighbors (outgoing)", nodes, edges, |backend| {
        let snapshot = SnapshotId::current();
        let query = sqlitegraph::backend::NeighborQuery {
            direction: sqlitegraph::backend::BackendDirection::Outgoing,
            edge_type: None,
        };
        let _neighbors = backend
            .neighbors(snapshot, 1, query)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_neighbors_incoming(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("neighbors (incoming)", nodes, edges, |backend| {
        let snapshot = SnapshotId::current();
        let query = sqlitegraph::backend::NeighborQuery {
            direction: sqlitegraph::backend::BackendDirection::Incoming,
            edge_type: None,
        };
        let _neighbors = backend
            .neighbors(snapshot, 1, query)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn benchmark_neighbors_both(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("neighbors (undirected)", nodes, edges, |backend| {
        // Get both incoming and outgoing separately
        let snapshot = SnapshotId::current();
        let query_out = sqlitegraph::backend::NeighborQuery {
            direction: sqlitegraph::backend::BackendDirection::Outgoing,
            edge_type: None,
        };
        let query_in = sqlitegraph::backend::NeighborQuery {
            direction: sqlitegraph::backend::BackendDirection::Incoming,
            edge_type: None,
        };
        let _outgoing = backend
            .neighbors(snapshot, 1, query_out)
            .map_err(|e| e.to_string())?;
        let _incoming = backend
            .neighbors(snapshot, 1, query_in)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

// ============================================================================
// 6. Centrality Algorithms (1 test - pagerank)
// ============================================================================

fn benchmark_pagerank(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    use sqlitegraph::algo::backend::pagerank;

    benchmark_both("pagerank", nodes, edges, |backend| {
        let _scores = pagerank(backend, 0.85, 10).map_err(|e| e.to_string())?;
        Ok(())
    })
}

// ============================================================================
// 7. Community Detection (1 test)
// ============================================================================

fn benchmark_label_propagation(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("label_propagation", nodes, edges, |backend| {
        // This requires SqliteGraph, not GraphBackend
        // For now, skip this test for V3
        let _ = backend;
        Err("Requires SqliteGraph - skipped".to_string())
    })
}

// ============================================================================
// 8. Connected Components (2 tests)
// ============================================================================

fn benchmark_connected_components(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("connected_components", nodes, edges, |backend| {
        let _ = backend;
        Err("Requires SqliteGraph - skipped".to_string())
    })
}

fn benchmark_weakly_connected(nodes: usize, edges: usize) -> Vec<BenchmarkResult> {
    benchmark_both("weakly_connected_components", nodes, edges, |backend| {
        let _ = backend;
        Err("Requires SqliteGraph - skipped".to_string())
    })
}

/// Print summary table
fn print_summary(results: &[BenchmarkResult]) {
    println!("\n\n");
    println!(
        "╔══════════════════════════════════════════════════════════════════════════════════════════════════╗"
    );
    println!(
        "║                         ALGORITHM BENCHMARK SUMMARY ({} Tests)",
        results.len()
    );
    println!(
        "╚══════════════════════════════════════════════════════════════════════════════════════════════════╝"
    );

    // Group by graph size
    let mut by_size: std::collections::HashMap<(usize, usize), Vec<&BenchmarkResult>> =
        std::collections::HashMap::new();
    for r in results {
        by_size
            .entry((r.graph_nodes, r.graph_edges))
            .or_default()
            .push(r);
    }

    for ((nodes, edges), group) in by_size {
        println!("\n--- Graph: {} nodes, {} edges ---", nodes, edges);
        println!(
            "{:<35} {:<12} {:<12} {:<10} Status",
            "Algorithm", "SQLite(ms)", "V3(ms)", "Speedup"
        );
        println!("{}", "-".repeat(90));

        // Get unique algorithm names for this size
        let mut algo_names: Vec<_> = group.iter().map(|r| r.algorithm.clone()).collect();
        algo_names.sort();
        algo_names.dedup();

        for algo in algo_names {
            let sqlite = group
                .iter()
                .find(|r| r.algorithm == algo && r.backend == "SQLite");
            let v3 = group
                .iter()
                .find(|r| r.algorithm == algo && r.backend == "V3");

            if let (Some(s), Some(v)) = (sqlite, v3) {
                let speedup = if v.elapsed_ms > 0.0 {
                    s.elapsed_ms / v.elapsed_ms
                } else {
                    0.0
                };
                let status = if s.success && v.success {
                    "✓ OK"
                } else if !s.success && !v.success {
                    "✗ Both Fail"
                } else if !s.success {
                    "✗ SQLite Fail"
                } else {
                    "✗ V3 Fail"
                };

                println!(
                    "{:<35} {:<12.2} {:<12.2} {:<10.2} {}",
                    algo, s.elapsed_ms, v.elapsed_ms, speedup, status
                );
            }
        }
    }

    // Overall statistics
    let total_tests = results.len();
    let passed_tests = results.iter().filter(|r| r.success).count();
    let sqlite_tests = results
        .iter()
        .filter(|r| r.backend == "SQLite" && r.success)
        .count();
    let v3_tests = results
        .iter()
        .filter(|r| r.backend == "V3" && r.success)
        .count();

    println!("\n{}", "=".repeat(90));
    println!(
        "TOTAL: {} tests | Passed: {} | Failed: {}",
        total_tests,
        passed_tests,
        total_tests - passed_tests
    );
    println!(
        "SQLite: {}/{} passed | V3: {}/{} passed",
        sqlite_tests,
        total_tests / 2,
        v3_tests,
        total_tests / 2
    );

    // Calculate average speedup
    let mut speedups = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<&BenchmarkResult>> =
        std::collections::HashMap::new();
    for r in results {
        let key = format!("{}_{}_{}", r.algorithm, r.graph_nodes, r.graph_edges);
        grouped.entry(key).or_default().push(r);
    }
    for (_, group) in grouped {
        if let (Some(s), Some(v)) = (
            group.iter().find(|r| r.backend == "SQLite"),
            group.iter().find(|r| r.backend == "V3"),
        ) && s.success
            && v.success
            && v.elapsed_ms > 0.0
        {
            speedups.push(s.elapsed_ms / v.elapsed_ms);
        }
    }
    if !speedups.is_empty() {
        let avg_speedup: f64 = speedups.iter().sum::<f64>() / speedups.len() as f64;
        let max_speedup = speedups.iter().fold(0.0f64, |a, &b| a.max(b));
        println!(
            "Average Speedup: {:.2}x | Max Speedup: {:.2}x",
            avg_speedup, max_speedup
        );
    }
}
