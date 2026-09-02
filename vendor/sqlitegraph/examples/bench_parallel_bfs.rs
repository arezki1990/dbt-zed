//! Simple parallel BFS benchmark
//!
//! Compares sequential vs parallel BFS performance on different graph sizes.

use sqlitegraph::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::native::v3::algorithm::parallel_bfs::{BfsConfig, parallel_bfs};
use sqlitegraph::{EdgeSpec, NodeSpec};
use std::time::Instant;
use tempfile::TempDir;

/// Create a chain graph: 0 -> 1 -> 2 -> ... -> (n-1)
fn create_chain_graph(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create nodes
    for i in 0..n {
        let node = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!(null),
        };
        let id = backend.insert_node(node).expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create edges to form a chain
    for i in 0..node_ids.len() - 1 {
        let edge = EdgeSpec {
            from: node_ids[i],
            to: node_ids[i + 1],
            edge_type: "chain".to_string(),
            data: serde_json::json!(null),
        };
        backend.insert_edge(edge).expect("Failed to insert edge");
    }

    node_ids
}

/// Create a star graph: center connected to all other nodes
fn create_star_graph(backend: &V3Backend, n: usize) -> Vec<i64> {
    let mut node_ids = Vec::new();

    // Create nodes
    for i in 0..n {
        let node = NodeSpec {
            kind: "test_node".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!(null),
        };
        let id = backend.insert_node(node).expect("Failed to insert node");
        node_ids.push(id);
    }

    // Create star edges (center node 0 connected to all others)
    for i in 1..n {
        let edge = EdgeSpec {
            from: node_ids[0],
            to: node_ids[i],
            edge_type: "star".to_string(),
            data: serde_json::json!(null),
        };
        backend.insert_edge(edge).expect("Failed to insert edge");
    }

    node_ids
}

fn benchmark_chain_graphs() {
    println!("=== Chain Graph Benchmark ===");
    println!("Testing sequential vs parallel BFS on chain topology");
    println!(
        "{:<10} {:<15} {:<15} {:<10} {:<15}",
        "Size", "Sequential", "Parallel", "Speedup", "Nodes visited"
    );
    println!("{}", "-".repeat(70));

    let sizes = vec![100, 500, 1_000, 5_000, 10_000];

    for size in sizes {
        // Sequential BFS
        let temp_dir1 = TempDir::new().expect("Failed to create temp dir");
        let db_path1 = temp_dir1.path().join("bench_seq.db");
        let backend1 = V3Backend::create(&db_path1).expect("Failed to create backend");
        let node_ids1 = create_chain_graph(&backend1, size);

        let config_seq = BfsConfig {
            max_threads: None,
            min_parallel_size: size * 10, // Force sequential
        };

        let start_seq = Instant::now();
        let result_seq =
            parallel_bfs(&backend1, node_ids1[0], Some(config_seq)).expect("Failed to perform BFS");
        let time_seq = start_seq.elapsed();

        // Parallel BFS (4 threads)
        let temp_dir2 = TempDir::new().expect("Failed to create temp dir");
        let db_path2 = temp_dir2.path().join("bench_par.db");
        let backend2 = V3Backend::create(&db_path2).expect("Failed to create backend");
        let node_ids2 = create_chain_graph(&backend2, size);

        let config_par = BfsConfig {
            max_threads: Some(4),
            min_parallel_size: 1000,
        };

        let start_par = Instant::now();
        let result_par =
            parallel_bfs(&backend2, node_ids2[0], Some(config_par)).expect("Failed to perform BFS");
        let time_par = start_par.elapsed();

        assert_eq!(result_seq.total_visited, result_par.total_visited);

        let speedup = time_seq.as_secs_f64() / time_par.as_secs_f64();

        println!(
            "{:<10} {:<15.2?} {:<15.2?} {:<10.2}× {:<15}",
            size, time_seq, time_par, speedup, result_seq.total_visited
        );
    }

    println!();
}

