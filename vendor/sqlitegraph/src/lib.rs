//! SQLite-based graph database with unified backend support.
// Module-level dead_code suppression removed as part of CODE-02
// Individual items may still need specific allows where truly unused
//!
//! `sqlitegraph` provides a lightweight, deterministic graph database for embedded Rust applications.
//! It supports explicit SQLite, Native V3, and Combined backend modes through a unified API.
//!
//! # Architecture
//!
//! The crate is organized into focused modules:
//!
//! ```text
//! sqlitegraph/
//! ├── graph/           # Core graph database (SqliteGraph, GraphEntity, GraphEdge)
//! ├── backend/         # Unified backend trait (GraphBackend, SqliteGraphBackend, NativeGraphBackend)
//! ├── algo/            # Graph algorithms (PageRank, Betweenness, Louvain, Label Propagation)
//! ├── hnsw/            # Vector similarity search (HNSW index, distance metrics)
//! ├── cache/           # LRU-K adjacency cache for traversal optimization
//! ├── introspection/   # Debugging and observability APIs
//! ├── progress/        # Progress tracking for long-running operations
//! ├── mvcc/            # MVCC-lite snapshot system
//! ├── pattern_engine/  # Triple pattern matching
//! ├── query/           # High-level query interface
//! └── recovery/        # Backup and restore utilities
//! ```
//!
//! # Features
//!
//! - **Explicit Backend Modes**: Choose SQLite, Native V3, or Combined mode through one config surface
//! - **Entity and Edge Storage**: Rich metadata support with JSON serialization
//! - **Pattern Matching**: Efficient triple pattern matching with cache-enabled fast-path
//! - **Traversal Algorithms**: Built-in BFS, k-hop, and shortest path algorithms
//! - **Graph Algorithms**: PageRank, Betweenness Centrality, Louvain, Label Propagation
//! - **Vector Search**: HNSW approximate nearest neighbor search with persistence
//! - **MVCC Snapshots**: Read isolation with snapshot consistency
//! - **Bulk Operations**: High-performance batch insertions for large datasets
//! - **Introspection**: Debugging APIs for cache stats, file sizes, edge counts
//! - **Progress Tracking**: Callback-based progress for long-running algorithms
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use sqlitegraph::{open_graph, GraphConfig, BackendKind};
//!
//! // Use SQLite backend (default)
//! let cfg = GraphConfig::sqlite();
//! let graph = open_graph("my_graph.db", &cfg)?;
//!
//! // Or use Native backend (V3 experimental backend)
//! let cfg = GraphConfig::native();
//! let graph = open_graph("my_graph.db", &cfg)?;
//!
//! // Or use Combined mode (SQLite-authoritative contract; CombinedGraphBackend today)
//! let cfg = GraphConfig::combined();
//! let graph = open_graph("my_graph.db", &cfg)?;
//!
//! // Both backends support the same operations
//! let node_id = graph.insert_node(/* node spec */)?;
//! let neighbor_ids = graph.neighbors(node_id, /* query */)?;
//! ```
//!
//! # Backend Selection
//!
//! ## Feature Matrix
//!
//! | Feature | SQLite Backend | Native Backend | Combined Backend |
//! |---------|----------------|----------------|------------------|
//! | **Status** | Stable | Experimental | Phase 2 authority seam |
//! | **Authority** | SQLite | Native V3 | SQLite |
//! | **ACID Transactions** | ✅ Full | ✅ WAL-based | Delegates to SQLite today |
//! | **Graph Algorithms** | ✅ Full support | ✅ Full support | Delegates to SQLite today |
//! | **HNSW Vector Search** | ✅ With persistence | ✅ With persistence (V3 KV) | Delegates to SQLite today |
//! | **Turbovec Acceleration** | ❌ | ✅ Feature-gated, large-vector routing | ❌ |
//! | **MVCC Snapshots** | ✅ | ✅ | Delegates to SQLite today |
//! | **Pattern Matching** | ✅ | ✅ | Delegates to SQLite today |
//! | **Raw SQL Access** | ✅ Native | ❌ Not supported | ✅ Native |
//! | **File Format** | SQLite DB | Custom binary (V3) | SQLite DB |
//! | **Default Traversal Runtime** | SQLite table/index path | V3 edge-store / B+Tree path | SQLite table/index path today via `CombinedGraphBackend` |
//! | **Optional materialized live reads** | N/A | CSR runtime views | Untyped `neighbors()` / `bfs()` / `k_hop()` / `node_degree()` / `shortest_path()` via opt-in CSR fallback; specialist mode, not default |
//! | **Atomic SQLite + graph materialization** | N/A | N/A | Not yet implemented |
//! | **Materialized freshness gate** | N/A | Backend-local runtime versioning | `materialized_version >= authoritative_version` required |
//! | **Incremental materialized maintenance** | N/A | N/A | Combined mode refreshes affected CSR rows on edge insert/delete when `PreferMaterialized` is enabled |
//!
//! ## When to Use SQLite Backend
//!
//! Choose SQLite backend when:
//! - **ACID guarantees** are critical for your application
//! - **Raw SQL access** needed for complex queries or joins
//! - **Database compatibility** with SQLite tools (sqlite3, DB Browser)
//! - **Mature ecosystem** with third-party tooling
//! - **HNSW persistence** required
//!
//! ## When to Use Native Backend
//!
//! Choose Native backend when:
//! - **Performance is critical** (faster reads/writes)
//! - **No external dependencies** desired (pure Rust)
//! - **Fast startup** with large datasets
//! - **Custom binary format (V3)** acceptable
//! - **You can tolerate experimental backend status** while native-v3 traversal
//!   uses CSR runtime views layered over the edge-store/B+Tree source of truth,
//!   with CSR/sharding still treated as experimental infrastructure
//!
//! ## When to Use Combined Backend
//!
//! Choose Combined mode when:
//! - **You want an explicit SQLite-authoritative contract** today
//! - **You intend to adopt future SQLite + graph materialization work** without
//!   changing the public mode selection later
//! - **You want an explicit combined-mode type today**, where
//!   `CombinedGraphBackend` preserves a separate public seam without yet
//!   providing a distinct atomic graph-materialization boundary
//! - **You may want to opt into materialized live traversal reads** later via
//!   `CombinedConfig`, while keeping SQLite as the fallback authority
//! - **You accept the current tradeoff**: modest cold-read gains in exchange
//!   for materially slower writes when `PreferMaterialized` is enabled, with
//!   no current end-to-end mixed-workload win
//! - **You want stale materialized views rejected automatically**, not merely
//!   bypassed when rows are missing
//! - **You want an explicit rebuild/publish step** via
//!   `CombinedGraphBackend::publish_materialized_views()`
//! - **You want incremental upkeep for affected live traversal rows** without
//!   turning graph materialization into a second source of truth
//!
//! # Thread Safety
//!
//! ## SqliteGraph is NOT Thread-Safe
//!
//! `SqliteGraph` uses interior mutability (`RefCell`) and is **not `Sync`**:
//!
//! ```rust,ignore
//! use sqlitegraph::SqliteGraph;
//! use std::thread;
//!
//! let graph = SqliteGraph::open("test.db")?;
//!
//! // ❌ WRONG: Sharing graph across threads for writes
//! let graph_clone = graph;
//! thread::spawn(move || {
//!     graph_clone.insert_node(...)?; // DATA RACE!
//! });
//!
//! // ✅ CORRECT: Use snapshots for concurrent reads
//! let snapshot = graph.snapshot()?;
//! thread::spawn(move || {
//!     let neighbors = snapshot.neighbors(node_id)?; // Thread-safe
//! });
//! ```
//!
//! ## Concurrent Read Access
//!
//! Use [`GraphSnapshot`] for thread-safe concurrent reads:
//!
//! ```rust,ignore
//! use sqlitegraph::{GraphSnapshot, SqliteGraph};
//!
//! let graph = SqliteGraph::open("my_graph.db")?;
//!
//! // Create multiple snapshots for concurrent reads
//! let snapshot1 = graph.snapshot()?;
//! let snapshot2 = graph.snapshot()?;
//!
//! // Both snapshots can be used concurrently (thread-safe)
//! let handle1 = std::thread::spawn(move || {
//!     snapshot1.neighbors(node_id)
//! });
//!
//! let handle2 = std::thread::spawn(move || {
//!     snapshot2.neighbors(node_id)
//! });
//! ```
//!
//! ## Write Serialization
//!
//! All writes must be serialized:
//!
//! ```rust,ignore
//! // ✅ CORRECT: Single thread for all writes
//! let graph = SqliteGraph::open("my_graph.db")?;
//! for i in 0..1000 {
//!     graph.insert_node(...)?;
//!     graph.insert_edge(...)?;
//! }
//!
//! // ❌ WRONG: Concurrent writes
//! let graph = Arc::new(Mutex::new(graph));
//! let handle1 = thread::spawn(|| {
//!     let g = graph.lock().unwrap();
//!     g.insert_node(...)
//! });
//! let handle2 = thread::spawn(|| {
//!     let g = graph.lock().unwrap();
//!     g.insert_node(...)
//! });
//! // Even with Mutex, this can cause issues due to RefCell
//! ```
//!
//! # Error Handling
//!
//! All operations return [`Result<T, SqliteGraphError>`]:
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, SqliteGraphError};
//!
//! let graph = SqliteGraph::open("my_graph.db")?;
//!
//! match graph.insert_node(node_spec) {
//!     Ok(node_id) => println!("Created node {}", node_id),
//!     Err(SqliteGraphError::EntityNotFound) => {
//!         println!("Node not found");
//!     }
//!     Err(SqliteGraphError::DatabaseError(e)) => {
//!         eprintln!("Database error: {}", e);
//!     }
//!     Err(e) => {
//!         eprintln!("Other error: {}", e);
//!     }
//! }
//! ```
//!
//! # Performance Comparison
//!
//! ## Read Performance
//! - **SQLite Backend**: 10-100μs per neighbor lookup (cached: ~100ns)
//! - **Native Backend**: 1-10μs per neighbor lookup (cached: ~100ns)
//! - **Cache hit ratio**: 80-95% for traversal workloads
//!
//! ## Write Performance
//! - **SQLite Backend**: 100-500μs per insert (transaction-batched)
//! - **Native Backend**: 10-100μs per insert (transaction-batched)
//! - **Bulk insert**: 10-100x faster with `bulk_insert_entities()`
//!
//! ## Memory Usage
//! - **Base overhead**: O(V + E) for graph storage
//! - **Cache overhead**: 10-20% additional memory
//! - **HNSW index**: 2-3x vector data size
//!
//! # Public API Organization
//!
//! This crate exports a clean, stable public API organized as follows:
//!
//! ## Core Types
//! - [`GraphEntity`] - Graph node/vertex representation
//! - [`GraphEdge`] - Graph edge/relationship representation
//! - [`GraphBackend`] - Unified trait for backend implementations
//! - [`SqliteGraphBackend`] - SQLite backend implementation
//! - [`CombinedGraphBackend`] - Combined-mode SQLite-authoritative wrapper
//! - [`V3Backend`] - Native V3 backend implementation
//!
//! ## Configuration
//! - [`BackendKind`] - Runtime backend selection enum
//! - [`GraphConfig`] - Unified configuration for both backends
//! - [`CombinedConfig`] - Combined-mode read/fallback configuration
//! - [`SqliteConfig`] - SQLite-specific options
//! - [`NativeConfig`] - Native-specific options
//! - [`open_graph()`] - Unified factory function
//!
//! ## Operations
//! - [`insert_node()`], [`insert_edge()`] - Single entity/edge insertion
//! - [`bulk_insert_entities()`], [`bulk_insert_edges()`] - Batch operations
//! - [`neighbors()`] - Direct neighbor queries
//! - [`bfs()`], [`k_hop()`], [`shortest_path()`] - Graph traversal algorithms
//! - [`pattern_engine`] - Pattern matching and triple storage
//!
//! ## Graph Algorithms
//! - [`pagerank`] - PageRank centrality
//! - [`betweenness_centrality`] - Betweenness centrality
//! - [`louvain_communities`] - Louvain community detection
//! - [`label_propagation`] - Label propagation algorithm
//!
//! ## Vector Search
//! - [`hnsw::HnswIndex`] - HNSW vector search index
//! - [`hnsw::HnswConfig`] - HNSW configuration
//! - [`hnsw::DistanceMetric`] - Distance metrics (Cosine, Euclidean, etc.)
//!
//! ## Utilities
//! - [`SqliteGraphError`] - Comprehensive error handling
//! - [`GraphSnapshot`] - MVCC snapshot system
//! - [`GraphIntrospection`] - Introspection and debugging APIs
//! - [`ProgressCallback`] - Algorithm progress tracking
//! - [`recovery`] - Database backup and restore utilities

