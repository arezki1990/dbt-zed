use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

const DATASET_SIZES: &[usize] = &[100, 1000, 10000];
const EDGE_MULTIPLIERS: &[f64] = &[2.0, 2.0, 2.0];
const DENSE_EDGE_MULTIPLIER: f64 = 500.0;

#[derive(Debug)]
struct SimpleAdjList {
    nodes: Vec<u64>,
    outgoing: HashMap<u64, Vec<u64>>,
    incoming: HashMap<u64, Vec<u64>>,
    next_id: u64,
}

impl SimpleAdjList {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            next_id: 0,
        }
    }

    fn add_node(&mut self, _name: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(id);
        self.outgoing.insert(id, Vec::new());
        self.incoming.insert(id, Vec::new());
        id
    }

    fn add_edge(&mut self, src: u64, dst: u64) {
        if let Some(outgoing) = self.outgoing.get_mut(&src)
            && !outgoing.contains(&dst)
        {
            outgoing.push(dst);
        }
        if let Some(incoming) = self.incoming.get_mut(&dst)
            && !incoming.contains(&src)
        {
            incoming.push(src);
        }
    }

    fn neighbors(&self, node: u64, direction: Direction) -> Option<&Vec<u64>> {
        match direction {
            Direction::Outgoing => self.outgoing.get(&node),
            Direction::Incoming => self.incoming.get(&node),
            Direction::Undirected => {
                // Combine both
                None // Simplified for benchmark
            }
        }
    }

    fn bfs(&self, start: u64, max_depth: Option<usize>) -> Vec<u64> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        queue.push_back((start, 0));
        visited.insert(start);

        while let Some((node, depth)) = queue.pop_front() {
            if let Some(max_depth) = max_depth
                && depth > max_depth
            {
                continue;
            }

            result.push(node);

            if let Some(neighbors) = self.outgoing.get(&node) {
                for &neighbor in neighbors {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }

        result
    }

    fn memory_usage(&self) -> usize {
        use std::mem;
        mem::size_of::<Self>()
            + self.nodes.capacity() * mem::size_of::<u64>()
            + self.outgoing.capacity() * mem::size_of::<(u64, Vec<u64>)>()
            + self.incoming.capacity() * mem::size_of::<(u64, Vec<u64>)>()
            + self
                .outgoing
                .values()
                .map(|v| v.capacity() * mem::size_of::<u64>())
                .sum::<usize>()
            + self
                .incoming
                .values()
                .map(|v| v.capacity() * mem::size_of::<u64>())
                .sum::<usize>()
    }
}

#[derive(Debug)]
enum Direction {
    Outgoing,
    Incoming,
    Undirected,
}

fn generate_dataset(seed: u64, num_nodes: usize, edge_multiplier: f64) -> Vec<(u64, u64)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let num_edges = (num_nodes as f64 * edge_multiplier) as usize;
    let mut edges = Vec::with_capacity(num_edges);

    // Create a connected graph first
    for i in 1..num_nodes {
        let j = rng.gen_range(0..i);
        edges.push((j as u64, i as u64));
    }

    // Add random edges
    for _ in edges.len()..num_edges {
        let a = rng.gen_range(0..num_nodes) as u64;
        let b = rng.gen_range(0..num_nodes) as u64;
        if a != b {
            edges.push((a, b));
        }
    }

    edges
}

fn benchmark_adjlist_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("adjlist_create");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for &num_nodes in DATASET_SIZES {
        let edge_multiplier =
            EDGE_MULTIPLIERS[DATASET_SIZES.iter().position(|&n| n == num_nodes).unwrap()];

        group.throughput(Throughput::Elements(num_nodes as u64));

        group.bench_with_input(
            BenchmarkId::new("create_graph", num_nodes),
            &num_nodes,
            |b, &num_nodes| {
                b.iter(|| {
                    let mut graph = SimpleAdjList::new();
                    let edges = generate_dataset(42, num_nodes, edge_multiplier);

                    for i in 0..num_nodes {
                        let _ = graph.add_node(&format!("node_{}", i));
                    }

                    for (src, dst) in edges {
                        if src < num_nodes as u64 && dst < num_nodes as u64 {
                            graph.add_edge(src, dst);
                        }
                    }

                    black_box(graph);
                });
            },
        );
    }

    group.finish();
}

