//! SQLite-backed graph database implementation.
//!
//! This module provides the core graph database implementation with SQLite storage,
//! entity and edge management, pattern matching, and MVCC-lite snapshot isolation.
//!
//! # Module Architecture
//!
//! The graph module is organized into focused submodules:
//!
//! - **core**: [`SqliteGraph`] - Main graph database implementation
//! - **types**: [`GraphEntity`], [`GraphEdge`] - Core graph data structures
//! - **adjacency**: Adjacency list management with caching
//! - **entity_ops**: Node/entity CRUD operations
//! - **edge_ops**: Edge/relationship CRUD operations
//! - **pattern_matching**: Triple pattern matching engine
//! - **snapshot**: MVCC-lite snapshot system for read isolation
//! - **metrics**: Performance monitoring and instrumentation
//!
//! # Core Types
//!
//! - [`SqliteGraph`] - Main graph database struct with dual backend support
//! - [`GraphEntity`] - Graph node/vertex representation with labels and properties
//! - [`GraphEdge`] - Graph edge/relationship representation with metadata
//!
//! # Invariants and Guarantees
//
//! ## Thread Safety
//!
//! **NOT thread-safe for concurrent writes.** `SqliteGraph` uses interior mutability
//! (`RefCell`) and is not `Sync`. Do not share across threads for write operations.
//!
//! For concurrent read access, use [`GraphSnapshot`] from the `mvcc` module:
//!
//! ```rust,ignore
//! use sqlitegraph::{GraphSnapshot, SqliteGraph};
//!
//! let graph = SqliteGraph::open("my_graph.db")?;
//!
//! // Create snapshots for concurrent reads
//! let snapshot1 = graph.snapshot()?;
//! let snapshot2 = graph.snapshot()?;
//!
//! // Both snapshots can be used concurrently (thread-safe)
//! std::thread::spawn(move || {
//!     let neighbors = snapshot1.neighbors(node_id)?;
//! });
//! ```
//!
//! ## Transaction Isolation
//!
//! This graph provides **MVCC-lite** snapshot isolation:
//!
//! - **Readers** see consistent snapshot at creation time
//! - **Writers** are serialized (one write transaction at a time)
//! - **No phantom reads** - snapshots are immutable after creation
//! - **Write skew** possible - snapshots don't prevent concurrent write conflicts
//!
//! MVCC-lite guarantees:
//! - Readers never block writers
//! - Writers never block readers
//! - Each snapshot sees a consistent view
//! - No dirty reads within a snapshot
//!
//! ## Edge Case Behavior
//!
//! ### Empty Graph
//! - Queries on empty graph return empty results (not errors)
//! - `all_entity_ids()` returns empty `Vec`
//! - Pattern matching returns empty iterator
//!
//! ### Deleted Nodes
//! - Deleted nodes are removed from all adjacency lists
//! - Edges to/from deleted nodes are automatically removed
//! - Subsequent queries return `SqliteGraphError::EntityNotFound`
//!
//! ### Self-Loops
//! - Self-loops (edges where source == target) are supported
//! - Stored in both incoming and outgoing adjacency lists
//! - Returned by both `fetch_outgoing` and `fetch_incoming`
//!
//! # Performance Characteristics
//!
//! ## Insert Operations
//! - **Node insert**: O(1) amortized (single SQLite INSERT)
//! - **Edge insert**: O(1) amortized (two SQLite INSERTs + cache update)
//! - **Bulk insert**: O(N) for batch operations (transaction overhead amortized)
//!
//! ## Query Operations
//! - **Neighbor lookup**: O(degree) for cached adjacency, O(log V) for database query
//! - **Pattern match**: O(matched triples) with cache acceleration
//! - **BFS traversal**: O(V + E) worst case, typically O(branching_factor × depth)
//! - **Shortest path**: O(V + E) for unweighted BFS-based algorithm
//!
//! ## Memory Usage
//! - **Base graph**: O(V + E) for adjacency lists
//! - **Cache overhead**: O(cached_nodes × avg_degree) for LRU cache
//! - **Snapshot**: O(1) per snapshot (shares graph data, isolated transaction)
//!
//! # Usage Examples
//!
//! ## Basic Graph Operations
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, GraphEntityCreate};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//!
//! // Insert nodes
//! let node1 = graph.insert_node(GraphEntityCreate {
//!     labels: vec!["Person".into()],
//!     properties: vec![],
//! })?;
//!
//! let node2 = graph.insert_node(GraphEntityCreate {
//!     labels: vec!["Person".into()],
//!     properties: vec![],
//! })?;
//!
//! // Insert edge
//! graph.insert_edge(node1, "KNOWS".into(), node2, vec![])?;
//!
//! // Query neighbors
//! let neighbors = graph.fetch_outgoing(node1)?;
//! assert_eq!(neighbors, vec![node2]);
//! ```
//!
//! ## Pattern Matching
//!
//! ```rust,ignore
//! use sqlitegraph::{PatternTriple, match_triples};
//!
//! let pattern = PatternTriple {
//!     subject: Some(node1),
//!     predicate: Some("KNOWS".into()),
//!     object: None, // Wildcard
//! };
//!
//! let matches = match_triples(&graph, pattern)?;
//! for triple in matches {
//!     println!("{:?} KNOWS {:?}", triple.subject, triple.object);
//! }
//! ```
//!
//! # Backend Selection
//!
//! This module provides the SQLite backend implementation. For the native backend
//! (optimized for performance without SQLite dependencies), see `backend::NativeGraphBackend`.
//!
//! Both backends implement the `GraphBackend` trait, providing a unified API for
//! graph operations regardless of storage engine.

pub use self::core::{SqliteGraph, is_in_memory_connection};

mod adjacency;
mod core;
mod edge_ops;
mod entity_ops;
mod metrics;
mod metrics_schema;
mod pattern_matching;
pub mod pool;
mod snapshot;
mod types;

pub use adjacency::{ConnectionWrapper, StatementWrapper};
pub use metrics::{GraphMetricsSnapshot, InstrumentedConnection};
pub use types::{GraphEdge, GraphEntity, GraphStats, SnapshotMetadata};
