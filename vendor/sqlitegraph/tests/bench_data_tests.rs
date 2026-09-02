use sqlitegraph::bench_utils::{GraphDataset, GraphShape, generate_graph};

fn assert_sorted(dataset: &GraphDataset) {
    let mut last = None;
    for edge in &dataset.edges {
        let key = (edge.from_id, edge.to_id);
        if let Some(prev) = last {
            assert!(prev <= key, "edges not sorted lexicographically");
        }
        last = Some(key);
    }
}

#[test]
fn test_line_graph_counts() {
    let dataset = generate_graph(GraphShape::Line, 1024, 7);
    assert_eq!(dataset.nodes(), 1024);
    assert_eq!(dataset.edges(), 1023);
    assert_sorted(&dataset);
}

#[test]
fn test_star_graph_counts() {
    let dataset = generate_graph(GraphShape::Star, 2048, 9);
    assert_eq!(dataset.nodes(), 2048);
    assert_eq!(dataset.edges(), 2047);
    assert!(dataset.edges.iter().all(|edge| edge.from_id == 0));
    assert_sorted(&dataset);
}

#[test]
fn test_grid_graph_counts() {
    let dataset = generate_graph(
        GraphShape::Grid2D {
            width: 64,
            height: 64,
        },
        4096,
        11,
    );
    let expected = (63 * 64) * 2;
    assert_eq!(dataset.nodes(), 4096);
    assert_eq!(dataset.edges(), expected);
    assert_sorted(&dataset);
}

#[test]
fn test_random_er_deterministic_edges() {
    let first = generate_graph(GraphShape::RandomErdosRenyi { edges: 500 }, 2000, 42);
    let second = generate_graph(GraphShape::RandomErdosRenyi { edges: 500 }, 2000, 42);
    assert_eq!(first.edges(), second.edges());
    for (edge_a, edge_b) in first.edges.iter().zip(second.edges.iter()) {
        assert_eq!(edge_a.from_id, edge_b.from_id);
        assert_eq!(edge_a.to_id, edge_b.to_id);
    }
    assert_sorted(&first);
}

#[test]
fn test_scale_free_degree_profile() {
    let dataset = generate_graph(GraphShape::ScaleFree { m: 4 }, 4096, 1337);
    let expected_edges = ((5 * 4) / 2) + (4096 - 5) * 4;
    assert_eq!(dataset.edges(), expected_edges);
    assert!(dataset.hub_index() < dataset.nodes());
    let degrees = dataset.degrees();
    let hub_degree = degrees[dataset.hub_index()];
    let other_degree = degrees[1024];
    assert!(hub_degree > other_degree);
    assert_sorted(&dataset);
}
