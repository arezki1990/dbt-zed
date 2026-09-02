use serde_json::json;
use sqlitegraph::{
    GraphEdge, GraphEntity, SqliteGraph,
    bfs::{bfs_neighbors, shortest_path},
    query::GraphQuery,
};

fn complex_graph() -> SqliteGraph {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    for idx in 0..20 {
        graph
            .insert_entity(&GraphEntity {
                id: 0,
                kind: if idx % 2 == 0 { "Function" } else { "Type" }.into(),
                name: format!("Node{idx}"),
                file_path: Some(format!("src/node_{idx}.rs")),
                data: json!({ "idx": idx }),
            })
            .unwrap();
    }

    let edges = vec![
        (1, 2, "CALLS"),
        (2, 3, "CALLS"),
        (3, 4, "CALLS"),
        (4, 5, "CALLS"),
        (5, 6, "CALLS"),
        (6, 7, "CALLS"),
        (2, 8, "USES_TYPE"),
        (8, 9, "USES_TYPE"),
        (9, 10, "USES_TYPE"),
        (3, 11, "INCLUDES"),
        (11, 12, "INCLUDES"),
        (12, 13, "INCLUDES"),
        (7, 14, "MEMBER_OF"),
        (14, 15, "MEMBER_OF"),
        (15, 16, "MEMBER_OF"),
        (9, 17, "USES_TYPE"),
        (17, 18, "CALLS"),
        (18, 19, "CALLS"),
    ];

    for &(from, to, ty) in &edges {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: from,
                to_id: to,
                edge_type: ty.into(),
                data: json!({ "kind": ty }),
            })
            .unwrap();
    }

    graph
}

#[test]
fn test_integration_call_graph_traversal() {
    let graph = complex_graph();
    let visited = bfs_neighbors(&graph, 1, 6).expect("bfs");
    assert_eq!(
        visited,
        vec![1, 2, 3, 8, 4, 11, 9, 5, 12, 10, 17, 6, 13, 18, 7, 19]
    );
}

#[test]
fn test_integration_include_graph_traversal() {
    let graph = complex_graph();
    let q = GraphQuery::new(&graph);
    assert_eq!(q.neighbors(3).unwrap(), vec![4, 11]);
    assert_eq!(q.incoming(11).unwrap(), vec![3]);
}

#[test]
fn test_integration_component_size() {
    let graph = complex_graph();
    let visited = bfs_neighbors(&graph, 2, 10).unwrap();
    assert_eq!(visited.len(), 18);
}

#[test]
fn test_integration_shortest_path_code_example() {
    let graph = complex_graph();
    let path = shortest_path(&graph, 2, 13).unwrap();
    assert_eq!(path, Some(vec![2, 3, 11, 12, 13]));
}

#[test]
fn test_integration_isolated_node() {
    let graph = complex_graph();
    let visited = bfs_neighbors(&graph, 20, 5).unwrap();
    assert_eq!(visited, vec![20]);
}

#[test]
fn test_integration_has_path_between_far_nodes() {
    let graph = complex_graph();
    let q = graph.query();
    assert!(q.has_path(2, 19, 10).unwrap());
}

#[test]
fn test_integration_member_chain_neighbors() {
    let graph = complex_graph();
    let q = graph.query();
    assert_eq!(q.neighbors(14).unwrap(), vec![15]);
    assert_eq!(q.incoming(15).unwrap(), vec![14]);
}

#[test]
fn test_integration_shortest_path_to_tail_is_deterministic() {
    let graph = complex_graph();
    let path = shortest_path(&graph, 9, 19).unwrap();
    assert_eq!(path, Some(vec![9, 17, 18, 19]));
}

#[test]
fn test_integration_multi_hop_bfs_ordering() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let ids: Vec<_> = (0..6)
        .map(|idx| {
            graph
                .insert_entity(&GraphEntity {
                    id: 0,
                    kind: "Node".into(),
                    name: format!("Node{idx}"),
                    file_path: None,
                    data: json!({ "idx": idx }),
                })
                .unwrap()
        })
        .collect();

    let mut edges = vec![(0, 2), (0, 1), (1, 3), (1, 4), (2, 5)];
    for (from, to) in edges.drain(..) {
        graph
            .insert_edge(&GraphEdge {
                id: 0,
                from_id: ids[from],
                to_id: ids[to],
                edge_type: "LINK".into(),
                data: json!({}),
            })
            .unwrap();
    }

    let visited = bfs_neighbors(&graph, ids[0], 2).unwrap();
    assert_eq!(
        visited,
        vec![ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]]
    );
}
