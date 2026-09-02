use serde_json::json;
use sqlitegraph::backend::BackendDirection;
use sqlitegraph::pattern::{self, NodeConstraint, PatternLeg, PatternQuery};
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph};

fn insert_node(graph: &SqliteGraph, kind: &str, name: &str) -> i64 {
    graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: kind.into(),
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

fn build_graph() -> (SqliteGraph, Vec<i64>) {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let f1 = insert_node(&graph, "Function", "A_func");
    let f2 = insert_node(&graph, "Function", "B_func");
    let f3 = insert_node(&graph, "Function", "C_func");
    let s1 = insert_node(&graph, "Struct", "S_alpha");
    let s2 = insert_node(&graph, "Struct", "S_beta");

    insert_edge(&graph, f1, f2, "CALLS");
    insert_edge(&graph, f2, s1, "USES");
    insert_edge(&graph, f1, f3, "CALLS");
    insert_edge(&graph, f3, s2, "USES");
    insert_edge(&graph, f2, f3, "CALLS");

    (graph, vec![f1, f2, f3, s1, s2])
}

#[test]
fn test_pattern_query_matches_kind_chain() {
    let (graph, ids) = build_graph();
    let pattern = PatternQuery {
        root: Some(NodeConstraint::kind("Function")),
        legs: vec![
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".into()),
                constraint: Some(NodeConstraint::kind("Function")),
            },
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".into()),
                constraint: Some(NodeConstraint::kind("Struct")),
            },
        ],
    };
    let matches = graph
        .query()
        .pattern_matches(ids[0], &pattern)
        .expect("pattern");
    let sequences: Vec<Vec<i64>> = matches.into_iter().map(|m| m.nodes).collect();
    assert_eq!(
        sequences,
        vec![vec![ids[0], ids[1], ids[3]], vec![ids[0], ids[2], ids[4]]]
    );
}

#[test]
fn test_pattern_query_blocks_root_constraint() {
    let (graph, ids) = build_graph();
    let pattern = PatternQuery {
        root: Some(NodeConstraint::kind("Struct")),
        legs: vec![PatternLeg {
            direction: BackendDirection::Outgoing,
            edge_type: Some("CALLS".into()),
            constraint: Some(NodeConstraint::kind("Function")),
        }],
    };
    let matches = graph
        .query()
        .pattern_matches(ids[0], &pattern)
        .expect("pattern");
    assert!(matches.is_empty());
}

#[test]
fn entity_ids_with_constraint_filters_by_kind_and_prefix() {
    let graph = SqliteGraph::open_in_memory().expect("graph");
    let alpha_fn = insert_node(&graph, "Fn", "alpha_fn");
    let beta_fn = insert_node(&graph, "Fn", "beta_fn");
    let _module = insert_node(&graph, "Module", "alpha_mod");

    let mut constraint = NodeConstraint::kind("Fn");
    let ids = pattern::entity_ids_with_constraint(&graph, &constraint).expect("ids");
    assert_eq!(ids, vec![alpha_fn, beta_fn]);

    constraint.name_prefix = Some("alpha".into());
    let ids = pattern::entity_ids_with_constraint(&graph, &constraint).expect("prefixed");
    assert_eq!(ids, vec![alpha_fn]);
}

#[test]
fn test_pattern_query_with_name_prefix_filter() {
    let (graph, ids) = build_graph();
    let pattern = PatternQuery {
        root: Some(NodeConstraint::name_prefix("A_")),
        legs: vec![
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".into()),
                constraint: Some(NodeConstraint::name_prefix("B_")),
            },
            PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some("USES".into()),
                constraint: Some(NodeConstraint::name_prefix("S_a")),
            },
        ],
    };
    let matches = graph
        .query()
        .pattern_matches(ids[0], &pattern)
        .expect("pattern");
    let sequences: Vec<Vec<i64>> = matches.into_iter().map(|m| m.nodes).collect();
    assert_eq!(sequences, vec![vec![ids[0], ids[1], ids[3]]]);
}