// Core public modules
pub mod backend;
pub mod config;
pub mod cypher;
pub mod errors;
pub mod graph;
pub mod introspection;
pub mod snapshot;

// Re-export core utilities that are stable public APIs
pub use api_ergonomics::{Label, NodeId, PropertyKey, PropertyValue};
pub use graph_opt::{
    GraphEdgeCreate, GraphEntityCreate, bulk_insert_edges, bulk_insert_entities, cache_stats,
};
pub use index::{add_label, add_property};
pub use mvcc::{GraphSnapshot, SnapshotState, VersionedSnapshot};
pub use pattern_engine::{PatternTriple, TripleMatch, match_triples};
pub use pattern_engine_cache::match_triples_fast;
pub use query::GraphQuery;
pub use recovery::{dump_graph_to_path, load_graph_from_path, load_graph_from_reader};
pub use snapshot::SnapshotId;
pub use temporal::{
    LineageBarcode, TemporalBarcode, TemporalPersistencePoint, compute_temporal_barcode,
    cycle_rank_snapshot, cycle_scc_barcode, scc_lineage_barcode, temporal_persistence_sweep,
};
// Re-export backend implementations
pub use backend::native::v3::V3Backend as NativeGraphBackend;
pub use backend::{BackendDirection, ChainStep, GraphBackend};
pub use backend::{BackupResult, EdgeSpec, NeighborQuery, NodeSpec, SqliteGraphBackend};

