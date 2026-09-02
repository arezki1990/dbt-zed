use serde_json::json;
use sqlitegraph::{
    graph::SqliteGraph,
    graph_opt::{
        GraphEdgeCreate, GraphEntityCreate, adjacency_fetch_outgoing_batch, bulk_insert_edges,
        bulk_insert_entities, cache_clear_ranges, cache_stats,
    },
};

fn graph() -> SqliteGraph {
    SqliteGraph::open_in_memory().expect("graph")
}

#[test]
fn test_bulk_insert_vs_single_insert_equivalence() {
    let graph = graph();
    let expected = graph
        .insert_entity(&sqlitegraph::graph::GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "single".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    let ids = bulk_insert_entities(
        &graph,
        &[GraphEntityCreate {
            kind: "Fn".into(),
            name: "single".into(),
            file_path: None,
            data: json!({}),
        }],
    )
    .expect("bulk");
    assert_eq!(ids.len(), 1);
    let manual = graph.get_entity(ids[0]).unwrap();
    assert_eq!(manual.kind, "Fn");
    assert!(manual.id > expected);
}

#[test]
fn test_batch_outgoing_matches_scalar_fetch() {
    let graph = graph();
    let a = graph
        .insert_entity(&sqlitegraph::graph::GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "a".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    let b = graph
        .insert_entity(&sqlitegraph::graph::GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "b".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    graph
        .insert_edge(&sqlitegraph::graph::GraphEdge {
            id: 0,
            from_id: a,
            to_id: b,
            edge_type: "CALLS".into(),
            data: json!({}),
        })
        .unwrap();
    let batch = adjacency_fetch_outgoing_batch(&graph, &[a]).expect("batch");
    assert_eq!(batch[0].1, vec![b]);
}

#[test]
fn test_batch_order_is_deterministic() {
    let graph = graph();
    let ids = (0..3)
        .map(|i| {
            graph
                .insert_entity(&sqlitegraph::graph::GraphEntity {
                    id: 0,
                    kind: "Fn".into(),
                    name: format!("n{i}"),
                    file_path: None,
                    data: json!({}),
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let first = adjacency_fetch_outgoing_batch(&graph, &ids).unwrap();
    let second = adjacency_fetch_outgoing_batch(&graph, &ids).unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_cache_stats_changes_on_hits_and_misses() {
    let graph = graph();
    let ids = (0..2)
        .map(|i| {
            graph
                .insert_entity(&sqlitegraph::graph::GraphEntity {
                    id: 0,
                    kind: "Fn".into(),
                    name: format!("n{i}"),
                    file_path: None,
                    data: json!({}),
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let start = cache_stats(&graph);
    let query = graph.query();
    query.neighbors(ids[0]).unwrap();
    query.neighbors(ids[0]).unwrap();
    let after = cache_stats(&graph);
    assert!(after.misses > start.misses);
    assert!(after.hits > start.hits);
    cache_clear_ranges(&graph, &ids);
    let cleared = cache_stats(&graph);
    assert_eq!(cleared.entries, 0);
}

#[test]
fn test_bulk_insert_edges_skips_duplicates() {
    let graph = graph();
    let from = graph
        .insert_entity(&sqlitegraph::graph::GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "from".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    let to = graph
        .insert_entity(&sqlitegraph::graph::GraphEntity {
            id: 0,
            kind: "Fn".into(),
            name: "to".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    let ids = bulk_insert_edges(
        &graph,
        &[
            GraphEdgeCreate {
                from_id: from,
                to_id: to,
                edge_type: "CALLS".into(),
                data: json!({}),
            },
            GraphEdgeCreate {
                from_id: from,
                to_id: to,
                edge_type: "CALLS".into(),
                data: json!({}),
            },
        ],
    )
    .expect("bulk edges");
    assert_eq!(ids.len(), 1);
    let neighbors = graph.query().neighbors(from).unwrap();
    assert_eq!(neighbors, vec![to]);
}
