use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::json;

use crate::{GraphEdge, GraphEntity};

#[derive(Clone, Debug)]
pub struct GraphDataset {
    pub entities: Vec<GraphEntity>,
    pub edges: Vec<GraphEdge>,
}

impl GraphDataset {
    pub fn nodes(&self) -> usize {
        self.entities.len()
    }

    pub fn edges(&self) -> usize {
        self.edges.len()
    }

    pub fn degrees(&self) -> Vec<usize> {
        let mut counts = vec![0usize; self.entities.len()];
        for edge in &self.edges {
            let from = edge.from_id as usize;
            let to = edge.to_id as usize;
            counts[from] += 1;
            counts[to] += 1;
        }
        counts
    }

    pub fn hub_index(&self) -> usize {
        let mut best = (0usize, 0usize);
        for (idx, deg) in self.degrees().into_iter().enumerate() {
            if deg > best.0 {
                best = (deg, idx);
            }
        }
        best.1
    }

    pub fn mapped_edge(edge: &GraphEdge, id_map: &[i64]) -> GraphEdge {
        GraphEdge {
            id: 0,
            from_id: id_map[edge.from_id as usize],
            to_id: id_map[edge.to_id as usize],
            edge_type: edge.edge_type.clone(),
            data: edge.data.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum GraphShape {
    Line,
    Star,
    Grid2D { width: usize, height: usize },
    RandomErdosRenyi { edges: usize },
    ScaleFree { m: usize },
}

pub fn generate_graph(shape: GraphShape, node_count: usize, seed: u64) -> GraphDataset {
    assert!(node_count > 1, "node_count must exceed 1");
    let entities = build_entities(node_count);
    let mut edges = match shape {
        GraphShape::Line => generate_line_edges(node_count),
        GraphShape::Star => generate_star_edges(node_count),
        GraphShape::Grid2D { width, height } => generate_grid_edges(width, height, node_count),
        GraphShape::RandomErdosRenyi { edges } => generate_random_edges(node_count, edges, seed),
        GraphShape::ScaleFree { m } => generate_scale_free_edges(node_count, m, seed),
    };
    edges.sort_by(|a, b| {
        a.from_id
            .cmp(&b.from_id)
            .then_with(|| a.to_id.cmp(&b.to_id))
            .then_with(|| a.edge_type.cmp(&b.edge_type))
    });
    GraphDataset { entities, edges }
}

fn build_entities(count: usize) -> Vec<GraphEntity> {
    (0..count)
        .map(|idx| GraphEntity {
            id: idx as i64,
            kind: "Node".to_string(),
            name: format!("Node{idx}"),
            file_path: None,
            data: json!({ "idx": idx }),
        })
        .collect()
}

fn generate_line_edges(count: usize) -> Vec<GraphEdge> {
    (0..count - 1)
        .map(|idx| new_edge(idx, idx + 1, "LINE"))
        .collect()
}

fn generate_star_edges(count: usize) -> Vec<GraphEdge> {
    (1..count).map(|leaf| new_edge(0, leaf, "STAR")).collect()
}

fn generate_grid_edges(width: usize, height: usize, node_count: usize) -> Vec<GraphEdge> {
    assert_eq!(
        width * height,
        node_count,
        "grid dimensions must match node count"
    );
    let mut edges = Vec::with_capacity(width * height * 2);
    for y in 0..height {
        for x in 0..width {
            let base = grid_index(x, y, width);
            if x + 1 < width {
                edges.push(new_edge(base, grid_index(x + 1, y, width), "GRID"));
            }
            if y + 1 < height {
                edges.push(new_edge(base, grid_index(x, y + 1, width), "GRID"));
            }
        }
    }
    edges
}

fn generate_random_edges(node_count: usize, edge_count: usize, seed: u64) -> Vec<GraphEdge> {
    let total_pairs = pair_count(node_count);
    assert!(
        edge_count as u128 <= total_pairs,
        "edge_count exceeds possible pairs"
    );
    let mut rng = StdRng::seed_from_u64(seed);
    let mut edges = Vec::with_capacity(edge_count);
    let mut idx = 0u64;
    let mut remaining_edges = edge_count as u64;
    while remaining_edges > 0 && idx < total_pairs as u64 {
        let remaining_pairs = total_pairs as u64 - idx;
        let p = remaining_edges as f64 / remaining_pairs as f64;
        let skip = sample_geometric(&mut rng, p);
        idx += skip;
        if idx >= total_pairs as u64 {
            break;
        }
        let (from, to) = pair_from_index(idx, node_count as u64);
        edges.push(new_edge(from as usize, to as usize, "ER"));
        idx += 1;
        remaining_edges -= 1;
    }
    edges
}

fn generate_scale_free_edges(node_count: usize, m: usize, seed: u64) -> Vec<GraphEdge> {
    assert!(m > 0, "m must be positive");
    assert!(node_count > m + 1, "node_count must exceed m + 1");
    let mut rng = StdRng::seed_from_u64(seed);
    let mut degrees = vec![0usize; node_count];
    let mut edges = Vec::new();
    let seed_nodes = m + 1;
    for u in 0..seed_nodes {
        for v in (u + 1)..seed_nodes {
            edges.push(new_edge(u, v, "SF"));
            degrees[u] += 1;
            degrees[v] += 1;
        }
    }
    let mut total_degree: usize = degrees.iter().sum();
    for new_node in seed_nodes..node_count {
        let mut targets = Vec::new();
        while targets.len() < m {
            let pick = rng.gen_range(0..total_degree);
            let mut cumulative = 0usize;
            for (candidate, degree) in degrees.iter().take(new_node).enumerate() {
                cumulative += *degree;
                if pick < cumulative {
                    if !targets.contains(&candidate) {
                        targets.push(candidate);
                    }
                    break;
                }
            }
        }
        targets.sort_unstable();
        targets.dedup();
        while targets.len() < m {
            targets.push(targets.len() % new_node);
            targets.sort_unstable();
            targets.dedup();
        }
        for target in targets {
            edges.push(new_edge(target, new_node, "SF"));
            degrees[target] += 1;
            degrees[new_node] += 1;
            total_degree += 2;
        }
    }
    edges
}

fn new_edge(from: usize, to: usize, label: &str) -> GraphEdge {
    GraphEdge {
        id: 0,
        from_id: from as i64,
        to_id: to as i64,
        edge_type: label.to_string(),
        data: json!({ "label": label }),
    }
}

fn grid_index(x: usize, y: usize, width: usize) -> usize {
    y * width + x
}

fn pair_count(nodes: usize) -> u128 {
    let n = nodes as u128;
    n * (n - 1) / 2
}

fn sample_geometric(rng: &mut StdRng, p: f64) -> u64 {
    let u = rng.r#gen::<f64>().max(f64::MIN_POSITIVE);
    ((u.ln() / (1.0 - p).ln()).floor().max(0.0)) as u64
}

fn pair_from_index(idx: u64, nodes: u64) -> (u64, u64) {
    let mut left = 0;
    let mut start = 0u64;
    while left < nodes - 1 {
        let remaining = nodes - left - 1;
        if idx < start + remaining {
            return (left, left + 1 + (idx - start));
        }
        start += remaining;
        left += 1;
    }
    (nodes - 2, nodes - 1)
}
