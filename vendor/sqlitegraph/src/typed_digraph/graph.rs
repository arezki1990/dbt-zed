use std::ops::{Index, IndexMut};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeIndex(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeIndex(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

impl NodeIndex {
    pub fn new(i: usize) -> Self {
        NodeIndex(i)
    }
}

impl EdgeIndex {
    pub fn new(i: usize) -> Self {
        EdgeIndex(i)
    }
}

impl From<usize> for NodeIndex {
    fn from(i: usize) -> Self {
        NodeIndex(i)
    }
}

impl From<usize> for EdgeIndex {
    fn from(i: usize) -> Self {
        EdgeIndex(i)
    }
}

#[derive(Clone)]
struct EdgeEntry<E> {
    from: usize,
    to: usize,
    weight: Option<E>,
}

#[derive(Clone)]
pub struct TypedDiGraph<N, E> {
    nodes: Vec<Option<N>>,
    edges: Vec<Option<EdgeEntry<E>>>,
    adj_out: Vec<Vec<(usize, usize)>>,
    adj_in: Vec<Vec<(usize, usize)>>,
    free_node_slots: Vec<usize>,
    free_edge_slots: Vec<usize>,
    node_count: usize,
    edge_count: usize,
}

impl<N, E> TypedDiGraph<N, E> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            adj_out: Vec::new(),
            adj_in: Vec::new(),
            free_node_slots: Vec::new(),
            free_edge_slots: Vec::new(),
            node_count: 0,
            edge_count: 0,
        }
    }

    pub fn with_capacity(nodes: usize, edges: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(nodes),
            edges: Vec::with_capacity(edges),
            adj_out: Vec::with_capacity(nodes),
            adj_in: Vec::with_capacity(nodes),
            free_node_slots: Vec::new(),
            free_edge_slots: Vec::new(),
            node_count: 0,
            edge_count: 0,
        }
    }

    pub fn add_node(&mut self, weight: N) -> NodeIndex {
        self.node_count += 1;
        if let Some(slot) = self.free_node_slots.pop() {
            self.nodes[slot] = Some(weight);
            NodeIndex(slot)
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Some(weight));
            self.adj_out.push(Vec::new());
            self.adj_in.push(Vec::new());
            NodeIndex(idx)
        }
    }

    pub fn add_edge(&mut self, from: NodeIndex, to: NodeIndex, weight: E) -> EdgeIndex {
        assert!(
            from.0 < self.nodes.len() && self.nodes[from.0].is_some(),
            "NodeIndex({}) is not a valid node",
            from.0
        );
        assert!(
            to.0 < self.nodes.len() && self.nodes[to.0].is_some(),
            "NodeIndex({}) is not a valid node",
            to.0
        );
        self.edge_count += 1;
        let entry = EdgeEntry {
            from: from.0,
            to: to.0,
            weight: Some(weight),
        };
        if let Some(slot) = self.free_edge_slots.pop() {
            self.edges[slot] = Some(entry);
            self.adj_out[from.0].push((slot, to.0));
            self.adj_in[to.0].push((slot, from.0));
            EdgeIndex(slot)
        } else {
            let idx = self.edges.len();
            self.edges.push(Some(entry));
            self.adj_out[from.0].push((idx, to.0));
            self.adj_in[to.0].push((idx, from.0));
            EdgeIndex(idx)
        }
    }

    pub fn remove_node(&mut self, idx: NodeIndex) -> Option<N> {
        if idx.0 >= self.nodes.len() {
            return None;
        }
        let weight = self.nodes[idx.0].take()?;
        self.node_count -= 1;

        let out_edges: Vec<usize> = self.adj_out[idx.0].iter().map(|(e, _)| *e).collect();
        for edge_idx in &out_edges {
            self.remove_edge_raw(*edge_idx);
        }
        self.adj_out[idx.0].clear();

        let in_edges: Vec<usize> = self.adj_in[idx.0].iter().map(|(e, _)| *e).collect();
        for edge_idx in &in_edges {
            self.remove_edge_raw(*edge_idx);
        }
        self.adj_in[idx.0].clear();

        self.free_node_slots.push(idx.0);
        Some(weight)
    }

    pub fn remove_edge(&mut self, idx: EdgeIndex) -> Option<E> {
        let entry = self.edges[idx.0].take()?;
        self.remove_edge_from_adj(idx.0, entry.from, entry.to);
        self.edge_count -= 1;
        self.free_edge_slots.push(idx.0);
        Some(entry.weight.expect("invariant: edge weight present"))
    }

    fn remove_edge_raw(&mut self, edge_idx: usize) {
        if let Some(entry) = self.edges[edge_idx].take() {
            self.remove_edge_from_adj(edge_idx, entry.from, entry.to);
            self.edge_count -= 1;
            self.free_edge_slots.push(edge_idx);
        }
    }

    fn remove_edge_from_adj(&mut self, edge_idx: usize, from: usize, to: usize) {
        self.adj_out[from].retain(|(e, _)| *e != edge_idx);
        self.adj_in[to].retain(|(e, _)| *e != edge_idx);
    }

    pub fn node_weight(&self, idx: NodeIndex) -> Option<&N> {
        self.nodes.get(idx.0)?.as_ref()
    }

    pub fn node_weight_mut(&mut self, idx: NodeIndex) -> Option<&mut N> {
        self.nodes.get_mut(idx.0)?.as_mut()
    }

    pub fn edge_weight(&self, idx: EdgeIndex) -> Option<&E> {
        self.edges.get(idx.0)?.as_ref()?.weight.as_ref()
    }

    pub fn edge_endpoints(&self, idx: EdgeIndex) -> Option<(NodeIndex, NodeIndex)> {
        let entry = self.edges.get(idx.0)?.as_ref()?;
        Some((NodeIndex(entry.from), NodeIndex(entry.to)))
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub fn contains_node(&self, idx: NodeIndex) -> bool {
        idx.0 < self.nodes.len() && self.nodes[idx.0].is_some()
    }

    pub fn neighbors_directed(
        &self,
        idx: NodeIndex,
        dir: Direction,
    ) -> impl Iterator<Item = NodeIndex> + '_ {
        let adj = match dir {
            Direction::Outgoing => &self.adj_out,
            Direction::Incoming => &self.adj_in,
        };
        adj.get(idx.0)
            .map(|list| list.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|(_, target)| NodeIndex(*target))
    }

    pub fn edges_directed(
        &self,
        idx: NodeIndex,
        dir: Direction,
    ) -> impl Iterator<Item = (EdgeIndex, &E, NodeIndex)> + '_ {
        let adj = match dir {
            Direction::Outgoing => &self.adj_out,
            Direction::Incoming => &self.adj_in,
        };
        adj.get(idx.0)
            .map(|list| list.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|(edge_idx, target)| {
                let entry = self.edges.get(*edge_idx)?.as_ref()?;
                let weight = entry.weight.as_ref()?;
                Some((EdgeIndex(*edge_idx), weight, NodeIndex(*target)))
            })
    }

    pub fn out_degree(&self, idx: NodeIndex) -> usize {
        self.adj_out.get(idx.0).map(|list| list.len()).unwrap_or(0)
    }

    pub fn in_degree(&self, idx: NodeIndex) -> usize {
        self.adj_in.get(idx.0).map(|list| list.len()).unwrap_or(0)
    }

    pub fn node_indices(&self) -> impl Iterator<Item = NodeIndex> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_some())
            .map(|(i, _)| NodeIndex(i))
    }

    pub fn edge_indices(&self) -> impl Iterator<Item = EdgeIndex> + '_ {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_some())
            .map(|(i, _)| EdgeIndex(i))
    }

    pub fn raw_node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn raw_edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn find_edge(&self, from: NodeIndex, to: NodeIndex) -> Option<EdgeIndex> {
        self.adj_out
            .get(from.0)?
            .iter()
            .find(|(_, target)| *target == to.0)
            .map(|(edge_idx, _)| EdgeIndex(*edge_idx))
    }

    pub fn iter_edges(&self) -> impl Iterator<Item = EdgeRef<&E>> + '_ {
        self.edges.iter().filter_map(|entry| {
            let e = entry.as_ref()?;
            Some(EdgeRef {
                source: NodeIndex(e.from),
                target: NodeIndex(e.to),
                weight: e.weight.as_ref()?,
            })
        })
    }
}

pub struct EdgeRef<W> {
    pub source: NodeIndex,
    pub target: NodeIndex,
    pub weight: W,
}

impl<N, E> Default for TypedDiGraph<N, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E> Index<NodeIndex> for TypedDiGraph<N, E> {
    type Output = N;
    fn index(&self, index: NodeIndex) -> &N {
        self.node_weight(index).expect("NodeIndex out of bounds")
    }
}

impl<N, E> IndexMut<NodeIndex> for TypedDiGraph<N, E> {
    fn index_mut(&mut self, index: NodeIndex) -> &mut N {
        self.node_weight_mut(index)
            .expect("NodeIndex out of bounds")
    }
}

impl<N, E> Index<EdgeIndex> for TypedDiGraph<N, E> {
    type Output = E;
    fn index(&self, index: EdgeIndex) -> &E {
        self.edge_weight(index).expect("EdgeIndex out of bounds")
    }
}
