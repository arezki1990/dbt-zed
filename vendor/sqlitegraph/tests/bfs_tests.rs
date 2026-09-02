use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, SqliteGraph,
    backend::BackendDirection,
    bfs::{bfs_neighbors, bfs_neighbors_filtered, shortest_path, shortest_path_filtered},
};

fn build_graph(edges: &[(i64, i64)]) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    for idx in 0..10 {
        let name = format!("node_{idx}");
        graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "Node".to_string(),
                name,
                file_path: None,
                data: json!({}),
            })
            .unwrap();
    }
    for &(from, to) in edges {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: from,
                to_id: to,
                edge_type: "LINK".to_string(),
                data: json!({}),
            })
            .unwrap();
    }
    graph
}

fn build_typed_graph(edges: &[(i64, i64, &str)]) -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    for idx in 0..10 {
        let name = format!("node_{idx}");
        graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: "Node".to_string(),
                name,
                file_path: None,
                data: json!({}),
            })
            .unwrap();
    }
    for &(from, to, edge_type) in edges {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: from,
                to_id: to,
                edge_type: edge_type.to_string(),
                data: json!({}),
            })
            .unwrap();
    }
    graph
}

#[test]
fn test_bfs_traversal_single_component() {
    let edges = vec![(1, 2), (2, 3), (3, 4), (4, 5)];
    let graph = build_graph(&edges);
    let visited = bfs_neighbors(&graph, 1, 10).expect("bfs");
    assert_eq!(visited, vec![1, 2, 3, 4, 5]);
}

#[test]
fn test_bfs_traversal_disconnected_graph() {
    let edges = vec![(1, 2), (2, 3), (6, 7)];
    let graph = build_graph(&edges);
    let visited = bfs_neighbors(&graph, 6, 10).expect("bfs");
    assert_eq!(visited, vec![6, 7]);
}

#[test]
fn test_shortest_path_exists() {
    let edges = vec![(1, 2), (2, 3), (1, 4), (4, 3)];
    let graph = build_graph(&edges);
    let path = shortest_path(&graph, 1, 3).expect("shortest");
    assert_eq!(path, Some(vec![1, 2, 3]));
}

#[test]
fn test_shortest_path_not_exists() {
    let edges = vec![(1, 2), (3, 4)];
    let graph = build_graph(&edges);
    let path = shortest_path(&graph, 1, 4).expect("shortest");
    assert_eq!(path, None);
}

#[test]
fn test_bfs_deterministic_with_same_insert_order() {
    let edges = vec![(1, 3), (1, 2), (2, 4), (2, 5)];
    let graph = build_graph(&edges);
    let visited = bfs_neighbors(&graph, 1, 10).expect("bfs");
    assert_eq!(visited, vec![1, 2, 3, 4, 5]);
}

#[test]
fn test_bfs_deterministic_with_shuffled_insert_order() {
    let mut edges = vec![(2, 4), (1, 3), (1, 2), (2, 5)];
    let graph = build_graph(&edges);
    edges.reverse();
    let graph_b = build_graph(&edges);
    let visited_a = bfs_neighbors(&graph, 1, 10).expect("bfs A");
    let visited_b = bfs_neighbors(&graph_b, 1, 10).expect("bfs B");
    assert_eq!(visited_a, visited_b);
}

#[test]
fn test_bfs_respects_max_depth() {
    let edges = vec![(1, 2), (2, 3), (3, 4)];
    let graph = build_graph(&edges);
    let visited = bfs_neighbors(&graph, 1, 1).expect("bfs");
    assert_eq!(visited, vec![1, 2]);
}

#[test]
fn test_shortest_path_prefers_lexicographic_neighbors() {
    let edges = vec![(1, 2), (1, 3), (2, 4), (3, 4)];
    let graph = build_graph(&edges);
    let path = shortest_path(&graph, 1, 4).expect("shortest");
    assert_eq!(path, Some(vec![1, 2, 4]));
}