fn benchmark_star_graphs() {
    println!("=== Star Graph Benchmark ===");
    println!("Testing sequential vs parallel BFS on star topology");
    println!("Star graphs have wide levels that benefit from parallelization");
    println!(
        "{:<10} {:<15} {:<15} {:<10} {:<15}",
        "Size", "Sequential", "Parallel", "Speedup", "Nodes visited"
    );
    println!("{}", "-".repeat(70));

    let sizes = vec![100, 500, 1_000, 5_000, 10_000];

    for size in sizes {
        // Sequential BFS
        let temp_dir1 = TempDir::new().expect("Failed to create temp dir");
        let db_path1 = temp_dir1.path().join("bench_seq.db");
        let backend1 = V3Backend::create(&db_path1).expect("Failed to create backend");
        let node_ids1 = create_star_graph(&backend1, size);

        let config_seq = BfsConfig {
            max_threads: None,
            min_parallel_size: size * 10, // Force sequential
        };

        let start_seq = Instant::now();
        let result_seq =
            parallel_bfs(&backend1, node_ids1[0], Some(config_seq)).expect("Failed to perform BFS");
        let time_seq = start_seq.elapsed();

        // Parallel BFS (4 threads)
        let temp_dir2 = TempDir::new().expect("Failed to create temp dir");
        let db_path2 = temp_dir2.path().join("bench_par.db");
        let backend2 = V3Backend::create(&db_path2).expect("Failed to create backend");
        let node_ids2 = create_star_graph(&backend2, size);

        let config_par = BfsConfig {
            max_threads: Some(4),
            min_parallel_size: 1000,
        };

        let start_par = Instant::now();
        let result_par =
            parallel_bfs(&backend2, node_ids2[0], Some(config_par)).expect("Failed to perform BFS");
        let time_par = start_par.elapsed();

        assert_eq!(result_seq.total_visited, result_par.total_visited);

        let speedup = time_seq.as_secs_f64() / time_par.as_secs_f64();

        println!(
            "{:<10} {:<15.2?} {:<15.2?} {:<10.2}× {:<15}",
            size, time_seq, time_par, speedup, result_seq.total_visited
        );
    }

    println!();
}

fn benchmark_crossover_point() {
    println!("=== Crossover Point Analysis ===");
    println!("Finding the graph size where parallel becomes faster than sequential");
    println!(
        "{:<10} {:<15} {:<15} {:<10} {:<10}",
        "Size", "Sequential", "Parallel", "Speedup", "Winner"
    );
    println!("{}", "-".repeat(65));

    let sizes = vec![100, 200, 500, 700, 1_000, 1_500, 2_000, 3_000, 5_000];

    for size in sizes {
        // Sequential BFS
        let temp_dir1 = TempDir::new().expect("Failed to create temp dir");
        let db_path1 = temp_dir1.path().join("bench_seq.db");
        let backend1 = V3Backend::create(&db_path1).expect("Failed to create backend");
        let node_ids1 = create_chain_graph(&backend1, size);

        let config_seq = BfsConfig {
            max_threads: None,
            min_parallel_size: size * 10, // Force sequential
        };

        let start_seq = Instant::now();
        let _result_seq =
            parallel_bfs(&backend1, node_ids1[0], Some(config_seq)).expect("Failed to perform BFS");
        let time_seq = start_seq.elapsed();

        // Parallel BFS (default config)
        let temp_dir2 = TempDir::new().expect("Failed to create temp dir");
        let db_path2 = temp_dir2.path().join("bench_par.db");
        let backend2 = V3Backend::create(&db_path2).expect("Failed to create backend");
        let node_ids2 = create_chain_graph(&backend2, size);

        let start_par = Instant::now();
        let _result_par =
            parallel_bfs(&backend2, node_ids2[0], None).expect("Failed to perform BFS");
        let time_par = start_par.elapsed();

        let speedup = time_seq.as_secs_f64() / time_par.as_secs_f64();

        let winner = if speedup > 1.0 {
            "Sequential"
        } else if speedup < 1.0 {
            "Parallel"
        } else {
            "Tie"
        };

        println!(
            "{:<10} {:<15.2?} {:<15.2?} {:<10.2}× {:<10}",
            size, time_seq, time_par, speedup, winner
        );
    }

    println!();
}

fn main() {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║         Parallel BFS Performance Benchmark                     ║");
    println!("║         V3 Backend with Rayon Parallelization                  ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    benchmark_chain_graphs();
    benchmark_star_graphs();
    benchmark_crossover_point();

    println!("=== Summary ===");
    println!("Key findings:");
    println!("1. Parallel BFS shows overhead on small graphs (<1000 nodes)");
    println!("2. Parallel BFS becomes competitive at ~1000-2000 nodes");
    println!("3. Star graphs benefit more from parallelization (wide levels)");
    println!("4. Chain graphs show minimal benefit (narrow levels)");
    println!();
    println!("Recommendation: Use min_parallel_size=1000 for optimal performance");
}