fn benchmark_adjlist_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("adjlist_query");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(1000);

    for &num_nodes in DATASET_SIZES {
        let edge_multiplier =
            EDGE_MULTIPLIERS[DATASET_SIZES.iter().position(|&n| n == num_nodes).unwrap()];

        // Pre-build graph
        let mut graph = SimpleAdjList::new();
        let edges = generate_dataset(42, num_nodes, edge_multiplier);

        for i in 0..num_nodes {
            graph.add_node(&format!("node_{}", i));
        }

        for (src, dst) in edges {
            if src < num_nodes as u64 && dst < num_nodes as u64 {
                graph.add_edge(src, dst);
            }
        }

        // Benchmark neighbor queries
        group.bench_with_input(
            BenchmarkId::new("neighbor_query_outgoing", num_nodes),
            &num_nodes,
            |b, _| {
                let mut rng = StdRng::seed_from_u64(42);
                b.iter(|| {
                    let node_id = rng.gen_range(1..num_nodes) as u64;
                    let neighbors = graph.neighbors(node_id, Direction::Outgoing);
                    black_box(neighbors);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("neighbor_query_incoming", num_nodes),
            &num_nodes,
            |b, _| {
                let mut rng = StdRng::seed_from_u64(42);
                b.iter(|| {
                    let node_id = rng.gen_range(1..num_nodes) as u64;
                    let neighbors = graph.neighbors(node_id, Direction::Incoming);
                    black_box(neighbors);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("neighbor_query_undirected", num_nodes),
            &num_nodes,
            |b, _| {
                let mut rng = StdRng::seed_from_u64(42);
                b.iter(|| {
                    let node_id = rng.gen_range(1..num_nodes) as u64;
                    let neighbors = graph.neighbors(node_id, Direction::Undirected);
                    black_box(neighbors);
                });
            },
        );

        // Benchmark BFS traversal
        group.bench_with_input(
            BenchmarkId::new("bfs_traversal", num_nodes),
            &num_nodes,
            |b, _| {
                b.iter(|| {
                    let visited = graph.bfs(0, Some(5));
                    black_box(visited);
                });
            },
        );
    }

    group.finish();
}

fn benchmark_adjlist_dense(c: &mut Criterion) {
    let mut group = c.benchmark_group("adjlist_dense");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);

    let num_nodes = 1000;

    group.throughput(Throughput::Elements(num_nodes as u64));

    group.bench_with_input(
        BenchmarkId::new("create_dense", num_nodes),
        &num_nodes,
        |b, _| {
            b.iter(|| {
                let mut graph = SimpleAdjList::new();
                let edges = generate_dataset(42, num_nodes, DENSE_EDGE_MULTIPLIER);

                for i in 0..num_nodes {
                    graph.add_node(&format!("node_{}", i));
                }

                for (src, dst) in edges {
                    if src < num_nodes as u64 && dst < num_nodes as u64 {
                        graph.add_edge(src, dst);
                    }
                }

                black_box(graph.memory_usage());
            });
        },
    );

    group.finish();
}

fn benchmark_adjlist_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("adjlist_memory");

    for &num_nodes in DATASET_SIZES {
        let edge_multiplier =
            EDGE_MULTIPLIERS[DATASET_SIZES.iter().position(|&n| n == num_nodes).unwrap()];

        group.bench_with_input(
            BenchmarkId::new("memory_usage", num_nodes),
            &num_nodes,
            |b, &num_nodes| {
                b.iter_with_setup(
                    || {
                        let mut graph = SimpleAdjList::new();
                        let edges = generate_dataset(42, num_nodes, edge_multiplier);

                        for i in 0..num_nodes {
                            graph.add_node(&format!("node_{}", i));
                        }

                        for (src, dst) in edges {
                            if src < num_nodes as u64 && dst < num_nodes as u64 {
                                graph.add_edge(src, dst);
                            }
                        }

                        graph
                    },
                    |graph| {
                        let memory = graph.memory_usage();
                        black_box(memory);
                    },
                );
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_adjlist_creation,
    benchmark_adjlist_queries,
    benchmark_adjlist_dense,
    benchmark_adjlist_memory
);
criterion_main!(benches);
