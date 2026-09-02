use super::graph::{Direction, NodeIndex, TypedDiGraph};

#[derive(Debug, Clone)]
pub struct CycleError {
    pub cycle: Vec<NodeIndex>,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "graph contains a cycle: {:?}",
            self.cycle.iter().map(|n| n.0).collect::<Vec<_>>()
        )
    }
}

impl std::error::Error for CycleError {}

pub fn is_cyclic_directed<N, E>(graph: &TypedDiGraph<N, E>) -> bool {
    let n = graph.raw_node_count();
    if n == 0 {
        return false;
    }

    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;

    let mut color = vec![WHITE; n];
    let mut stack: Vec<(NodeIndex, bool)> = Vec::new();

    for start in graph.node_indices() {
        if color[start.0] != WHITE {
            continue;
        }
        stack.push((start, false));
        while let Some((node, processed)) = stack.pop() {
            if processed {
                color[node.0] = BLACK;
                continue;
            }
            if color[node.0] == GRAY {
                continue;
            }
            if color[node.0] == BLACK {
                continue;
            }
            color[node.0] = GRAY;
            stack.push((node, true));
            for neighbor in graph.neighbors_directed(node, Direction::Outgoing) {
                match color[neighbor.0] {
                    WHITE => stack.push((neighbor, false)),
                    GRAY => return true,
                    _ => {}
                }
            }
        }
    }

    false
}

pub fn tarjan_scc<N, E>(graph: &TypedDiGraph<N, E>) -> Vec<Vec<NodeIndex>> {
    let n = graph.raw_node_count();
    if n == 0 {
        return Vec::new();
    }

    let mut index_counter: usize = 0;
    let mut node_index: Vec<Option<usize>> = vec![None; n];
    let mut lowlink: Vec<usize> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut stack: Vec<NodeIndex> = Vec::new();
    let mut components: Vec<Vec<NodeIndex>> = Vec::new();

    for start in graph.node_indices() {
        if node_index[start.0].is_some() {
            continue;
        }

        let mut work: Vec<(NodeIndex, usize)> = vec![(start, 0)];

        while let Some((v, current_state)) = work.last().copied() {
            let work_idx = work.len() - 1;

            if current_state == 0 {
                node_index[v.0] = Some(index_counter);
                lowlink[v.0] = index_counter;
                index_counter += 1;
                stack.push(v);
                on_stack[v.0] = true;
                work[work_idx].1 = 1;
            }

            let neighbors: Vec<NodeIndex> =
                graph.neighbors_directed(v, Direction::Outgoing).collect();

            let mut advanced = false;
            while let Some(i) = work[work_idx].1.checked_sub(1) {
                if i >= neighbors.len() {
                    break;
                }
                let w = neighbors[i];
                work[work_idx].1 = i + 2;

                if node_index[w.0].is_none() {
                    work.push((w, 0));
                    advanced = true;
                    break;
                } else if on_stack[w.0]
                    && let Some(w_index) = node_index[w.0]
                {
                    lowlink[v.0] = lowlink[v.0].min(w_index);
                }
            }

            if advanced {
                continue;
            }

            if let Some(v_index) = node_index[v.0]
                && lowlink[v.0] == v_index
            {
                let mut component = Vec::new();
                loop {
                    let w = stack.pop().expect("invariant: stack non-empty during SCC");
                    on_stack[w.0] = false;
                    component.push(w);
                    if w.0 == v.0 {
                        break;
                    }
                }
                components.push(component);
            }

            work.pop();
            if let Some((parent, _)) = work.last_mut() {
                lowlink[parent.0] = lowlink[parent.0].min(lowlink[v.0]);
            }
        }
    }

    components
}

pub fn toposort<N, E>(graph: &TypedDiGraph<N, E>) -> Result<Vec<NodeIndex>, CycleError> {
    let mut in_degree: Vec<usize> = vec![0; graph.raw_node_count()];
    let all_nodes: Vec<NodeIndex> = graph.node_indices().collect();

    for node in &all_nodes {
        for neighbor in graph.neighbors_directed(*node, Direction::Outgoing) {
            in_degree[neighbor.0] += 1;
        }
    }

    let mut queue: std::collections::VecDeque<NodeIndex> = all_nodes
        .iter()
        .filter(|&&node| in_degree[node.0] == 0)
        .copied()
        .collect();

    let mut result = Vec::with_capacity(all_nodes.len());

    while let Some(node) = queue.pop_front() {
        result.push(node);
        for neighbor in graph.neighbors_directed(node, Direction::Outgoing) {
            in_degree[neighbor.0] -= 1;
            if in_degree[neighbor.0] == 0 {
                queue.push_back(neighbor);
            }
        }
    }

    if result.len() != all_nodes.len() {
        let remaining: Vec<NodeIndex> = all_nodes
            .into_iter()
            .filter(|n| in_degree[n.0] > 0)
            .collect();
        return Err(CycleError { cycle: remaining });
    }

    Ok(result)
}

pub struct Dfs<'a, N, E> {
    graph: &'a TypedDiGraph<N, E>,
    stack: Vec<NodeIndex>,
    visited: Vec<bool>,
}

impl<'a, N, E> Dfs<'a, N, E> {
    pub fn new(graph: &'a TypedDiGraph<N, E>, start: NodeIndex) -> Self {
        let mut dfs = Self {
            graph,
            stack: Vec::new(),
            visited: vec![false; graph.raw_node_count()],
        };
        if graph.contains_node(start) {
            dfs.stack.push(start);
        }
        dfs
    }

    pub fn move_next(&mut self) -> Option<NodeIndex> {
        while let Some(node) = self.stack.pop() {
            if self.visited[node.0] {
                continue;
            }
            self.visited[node.0] = true;
            for neighbor in self.graph.neighbors_directed(node, Direction::Outgoing) {
                if !self.visited[neighbor.0] {
                    self.stack.push(neighbor);
                }
            }
            return Some(node);
        }
        None
    }

    pub fn visited(&self, node: NodeIndex) -> bool {
        self.visited.get(node.0).copied().unwrap_or(false)
    }

    pub fn reset(&mut self, start: NodeIndex) {
        self.visited.clear();
        self.visited.resize(self.graph.raw_node_count(), false);
        self.stack.clear();
        if self.graph.contains_node(start) {
            self.stack.push(start);
        }
    }
}

impl<'a, N, E> Iterator for Dfs<'a, N, E> {
    type Item = NodeIndex;

    fn next(&mut self) -> Option<Self::Item> {
        Dfs::move_next(self)
    }
}
