use super::*;

fn make_chain() -> TypedDiGraph<&'static str, ()> {
    let mut g = TypedDiGraph::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let c = g.add_node("c");
    let d = g.add_node("d");
    g.add_edge(a, b, ());
    g.add_edge(b, c, ());
    g.add_edge(c, d, ());
    g
}

fn make_cycle() -> TypedDiGraph<&'static str, ()> {
    let mut g = TypedDiGraph::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let c = g.add_node("c");
    g.add_edge(a, b, ());
    g.add_edge(b, c, ());
    g.add_edge(c, a, ());
    g
}

fn make_diamond() -> TypedDiGraph<&'static str, ()> {
    let mut g = TypedDiGraph::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let c = g.add_node("c");
    let d = g.add_node("d");
    g.add_edge(a, b, ());
    g.add_edge(a, c, ());
    g.add_edge(b, d, ());
    g.add_edge(c, d, ());
    g
}

#[test]
fn test_add_node_and_count() {
    let mut g = TypedDiGraph::<i32, ()>::new();
    assert_eq!(g.node_count(), 0);
    let a = g.add_node(1);
    let _b = g.add_node(2);
    assert_eq!(g.node_count(), 2);
    assert_eq!(g.edge_count(), 0);
    assert_eq!(*g.node_weight(a).unwrap(), 1);
    assert_eq!(*g.node_weight(_b).unwrap(), 2);
}

#[test]
fn test_add_edge_and_count() {
    let g = make_chain();
    assert_eq!(g.node_count(), 4);
    assert_eq!(g.edge_count(), 3);
}

#[test]
fn test_neighbors_outgoing() {
    let g = make_diamond();
    let a = NodeIndex(0);
    let neighbors: Vec<NodeIndex> = g.neighbors_directed(a, Direction::Outgoing).collect();
    assert_eq!(neighbors.len(), 2);
    assert!(neighbors.contains(&NodeIndex(1)));
    assert!(neighbors.contains(&NodeIndex(2)));
}

#[test]
fn test_neighbors_incoming() {
    let g = make_diamond();
    let d = NodeIndex(3);
    let neighbors: Vec<NodeIndex> = g.neighbors_directed(d, Direction::Incoming).collect();
    assert_eq!(neighbors.len(), 2);
    assert!(neighbors.contains(&NodeIndex(1)));
    assert!(neighbors.contains(&NodeIndex(2)));
}

#[test]
fn test_degrees() {
    let g = make_diamond();
    assert_eq!(g.out_degree(NodeIndex(0)), 2);
    assert_eq!(g.in_degree(NodeIndex(0)), 0);
    assert_eq!(g.out_degree(NodeIndex(3)), 0);
    assert_eq!(g.in_degree(NodeIndex(3)), 2);
}

#[test]
fn test_edge_weight() {
    let mut g = TypedDiGraph::<&str, i32>::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let e = g.add_edge(a, b, 42);
    assert_eq!(*g.edge_weight(e).unwrap(), 42);
}

#[test]
fn test_edge_endpoints() {
    let mut g = TypedDiGraph::<&str, ()>::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let e = g.add_edge(a, b, ());
    assert_eq!(g.edge_endpoints(e), Some((a, b)));
}

#[test]
fn test_remove_node() {
    let mut g = make_chain();
    let weight = g.remove_node(NodeIndex(1));
    assert_eq!(weight, Some("b"));
    assert_eq!(g.node_count(), 3);
    assert!(!g.contains_node(NodeIndex(1)));
    assert_eq!(g.edge_count(), 1);
}

#[test]
fn test_remove_edge() {
    let mut g = make_chain();
    let weight = g.remove_edge(EdgeIndex(1));
    assert_eq!(weight, Some(()));
    assert_eq!(g.edge_count(), 2);
    let neighbors: Vec<_> = g
        .neighbors_directed(NodeIndex(0), Direction::Outgoing)
        .collect();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], NodeIndex(1));
}

#[test]
fn test_node_reuse_after_remove() {
    let mut g = TypedDiGraph::<i32, ()>::new();
    let a = g.add_node(1);
    let _b = g.add_node(2);
    g.remove_node(a);
    let c = g.add_node(3);
    assert_eq!(g.node_count(), 2);
    assert_eq!(*g.node_weight(c).unwrap(), 3);
}

#[test]
fn test_index_operators() {
    let mut g = TypedDiGraph::<i32, ()>::new();
    let a = g.add_node(10);
    let b = g.add_node(20);
    g.add_edge(a, b, ());
    assert_eq!(g[a], 10);
    g[a] = 30;
    assert_eq!(g[a], 30);
}

#[test]
fn test_is_cyclic_dag() {
    let g = make_chain();
    assert!(!is_cyclic_directed(&g));
    let g = make_diamond();
    assert!(!is_cyclic_directed(&g));
}

#[test]
fn test_is_cyclic_cycle() {
    let g = make_cycle();
    assert!(is_cyclic_directed(&g));
}

#[test]
fn test_is_cyclic_empty() {
    let g: TypedDiGraph<i32, ()> = TypedDiGraph::new();
    assert!(!is_cyclic_directed(&g));
}

