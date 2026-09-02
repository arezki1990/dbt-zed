use serde_json::json;
use sqlitegraph::backend::{BackendDirection, ChainStep};
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph, multi_hop};

fn insert_node(graph: &SqliteGraph, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "Item".into(),
            name: name.into(),
            file_path: None,
            data: json!({"name": name}),
        })
        .expect("insert node")
}

fn insert_edge(graph: &SqliteGraph, from: i64, to: i64, edge_type: &str) {
    graph
        .insert_edge(&GraphEdge {
            id: 0,
            from_id: from,
            to_id: to,
            edge_type: edge_type.into(),
            data: json!({}),
        })
        .expect("insert edge");
}

fn build_sample_graph() -> (SqliteGraph, Vec<i64>) {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let a = insert_node(&graph, "A");
    let b = insert_node(&graph, "B");
    let c = insert_node(&graph, "C");
    let d = insert_node(&graph, "D");
    let e = insert_node(&graph, "E");

    insert_edge(&graph, a, b, "CALLS");
    insert_edge(&graph, b, c, "CALLS");
    insert_edge(&graph, c, d, "USES");
    insert_edge(&graph, a, e, "USES");
    insert_edge(&graph, e, d, "CALLS");

    (graph, vec![a, b, c, d, e])
}

#[test]
fn test_k_hop_outgoing_depth_two() {
    let (graph, ids) = build_sample_graph();
    let hops = graph.query().k_hop_outgoing(ids[0], 2).expect("k-hop");
    assert_eq!(hops, vec![ids[1], ids[4], ids[2], ids[3]]);
}

#[test]
fn test_k_hop_filtered_by_type() {
    let (graph, ids) = build_sample_graph();
    let hops = graph
        .query()
        .k_hop_filtered(ids[0], 3, BackendDirection::Outgoing, &["CALLS"])
        .expect("k-hop filtered");
    assert_eq!(hops, vec![ids[1], ids[2]]);
}

#[test]
fn test_chain_query_with_direction_and_type() {
    let (graph, ids) = build_sample_graph();
    let query = graph.query();
    let chain = [
        ChainStep {
            direction: BackendDirection::Outgoing,
            edge_type: Some("CALLS".into()),
        },
        ChainStep {
            direction: BackendDirection::Outgoing,
            edge_type: Some("CALLS".into()),
        },
        ChainStep {
            direction: BackendDirection::Outgoing,
            edge_type: Some("USES".into()),
        },
    ];
    let matches = query.chain(ids[0], &chain).expect("chain query");
    assert_eq!(matches, vec![ids[3]]);
}

#[test]
fn multi_source_khop_matches_union_of_single_runs() {
    let (graph, ids) = build_sample_graph();
    let seeds = vec![ids[0], ids[1]];
    let mut expected = Vec::new();
    for &seed in &seeds {
        let hops = graph.query().k_hop_outgoing(seed, 2).expect("single hop");
        expected.extend(hops);
    }
    expected.sort();
    expected.dedup();
    expected.retain(|id| !seeds.contains(id));
    expected.sort();
    let mut combined =
        multi_hop::k_hop_multi(&graph, &seeds, 2, BackendDirection::Outgoing).expect("multi hop");
    combined.sort();
    assert_eq!(combined, expected);
}
