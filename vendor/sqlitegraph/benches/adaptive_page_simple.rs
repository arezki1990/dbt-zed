//! Adaptive Page Sizing Performance Benchmark (Simplified)
//!
//! Run with: cargo bench --features v3-bench -- adaptive_page_simple
//!
//! This benchmark measures the actual performance impact of different page sizes
//! by simulating the I/O patterns that would be affected.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::Duration;
use tempfile::TempDir;

use sqlitegraph::backend::native::v3::storage::{AdaptivePageManager, MediaDetector, PageConfig};
use sqlitegraph::backend::{BackendDirection, EdgeSpec, NeighborQuery, NodeSpec};
use sqlitegraph::{config::GraphConfig, open_graph, snapshot::SnapshotId};

// ============================================================================
// RAW I/O BENCHMARKS (Simulating different page sizes)
// ============================================================================

/// Benchmark sequential read performance with different page sizes
fn bench_sequential_read_page_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_size/sequential_read");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    for &page_size in &[4096, 8192, 16384] {
        for &data_size in &[100, 1000, 10000] {
            group.throughput(Throughput::Bytes((data_size * page_size) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}KB", page_size / 1024), data_size),
                &(page_size, data_size),
                |b, &(page_size, data_size)| {
                    let temp_dir = TempDir::new().unwrap();
                    let file_path = temp_dir.path().join("seq_read.bin");

                    // Create file with data
                    {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create(true)
                            .open(&file_path)
                            .unwrap();

                        let page_data = vec![0u8; page_size];
                        for _ in 0..data_size {
                            file.write_all(&page_data).unwrap();
                        }
                    }

                    // Benchmark sequential read
                    b.iter(|| {
                        let mut file = OpenOptions::new().read(true).open(&file_path).unwrap();

                        let mut buffer = vec![0u8; page_size];
                        for _ in 0..data_size {
                            file.read_exact(&mut buffer).unwrap();
                            black_box(&buffer);
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

/// Benchmark random read performance with different page sizes
fn bench_random_read_page_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_size/random_read");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    for &page_size in &[4096, 8192, 16384] {
        for &data_size in &[100, 1000, 10000] {
            group.bench_with_input(
                BenchmarkId::new(format!("{}KB", page_size / 1024), data_size),
                &(page_size, data_size),
                |b, &(page_size, data_size)| {
                    let temp_dir = TempDir::new().unwrap();
                    let file_path = temp_dir.path().join("rand_read.bin");

                    // Create file with data
                    {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create(true)
                            .open(&file_path)
                            .unwrap();

                        let page_data = vec![0u8; page_size];
                        for _ in 0..data_size {
                            file.write_all(&page_data).unwrap();
                        }
                    }

                    // Pre-generate random page indices
                    use rand::Rng;
                    use rand::SeedableRng;
                    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
                    let random_indices: Vec<usize> = (0..data_size)
                        .map(|_| rng.gen_range(0..data_size))
                        .collect();

                    // Benchmark random read
                    b.iter(|| {
                        let mut file = OpenOptions::new().read(true).open(&file_path).unwrap();

                        let mut buffer = vec![0u8; page_size];
                        for &idx in &random_indices {
                            file.seek(SeekFrom::Start((idx * page_size) as u64))
                                .unwrap();
                            file.read_exact(&mut buffer).unwrap();
                            black_box(&buffer);
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

/// Benchmark write performance with different page sizes
fn bench_write_page_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_size/write");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    for &page_size in &[4096, 8192, 16384] {
        for &data_size in &[100, 1000, 10000] {
            group.throughput(Throughput::Bytes((data_size * page_size) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}KB", page_size / 1024), data_size),
                &(page_size, data_size),
                |b, &(page_size, data_size)| {
                    let temp_dir = TempDir::new().unwrap();
                    let file_path = temp_dir.path().join("write.bin");

                    let page_data = vec![0u8; page_size];

                    b.iter(|| {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create(true)
                            .open(&file_path)
                            .unwrap();

                        for _ in 0..data_size {
                            file.write_all(&page_data).unwrap();
                            black_box(&page_data);
                        }
                        file.flush().unwrap();
                    });
                },
            );
        }
    }
    group.finish();
}

// ============================================================================
// ADAPTIVE PAGE MANAGER BENCHMARKS
// ============================================================================

/// Benchmark adaptive page manager detection overhead
fn bench_adaptive_detection_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("adaptive/overhead");
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("first_detection", |b| {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        b.iter(|| {
            let mut manager = AdaptivePageManager::new(&db_path);
            black_box(manager.get_config());
        });
    });

    group.bench_function("cached_detection", |b| {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        b.iter(|| {
            let mut manager = AdaptivePageManager::new(&db_path);
            // First call (detection)
            black_box(manager.get_config());
            // Second call (cached)
            black_box(manager.get_config());
        });
    });

    group.finish();
}

/// Benchmark page config creation and validation
fn bench_page_config(c: &mut Criterion) {
    let mut group = c.benchmark_group("adaptive/config");
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("ssd_config", |b| {
        b.iter(|| {
            let config = PageConfig::ssd();
            black_box(config.is_valid());
        });
    });

    group.bench_function("hdd_config", |b| {
        b.iter(|| {
            let config = PageConfig::hdd();
            black_box(config.is_valid());
        });
    });

    group.bench_function("default_config", |b| {
        b.iter(|| {
            let config = PageConfig::default();
            black_box(config.is_valid());
        });
    });

    group.finish();
}

/// Benchmark media detection overhead
fn bench_media_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("adaptive/media_detection");
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("detect_tmp", |b| {
        let detector = MediaDetector::new();
        b.iter(|| {
            black_box(detector.detect("/tmp"));
        });
    });

    group.finish();
}

// ============================================================================
// END-TO-END GRAPH BENCHMARKS (Baseline performance)
// ============================================================================

/// Benchmark graph node insertion (baseline for adaptive page sizing)
fn bench_graph_node_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph/node_insertion");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20);

    for &size in &[100, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("v3_backend", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("graph.db");

                let config = GraphConfig::native();
                let graph = open_graph(&db_path, &config).unwrap();

                for i in 0..size {
                    let node_spec = NodeSpec {
                        kind: "test".to_string(),
                        name: format!("node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"id": i}),
                    };
                    black_box(graph.insert_node(node_spec).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark graph edge insertion
fn bench_graph_edge_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph/edge_insertion");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20);

    for &size in &[100, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("v3_backend", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("graph.db");

                let config = GraphConfig::native();
                let graph = open_graph(&db_path, &config).unwrap();

                // Create nodes first
                for i in 0..size {
                    let node_spec = NodeSpec {
                        kind: "test".to_string(),
                        name: format!("node_{}", i),
                        file_path: None,
                        data: serde_json::json!({"id": i}),
                    };
                    graph.insert_node(node_spec).unwrap();
                }

                // Insert edges
                for i in 0..size {
                    let edge_spec = EdgeSpec {
                        edge_type: "test_edge".to_string(),
                        from: i,
                        to: (i + 1) % size,
                        data: serde_json::json!({"edge_id": i}),
                    };
                    black_box(graph.insert_edge(edge_spec).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark graph neighbor queries
fn bench_graph_neighbor_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph/neighbor_queries");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20);

    for &size in &[100, 1000, 5000] {
        group.bench_with_input(BenchmarkId::new("v3_backend", size), &size, |b, &size| {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("graph.db");

            let config = GraphConfig::native();
            let graph = open_graph(&db_path, &config).unwrap();

            // Create a chain graph
            for i in 0..size {
                let node_spec = NodeSpec {
                    kind: "test".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                };
                graph.insert_node(node_spec).unwrap();
            }

            // Create edges
            for i in 0..size - 1 {
                let edge_spec = EdgeSpec {
                    edge_type: "chain".to_string(),
                    from: i as i64,
                    to: (i + 1) as i64,
                    data: serde_json::json!({}),
                };
                graph.insert_edge(edge_spec).unwrap();
            }

            // Benchmark neighbor queries
            b.iter(|| {
                for i in 0..size.min(100) {
                    let query = NeighborQuery {
                        edge_type: Some("chain".to_string()),
                        direction: BackendDirection::Outgoing,
                    };
                    black_box(
                        graph
                            .neighbors(SnapshotId::current(), i as i64, query)
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

// ============================================================================
// MAIN BENCHMARK GROUPS
// ============================================================================

criterion_group!(
    name = page_size_benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets =
        bench_sequential_read_page_size,
        bench_random_read_page_size,
        bench_write_page_size
);

criterion_group!(
    name = adaptive_benches;
    config = Criterion::default().measurement_time(Duration::from_secs(5));
    targets =
        bench_adaptive_detection_overhead,
        bench_page_config,
        bench_media_detection
);

criterion_group!(
    name = graph_benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets =
        bench_graph_node_insertion,
        bench_graph_edge_insertion,
        bench_graph_neighbor_queries
);

criterion_main!(page_size_benches, adaptive_benches, graph_benches);