#[test]
fn test_is_cyclic_self_loop() {
    let mut g = TypedDiGraph::<&str, ()>::new();
    let a = g.add_node("a");
    g.add_edge(a, a, ());
    assert!(is_cyclic_directed(&g));
}

#[test]
fn test_tarjan_scc_chain() {
    let g = make_chain();
    let sccs = tarjan_scc(&g);
    assert_eq!(sccs.len(), 4);
    for scc in &sccs {
        assert_eq!(scc.len(), 1);
    }
}

#[test]
fn test_tarjan_scc_cycle() {
    let g = make_cycle();
    let sccs = tarjan_scc(&g);
    let non_trivial: Vec<_> = sccs.iter().filter(|c| c.len() > 1).collect();
    assert_eq!(non_trivial.len(), 1);
    assert_eq!(non_trivial[0].len(), 3);
}

#[test]
fn test_tarjan_scc_two_cycles() {
    let mut g = TypedDiGraph::<&str, ()>::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let c = g.add_node("c");
    let d = g.add_node("d");
    let e = g.add_node("e");
    g.add_edge(a, b, ());
    g.add_edge(b, a, ());
    g.add_edge(c, d, ());
    g.add_edge(d, e, ());
    g.add_edge(e, c, ());

    let sccs = tarjan_scc(&g);
    let non_trivial: Vec<_> = sccs.iter().filter(|c| c.len() > 1).collect();
    assert_eq!(non_trivial.len(), 2);
}

#[test]
fn test_tarjan_scc_empty() {
    let g: TypedDiGraph<i32, ()> = TypedDiGraph::new();
    let sccs = tarjan_scc(&g);
    assert!(sccs.is_empty());
}

#[test]
fn test_toposort_dag() {
    let g = make_chain();
    let order = toposort(&g).unwrap();
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], NodeIndex(0));
    assert_eq!(order[3], NodeIndex(3));
}

#[test]
fn test_toposort_diamond() {
    let g = make_diamond();
    let order = toposort(&g).unwrap();
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], NodeIndex(0));
    assert_eq!(order[3], NodeIndex(3));
}

#[test]
fn test_toposort_cycle() {
    let g = make_cycle();
    let result = toposort(&g);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(!err.cycle.is_empty());
}

#[test]
fn test_toposort_empty() {
    let g: TypedDiGraph<i32, ()> = TypedDiGraph::new();
    let order = toposort(&g).unwrap();
    assert!(order.is_empty());
}

#[test]
fn test_dfs_traversal() {
    let g = make_chain();
    let mut dfs = Dfs::new(&g, NodeIndex(0));
    let visited: Vec<NodeIndex> = dfs.by_ref().collect();
    assert_eq!(visited.len(), 4);
    assert_eq!(visited[0], NodeIndex(0));
}

#[test]
fn test_dfs_diamond_visits_all() {
    let g = make_diamond();
    let mut dfs = Dfs::new(&g, NodeIndex(0));
    let visited: Vec<NodeIndex> = dfs.by_ref().collect();
    assert_eq!(visited.len(), 4);
}

#[test]
fn test_dfs_cycle_terminates() {
    let g = make_cycle();
    let mut dfs = Dfs::new(&g, NodeIndex(0));
    let visited: Vec<NodeIndex> = dfs.by_ref().collect();
    assert_eq!(visited.len(), 3);
}

#[test]
fn test_dfs_visited_flag() {
    let g = make_chain();
    let mut dfs = Dfs::new(&g, NodeIndex(0));
    dfs.move_next();
    assert!(dfs.visited(NodeIndex(0)));
    assert!(!dfs.visited(NodeIndex(3)));
}

#[test]
fn test_node_indices_iterator() {
    let mut g = TypedDiGraph::<i32, ()>::new();
    g.add_node(1);
    g.add_node(2);
    g.add_node(3);
    g.remove_node(NodeIndex(1));
    let indices: Vec<NodeIndex> = g.node_indices().collect();
    assert_eq!(indices.len(), 2);
    assert!(indices.contains(&NodeIndex(0)));
    assert!(indices.contains(&NodeIndex(2)));
}

#[test]
fn test_with_capacity() {
    let g: TypedDiGraph<i32, i32> = TypedDiGraph::with_capacity(10, 20);
    assert_eq!(g.node_count(), 0);
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn test_edges_directed() {
    let mut g = TypedDiGraph::<&str, i32>::new();
    let a = g.add_node("a");
    let b = g.add_node("b");
    let e = g.add_edge(a, b, 42);
    let edges: Vec<_> = g.edges_directed(a, Direction::Outgoing).collect();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].0, e);
    assert_eq!(*edges[0].1, 42);
    assert_eq!(edges[0].2, b);
}

#[test]
fn test_node_weight_mut() {
    let mut g = TypedDiGraph::<i32, ()>::new();
    let a = g.add_node(10);
    *g.node_weight_mut(a).unwrap() = 20;
    assert_eq!(*g.node_weight(a).unwrap(), 20);
}