#[test]
fn test_bfs_neighbors_filtered_restricts_traversal_to_allowed_edge_types() {
    let edges = vec![
        (1, 2, "CALL"),
        (2, 3, "CALL"),
        (1, 4, "IMPORTS"),
        (4, 5, "IMPORTS"),
    ];
    let graph = build_typed_graph(&edges);
    let visited =
        bfs_neighbors_filtered(&graph, 1, 10, &["CALL"], BackendDirection::Outgoing).expect("bfs");
    assert_eq!(visited, vec![1, 2, 3]);
}

#[test]
fn test_bfs_neighbors_filtered_empty_allowed_types_returns_empty() {
    let edges = vec![(1, 2, "CALL"), (2, 3, "CALL")];
    let graph = build_typed_graph(&edges);
    let visited =
        bfs_neighbors_filtered(&graph, 1, 10, &[], BackendDirection::Outgoing).expect("bfs");
    assert!(visited.is_empty());
}

#[test]
fn test_bfs_neighbors_filtered_multiple_kinds_unions_neighbors() {
    let edges = vec![
        (1, 2, "CALL"),
        (1, 3, "IMPORTS"),
        (1, 4, "TESTS"),
        (2, 5, "CALL"),
    ];
    let graph = build_typed_graph(&edges);
    let visited = bfs_neighbors_filtered(
        &graph,
        1,
        10,
        &["CALL", "IMPORTS"],
        BackendDirection::Outgoing,
    )
    .expect("bfs");
    assert!(visited.contains(&2));
    assert!(visited.contains(&3));
    assert!(visited.contains(&5));
    assert!(!visited.contains(&4));
}

#[test]
fn test_bfs_neighbors_filtered_incoming_direction() {
    let edges = vec![(1, 5, "CALL"), (2, 5, "CALL"), (3, 5, "IMPORTS")];
    let graph = build_typed_graph(&edges);
    let visited =
        bfs_neighbors_filtered(&graph, 5, 10, &["CALL"], BackendDirection::Incoming).expect("bfs");
    assert!(visited.contains(&5));
    assert!(visited.contains(&1));
    assert!(visited.contains(&2));
    assert!(!visited.contains(&3));
}

#[test]
fn test_shortest_path_filtered_uses_only_allowed_kinds() {
    let edges = vec![
        (1, 2, "CALL"),
        (2, 3, "CALL"),
        (1, 4, "IMPORTS"),
        (4, 3, "IMPORTS"),
    ];
    let graph = build_typed_graph(&edges);
    let path = shortest_path_filtered(&graph, 1, 3, &["CALL"]).expect("shortest");
    assert_eq!(path, Some(vec![1, 2, 3]));
    let path_imports = shortest_path_filtered(&graph, 1, 3, &["IMPORTS"]).expect("shortest");
    assert_eq!(path_imports, Some(vec![1, 4, 3]));
}

#[test]
fn test_shortest_path_filtered_returns_none_when_kind_excludes_only_path() {
    let edges = vec![(1, 2, "IMPORTS"), (2, 3, "IMPORTS")];
    let graph = build_typed_graph(&edges);
    let path = shortest_path_filtered(&graph, 1, 3, &["CALL"]).expect("shortest");
    assert_eq!(path, None);
}

#[test]
fn test_shortest_path_filtered_empty_allowed_types_returns_none() {
    let edges = vec![(1, 2, "CALL"), (2, 3, "CALL")];
    let graph = build_typed_graph(&edges);
    let path = shortest_path_filtered(&graph, 1, 3, &[]).expect("shortest");
    assert_eq!(path, None);
}

#[test]
fn test_shortest_path_filtered_same_node_returns_singleton() {
    let edges = vec![(1, 2, "CALL")];
    let graph = build_typed_graph(&edges);
    let path = shortest_path_filtered(&graph, 1, 1, &["CALL"]).expect("shortest");
    assert_eq!(path, Some(vec![1]));
}