// Re-export configuration and factory
pub use config::{BackendKind, GraphConfig, NativeConfig, SqliteConfig, open_graph};

// Re-export error types
pub use errors::SqliteGraphError;

// Re-export graph core types
pub use graph::{GraphEdge, GraphEntity, SqliteGraph};

// Re-export graph algorithms
pub use algo::async_traversal::{bfs_async, k_hop_async};
pub use algo::{
    betweenness_centrality, betweenness_centrality_with_progress, label_propagation,
    louvain_communities, louvain_communities_with_progress, pagerank, pagerank_with_progress,
};

// Re-export progress tracking
pub use progress::{ConsoleProgress, NoProgress, ProgressCallback, ProgressState};

// Re-export introspection API
pub use introspection::{EdgeCount, GraphIntrospection, IntrospectError};

// Internal modules - not part of public API
pub mod algo; // Public graph-algorithm library API (26 modules, ~530 tested fns)
mod api_ergonomics;
pub mod backend_selector;
pub mod bfs; // Public for tests
pub mod cache; // Public for tests
mod client; // Public for binary
mod fault_injection; // Public for tests
pub mod graph_opt; // Public for tests
pub mod index; // Public for tests
pub mod multi_hop; // Public for tests
mod pattern_engine_cache; // Already moved to core above
pub mod progress; // Public for tests and progress API usage
pub mod query_cache; // Public for internal use and tests
mod reasoning; // Public for binary
pub mod schema; // Public for tests // Public for tests

// Core public modules (these were accidentally removed)
pub mod mvcc; // Already exported above
pub mod pattern_engine; // Already exported above
pub mod query; // Already exported above
pub mod recovery; // Already exported above

pub mod typed_digraph;

/// Temporal topology analysis — SCC persistence barcode over the MVCC version
/// chain (ported from geographdb's `temporal_persistence_sweep`).
pub mod temporal;

// Modules that need to remain public for specific use cases
pub mod bench_gates; // Public for tests
pub mod bench_meta; // Public for tests
pub mod bench_regression; // Public for tests
pub mod bench_utils; // Public for tests
pub mod dsl; // Public for examples
pub mod hnsw;
pub mod inference; // Sparse inference engine
pub mod pattern; // Public for binary // HNSW vector search capabilities
pub mod sharding; // CSR sharded graph for prompt-local traversal

// Dependency monitoring module (feature-gated)
#[cfg(feature = "dependency-monitoring")]
pub mod dependency_monitor;

// Re-export cache statistics for benchmarking
pub use cache::CacheStats;
