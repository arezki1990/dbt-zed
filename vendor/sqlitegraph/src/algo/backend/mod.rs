//! Generic graph algorithms using GraphBackend trait
//!
//! This module provides algorithm implementations that work with any backend
//! implementing the GraphBackend trait (SQLite, Native V2, Native V3).
//!
//! # Design Principles
//!
//! - **Backend-agnostic**: Uses only GraphBackend trait methods
//! - **No SQLite dependency**: Works with V3 binary format
//! - **Zero overhead**: No wrappers or adapters, direct trait calls
//! - **Deterministic**: Same results regardless of backend
//!
//! # Available Algorithms
//!
//! - `pagerank` - PageRank centrality
//! - `betweenness_centrality` - Betweenness centrality (Brandes' algorithm)
//! - `bfs` - Breadth-first search traversal
//! - `shortest_path` - Dijkstra shortest path
//! - `strongly_connected_components` - Tarjan's SCC algorithm
//! - `topological_sort` - Kahn's algorithm
//!
//! # Usage
//!
//! ```rust
//! use sqlitegraph::backend::GraphBackend;
//! use sqlitegraph::algo::backend::pagerank;
//!
//! fn analyze_graph(graph: &dyn GraphBackend) {
//!     let scores = pagerank(graph, 0.85, 20).unwrap();
//!     // ... use scores
//! }
//! ```

pub mod centrality;
pub mod graph_ops;
pub mod iterator;
pub mod traversal;

// Re-export commonly used algorithms
pub use centrality::{betweenness_centrality, pagerank};
pub use graph_ops::{bfs, shortest_path, strongly_connected_components, topological_sort};
pub use iterator::{
    BfsIter, ConnectedComponentsIter, DfsIter, GraphIterator, TopologicalSortIter, bfs_iter,
    connected_components_iter, dfs_iter, topological_sort_iter,
};
pub use traversal::{bfs_traversal, dfs_traversal, k_hop_neighbors};
