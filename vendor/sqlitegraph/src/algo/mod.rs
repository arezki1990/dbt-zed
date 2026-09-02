//! Graph algorithms for centrality, community detection, and structure analysis.
//!
//! This module provides a collection of graph algorithms for analyzing graph
//! topology, identifying important nodes, and discovering community structure.
//! All algorithms are designed for unweighted, directed graphs and work with
//! both SQLite and Native backends.
//!
//! # Available Algorithms
//!
//! ## Centrality Algorithms
//!
//! - [`pagerank`] - PageRank centrality for identifying influential nodes
//! - [`betweenness_centrality`] - Betweenness centrality for finding bridge nodes
//!
//! ## Community Detection
//!
//! - [`label_propagation`] - Fast label propagation for community discovery
//! - [`louvain_communities`] - Louvain method for modularity optimization
//!
//! ## Structural Analysis
//!
//! - [`connected_components`] - Find all connected components in the graph
//! - [`weakly_connected_components`] - Find weakly connected components (undirected connectivity)
//! - [`strongly_connected_components`] - Find strongly connected components using Tarjan's algorithm
//! - [`cycle_basis`] - Minimal cycle basis for cycle explanation (Paton's algorithm)
//! - [`cycle_basis_bounded`] - Cycle basis with bounded enumeration
//! - [`cycle_basis_with_progress`] - Cycle basis with progress tracking
//! - [`CycleBasisBounds`] - Configuration for bounds (max_cycles, max_cycle_length, max_per_scc)
//! - [`CycleBasisResult`] - Result with cycles, SCC decomposition, helpers
//! - [`find_cycles_limited`] - Enumerate cycles up to a limit
//! - [`nodes_by_degree`] - Rank nodes by degree (hub detection)
//! - [`topological_sort`] - Compute topological ordering of nodes in DAGs
//!
//! ## Dependency Analysis
//!
//! - [`critical_path`] - Longest weighted path in DAG for bottleneck identification
//! - [`critical_path_with_progress`] - Critical path with progress tracking
//! - [`CriticalPathResult`] - Result with path, distance, bottlenecks, slack
//! - [`CriticalPathError`] - Error for non-DAG graphs
//!
//! ## Reachability Analysis
//!
//! - [`transitive_closure`] - Compute all-pairs reachability (can reach relationships)
//! - [`transitive_reduction`] - Remove redundant edges while preserving reachability
//! - [`TransitiveClosureBounds`] - Bounds for limiting transitive closure computation
//! - [`reachable_from`] - Forward reachability (what does this affect?)
//! - [`reverse_reachable_from`] - Backward reachability (what affects this?)
//! - [`can_reach`] - Point-to-point reachability check
//! - [`unreachable_from`] - Find unreachable nodes from entry
//!
//! ## Control Flow Analysis
//!
//! - [`dominators`] - Compute dominators and immediate dominator tree
//! - [`dominators_with_progress`] - Dominator computation with progress tracking
//! - [`DominatorResult`] - Dominance sets and immediate dominator tree
//! - [`post_dominators`] - Compute post-dominators and immediate post-dominator tree
//! - [`post_dominators_with_progress`] - Post-dominator computation with progress tracking
//! - [`post_dominators_auto_exit`] - Post-dominators with automatic exit detection
//! - [`PostDominatorResult`] - Post-dominance sets and immediate post-dominator tree
//! - [`dominance_frontiers`] - Compute dominance frontiers for SSA construction
//! - [`dominance_frontiers_with_progress`] - Dominance frontier computation with progress tracking
//! - [`iterated_dominance_frontiers`] - Compute iterated dominance frontiers for SSA φ-placement
//! - [`DominanceFrontierResult`] - Dominance frontier sets
//! - [`IteratedDominanceFrontierResult`] - Iterated dominance frontier result with φ-node placements
//! - [`natural_loops`] - Detect natural loops using back-edge dominance analysis
//! - [`natural_loops_with_progress`] - Natural loop detection with progress tracking
//! - [`NaturalLoop`] - Natural loop with header, back-edges, and body
//! - [`NaturalLoopsResult`] - Natural loop detection result with nesting analysis
//! - [`control_dependence_graph`] - Compute Control Dependence Graph from post-dominators
//! - [`control_dependence_from_exit`] - Compute CDG with automatic exit detection
//! - [`ControlDependenceResult`] - Control dependence edges and reverse mapping
//!
//! ## Program Analysis
//!
//! - [`backward_slice`] - Backward program slicing (what affects this node?)
//! - [`backward_slice_with_progress`] - Backward slicing with progress tracking
//! - [`forward_slice`] - Forward program slicing (what does this affect?)
//! - [`forward_slice_with_progress`] - Forward slicing with progress tracking
//! - [`SliceResult`] - Result with control_nodes, data_nodes, and slice_nodes
//!
//! ## Call Graph Analysis
//!
//! - [`collapse_sccs`] - Collapse SCCs to form condensation DAG for call graph analysis
//! - [`collapse_sccs_with_progress`] - SCC collapse with progress tracking
//! - [`SccCollapseResult`] - Result with node_to_supernode, supernode_members, supernode_edges
//!
//! ## Cut and Partitioning
//!
//! - [`min_st_cut`] - Minimum s-t edge cut for fault tolerance analysis
//! - [`min_st_cut_with_progress`] - Minimum s-t edge cut with progress tracking
//! - [`min_vertex_cut`] - Minimum vertex cut for critical node identification
//! - [`min_vertex_cut_with_progress`] - Minimum vertex cut with progress tracking
//! - [`MinCutResult`] - Result of minimum edge cut computation
//! - [`MinVertexCutResult`] - Result of minimum vertex cut computation
//! - [`partition_bfs_level`] - BFS-level graph partitioning for sharding
//! - [`partition_greedy`] - Greedy partitioning with boundary improvement
//! - [`partition_kway`] - Size-bounded k-way partitioning
//! - [`partition_kway_with_progress`] - K-way partitioning with progress tracking
//! - [`PartitionConfig`] - Configuration for k-way partitioning
//! - [`PartitionResult`] - Result of graph partitioning computation
//!
//! ## Path Analysis
//!
//! - [`enumerate_paths`] - Enumerate all execution paths using DFS with bounds
//! - [`enumerate_paths_with_progress`] - Path enumeration with progress tracking
//! - [`enumerate_paths_with_dominance`] - Path enumeration with dominance-based pruning
//! - [`enumerate_paths_with_dominance_progress`] - Dominance-constrained enumeration with progress tracking
//! - [`PathEnumerationConfig`] - Configuration for bounds (max_depth, max_paths, revisit_cap)
//! - [`PathEnumerationDominanceConfig`] - Configuration for constraint-based pruning
//! - [`PathClassification`] - Path classification (Normal, Error, Degenerate, Infinite)
//! - [`EnumeratedPath`] - Single execution path with nodes and classification
//! - [`PathEnumerationResult`] - Enumeration result with categorized paths and statistics
//! - [`PathEnumerationPruningStats`] - Statistics for dominance-based pruning effectiveness
//!
//! ## Observability Algorithms
//!
//! - [`happens_before_analysis`] - Event ordering for concurrent trace analysis
//! - [`impact_radius`] - Blast zone computation using bounded reachability
//! - [`impact_radius_with_progress`] - Impact radius with progress tracking
//! - [`VectorClock`] - Partial order data structure for happens-before analysis
//! - [`HappensBeforeResult`] - Result with concurrent pairs and race statistics
//! - [`ImpactRadiusConfig`] - Configuration for impact radius (max_distance, max_hops, weight_fn)
//! - [`ImpactRadiusResult`] - Result with blast_zone, distances, boundary, size
//! - [`TraceEvent`] - Runtime trace event representation
//! - [`Operation`] - Memory operation type (Read/Write)
//!
//! Note: [`WeightCallback`] and [`default_weight_fn`] are re-exported from the
//! dependency analysis section for use with impact radius computation.
//!
//! ## ML / Inference
//!
//! - [`find_subgraph_patterns`] - Bounded subgraph isomorphism for pattern matching
//! - [`find_subgraph_patterns_with_progress`] - Subgraph isomorphism with progress tracking
//! - [`SubgraphPatternBounds`] - Configuration for bounds (max_matches, timeout_ms, max_pattern_nodes)
//! - [`SubgraphMatchResult`] - Result with matches, patterns_found, computation_time_ms, bounded_hit
//! - [`structural_similarity`] - Structural similarity using isomorphism checking and MCS approximation
//! - [`structural_similarity_with_progress`] - Structural similarity with progress tracking
//! - [`SimilarityBounds`] - Configuration for bounds (max_matches, timeout_ms, similarity_threshold)
//! - [`SimilarityResult`] - Result with isomorphic, mcs_similarity, ged_distance, mcs_size
//! - [`rewrite_graph_patterns`] - DPO-style graph rewriting for pattern transformation
//! - [`rewrite_graph_patterns_with_progress`] - Graph rewriting with progress tracking
//! - [`RewriteRule`] - Rule specifying pattern, replacement, and interface
//! - [`RewriteBounds`] - Configuration for bounds (max_matches, validation)
//! - [`RewriteResult`] - Result with rewritten_graph, operations_applied, validation_errors
//!
//! ## Graph Diff
//!
//! - [`graph_diff()`] - Structural graph delta between two snapshots
//! - [`graph_diff_with_progress()`] - Graph diff with progress tracking
//! - [`validate_refactor()`] - Refactor validation with safety heuristics
//! - [`GraphDiffResult`] - Result with nodes/edges added/removed and similarity metrics
//! - [`NodeDelta`] - Node delta (nodes_added, nodes_removed)
//! - [`EdgeDelta`] - Edge delta (edges_added, edges_removed)
//! - [`RefactorValidation`] - Validation result with is_safe, breaking_changes, warnings
//!
//! ## Security & Compliance
//!
//! - [`propagate_taint_forward()`] - Forward taint propagation from sources to sinks
//! - [`propagate_taint_forward_with_progress()`] - Forward propagation with progress tracking
//! - [`propagate_taint_backward()`] - Backward taint propagation from sink to sources
//! - [`propagate_taint_backward_with_progress()`] - Backward propagation with progress tracking
//! - [`sink_reachability_analysis()`] - Full vulnerability detection (all sinks)
//! - [`sink_reachability_analysis_with_progress()`] - Sink analysis with progress tracking
//! - [`discover_sources_and_sinks()`] - Discover sources/sinks using custom callbacks
//! - [`discover_sources_and_sinks_default()`] - Discover using metadata-based detectors
//! - [`TaintResult`] - Result with sources, sinks_reached, tainted_nodes, source_sink_paths
//! - [`SourceCallback`] - Trait for custom source detection
//! - [`SinkCallback`] - Trait for custom sink detection
//! - [`MetadataSourceDetector`] - Default source detector using entity metadata
//! - [`MetadataSinkDetector`] - Default sink detector using entity metadata
//!
//! # Algorithm Characteristics
//!
//! | Algorithm | Time Complexity | Best For | Limitations |
//! |-----------|----------------|----------|-------------|
//! | PageRank | O(k × (V + E)) | Influence ranking | Requires connected graph for best results |
//! | Betweenness | O(V × (V + E)) | Bridge nodes | Expensive on large graphs |
//! | Label Propagation | O(k × E) | Fast clustering | Non-deterministic tiebreaking |
//! | Louvain | O(k × V × E) | Quality communities | Slower than label propagation |
//! | Connected Components | O(V + E) | Graph connectivity | None |
//! | Weakly Connected Components | O(V + E) | Undirected connectivity | None |
//! | Strongly Connected Components | O(V + E) | Cycle detection, loop analysis | None |
//! | Cycle Basis | O(V + E + C×V) | Cycle explanation, deadlock detection | C = number of cycles |
//! | Transitive Closure | O(V × (V + E)) | All-pairs reachability | O(V²) space, use bounds for large graphs |
//! | Transitive Reduction | O(V × (V + E)) | Graph simplification | Most meaningful for DAGs |
//! | Topological Sort | O(V + E) | Linear ordering of DAGs | Requires DAG (returns cycle error) |
//! | Critical Path | O(V + E) | Build systems, bottleneck identification | Requires DAG |
//! | Reachability | O(V + E) | Impact analysis, slicing | None |
//! | Dominators | O(V²) worst, faster in practice | CFG analysis, SSA construction | Requires single entry |
//! | Post-Dominators | O(V²) worst, faster in practice | CFG analysis, control dependence | Requires single exit or virtual exit |
//! | Dominance Frontiers | O(V²) worst | SSA φ-node placement, convergence points | Requires dominators |
//! | Iterated DF | O(V × iterations) | SSA construction, φ-node placement | Fixed-point iteration |
//! | Natural Loops | O(E × N) worst | Loop optimization, program analysis | Requires dominators |
//! | Control Dependence | O(E) after post-dom | Program slicing, impact analysis | Requires post-dominators |
//! | Program Slicing | O(V + E) | Bug isolation, impact analysis | Requires CDG + reachability |
//! | SCC Collapse | O(V + E) | Call graph analysis, mutual recursion detection | None |
//! | Path Enumeration | O(P × L) | Test coverage, symbolic execution | Bounds required for cyclic CFGs |
//! | Path Enumeration (Dominance) | O(P² × L) amortized | Path pruning for complex CFGs | Requires dominators, control dependence, natural loops |
//! | Min s-t Edge Cut | O(V × E²) | Fault tolerance, security boundaries | Requires connected source/sink |
//! | Min Vertex Cut | O(V × E²) | Critical node identification | Requires connected source/sink |
//! | BFS-Level Partitioning | O(V + E) | Graph sharding, locality | Local optimum based on seeds |
//! | Greedy Partitioning | O(I × E) | Cut minimization | Local optimum (I = iterations) |
//! | K-way Partitioning | O(V + E) | Multi-way sharding, load balancing | Requires k >= 2 |
//!
//! Where:
//! - V = number of vertices
//! - E = number of edges
//! - P = number of paths
//! - L = average path length
//! - k = number of iterations (algorithm-dependent)
//!
//! # Input Requirements
//!
//! ## Graph Connectivity
//!
//! - **Connected components**: Works on disconnected graphs (finds all components)
//! - **Weakly connected components**: Works on disconnected graphs (finds all components treating edges as undirected)
//! - **Strongly connected components**: Works on disconnected graphs (finds all SCCs including trivial)
//! - **Topological sort**: Only works on DAGs (returns cycle error with cycle path for cyclic graphs)
//! - **PageRank**: Handles disconnected components (splits rank)
//! - **Betweenness**: Handles disconnected components (each component separately)
//! - **Label propagation**: Works on disconnected graphs (each component independently)
//! - **Louvain**: Works on disconnected graphs (each component independently)
//! - **Transitive closure**: Works on disconnected graphs (each component independently)
//! - **Transitive reduction**: Works on disconnected graphs (each component independently)
//!
//! ## Edge Directionality
//!
//! All algorithms support **directed graphs**:
//!
//! - **Pagerank**: Follows outgoing edges (link-based ranking)
//! - **Betweenness**: Considers both directions for shortest paths
//! - **Label propagation**: Uses bidirectional edges (undirected view)
//! - **Louvain**: Uses bidirectional edges (undirected view)
//!
//! # Output Format
//!
//! ## Centrality Algorithms
//!
//! Return `Vec<(NodeId, Score)>` sorted by score descending:
//!
//! ```rust,ignore
//! # use sqlitegraph::algo::pagerank;
//! # let graph: sqlitegraph::SqliteGraph = unsafe { std::mem::zeroed() };
//! let results = pagerank(&graph)?;
//!
//! // Top 5 most influential nodes
//! for (node_id, score) in results.iter().take(5) {
//!     println!("Node {}: PageRank = {:.4}", node_id, score);
//! }
//! ```
//!
//! ## Community Detection
//!
//! Return `Vec<Vec<NodeId>>` where each inner vector is a community:
//!
//! ```rust,ignore
//! # use sqlitegraph::algo::louvain_communities;
//! # let graph: sqlitegraph::SqliteGraph = unsafe { std::mem::zeroed() };
//! let communities = louvain_communities(&graph)?;
//!
//! println!("Found {} communities", communities.len());
//! for (i, community) in communities.iter().enumerate() {
//!     println!("Community {}: {} nodes", i, community.len());
//! }
//! ```
//!
//! # Usage Patterns
//!
//! ## When to Use PageRank
//!
//! - **Identify influential nodes** in citation networks
//! - **Rank web pages** or documents by link structure
//! - **Find key entities** in knowledge graphs
//! - **Recommendation systems** based on graph structure
//!
//! ## When to Use Betweenness Centrality
//!
//! - **Find bridge nodes** connecting communities
//! - **Identify bottlenecks** in communication networks
//! - **Detect control points** in flow networks
//! - **Analyze information flow** in social networks
//!
//! ## When to Use Label Propagation
//!
//! - **Fast community detection** on large graphs
//! - **Exploratory analysis** where speed matters more than quality
//! - **Baseline comparison** for other clustering methods
//! - **Incremental clustering** where results update frequently
//!
//! ## When to Use Louvain
//!
//! - **High-quality community detection** where modularity matters
//! - **Hierarchical clustering** to reveal multi-scale structure
//! - **Research applications** requiring reproducible results
//! - **Final clustering** when offline computation is acceptable
//!
//! # Progress Tracking
//!
//! Long-running algorithms provide `_with_progress` variants:
//!
//! ```rust,ignore
//! use sqlitegraph::{
//!     algo::pagerank_with_progress,
//!     progress::ConsoleProgress
//! };
//!
//! let progress = ConsoleProgress::new();
//! let results = pagerank_with_progress(&graph, progress)?;
//! // Output: PageRank iteration 10/100...
//! ```
//!
//! Progress tracking is available for:
//! - [`pagerank_with_progress`]
//! - [`betweenness_centrality_with_progress`]
//! - [`louvain_communities_with_progress`]
//! - [`weakly_connected_components_with_progress`]
//! - [`transitive_closure_with_progress`]
//! - [`transitive_reduction_with_progress`]
//! - [`reachable_from_with_progress`]
//! - [`reverse_reachable_from_with_progress`]
//! - [`dominators_with_progress`]
//! - [`post_dominators_with_progress`]
//! - [`dominance_frontiers_with_progress`]
//! - [`natural_loops_with_progress`]
//! - [`enumerate_paths_with_progress`]
//! - [`enumerate_paths_with_dominance_progress`]
//! - [`critical_path_with_progress`]
//! - [`cycle_basis_with_progress`]
//! - [`backward_slice_with_progress`]
//! - [`forward_slice_with_progress`]
//! - [`find_subgraph_patterns_with_progress()`]
//! - [`structural_similarity_with_progress()`]
//! - [`graph_diff_with_progress()`]
//!
//! Use [`NoProgress`] for zero-overhead progress tracking (default).

// Re-export all public API functions for convenience
// Users can use `crate::algo::pagerank` instead of `crate::algo::centrality::pagerank`

// Centrality algorithms
pub use centrality::{
    betweenness_centrality, betweenness_centrality_with_progress, pagerank, pagerank_with_progress,
};

// Community detection algorithms
pub use community::{label_propagation, louvain_communities, louvain_communities_with_progress};

// Cycle analysis algorithms
pub use cycle_basis::{
    CycleBasisBounds, CycleBasisResult, cycle_basis, cycle_basis_bounded, cycle_basis_with_progress,
};

// Structural analysis algorithms
pub use scc::{SccResult, strongly_connected_components};
pub use structure::{connected_components, find_cycles_limited, nodes_by_degree};
pub use topological_sort::{TopoError, topological_sort};
pub use wcc::{weakly_connected_components, weakly_connected_components_with_progress};

// Dependency analysis algorithms
pub use critical_path::{
    CriticalPathError, CriticalPathResult, WeightCallback, critical_path,
    critical_path_with_progress, default_weight_fn,
};

// Reachability analysis algorithms
pub use reachability::{
    can_reach, reachable_from, reachable_from_with_progress, reverse_reachable_from,
    reverse_reachable_from_with_progress, unreachable_from,
};
pub use transitive_closure::{
    TransitiveClosureBounds, transitive_closure, transitive_closure_with_progress,
};
pub use transitive_reduction::{transitive_reduction, transitive_reduction_with_progress};

// Control flow analysis algorithms
pub use control_dependence::{
    ControlDependenceResult, control_dependence_from_exit, control_dependence_graph,
};
pub use dominance_frontiers::{
    DominanceFrontierResult, IteratedDominanceFrontierResult, dominance_frontiers,
    dominance_frontiers_with_progress, iterated_dominance_frontiers,
};
pub use dominators::{DominatorResult, dominators, dominators_with_progress};
pub use natural_loops::{
    NaturalLoop, NaturalLoopsResult, natural_loops, natural_loops_from_exit,
    natural_loops_with_progress,
};
pub use post_dominators::{
    PostDominatorResult, post_dominators, post_dominators_auto_exit, post_dominators_with_progress,
};

// Program analysis algorithms
pub use program_slicing::{
    SliceResult, backward_slice, backward_slice_with_progress, forward_slice,
    forward_slice_with_progress,
};

// Call graph analysis algorithms
pub use call_graph_analysis::{SccCollapseResult, collapse_sccs, collapse_sccs_with_progress};

// Cut and partitioning algorithms
pub use cut_partition::{
    MinCutResult, MinVertexCutResult, PartitionConfig, PartitionResult, min_st_cut,
    min_st_cut_with_progress, min_vertex_cut, min_vertex_cut_with_progress, partition_bfs_level,
    partition_greedy, partition_kway, partition_kway_with_progress,
};

// Path analysis algorithms
pub use path_enumeration::{
    EnumeratedPath, PathClassification, PathEnumerationConfig, PathEnumerationDominanceConfig,
    PathEnumerationPruningStats, PathEnumerationResult, enumerate_paths,
    enumerate_paths_with_dominance, enumerate_paths_with_dominance_progress,
    enumerate_paths_with_progress,
};

// Observability algorithms
pub use observability::{
    HappensBeforeResult, ImpactRadiusConfig, ImpactRadiusResult, Operation, TraceEvent,
    VectorClock, happens_before_analysis, impact_radius, impact_radius_with_progress,
};
// WeightCallback and default_weight_fn are re-exported from critical_path module

// Subgraph isomorphism algorithms
pub use subgraph_isomorphism::{
    SubgraphMatchResult, SubgraphPatternBounds, find_subgraph_patterns,
    find_subgraph_patterns_with_progress,
};

// Structural similarity algorithms
pub use graph_similarity::{
    SimilarityBounds, SimilarityResult, structural_similarity, structural_similarity_with_progress,
};

// Graph rewriting algorithms
pub use graph_rewriting::{
    RewriteBounds, RewriteOperation, RewriteResult, RewriteRule, rewrite_graph_patterns,
    rewrite_graph_patterns_with_progress,
};

// Graph diff algorithms
pub use graph_diff::{
    EdgeDelta, GraphDiffResult, NodeDelta, RefactorValidation, graph_diff,
    graph_diff_with_progress, validate_refactor,
};

// Security & Compliance algorithms
pub use taint_analysis::{
    MetadataSinkDetector, MetadataSourceDetector, SinkCallback, SourceCallback, TaintResult,
    discover_sources_and_sinks, discover_sources_and_sinks_default, propagate_taint_backward,
    propagate_taint_backward_with_progress, propagate_taint_forward,
    propagate_taint_forward_with_progress, sink_reachability_analysis,
    sink_reachability_analysis_with_progress,
};

// Backend-agnostic algorithms (work with any GraphBackend)
pub use backend::{
    BfsIter, ConnectedComponentsIter, DfsIter, GraphIterator, TopologicalSortIter,
    betweenness_centrality as betweenness_centrality_backend, bfs_iter, connected_components_iter,
    dfs_iter, dfs_traversal, k_hop_neighbors, pagerank as pagerank_backend,
    shortest_path as shortest_path_backend, strongly_connected_components as scc_backend,
    topological_sort as topological_sort_backend, topological_sort_iter,
};

// Module declarations
mod call_graph_analysis;
mod centrality;
mod community;
mod control_dependence;
mod critical_path;
mod cut_partition;
mod cycle_basis;
mod dominance_frontiers;
mod dominators;
mod graph_diff;
mod graph_rewriting;
mod graph_similarity;
mod natural_loops;
mod observability;
mod path_enumeration;
mod post_dominators;
mod program_slicing;
mod reachability;
mod scc;
mod structure;
mod subgraph_isomorphism;
mod taint_analysis;
mod topological_sort;
mod transitive_closure;
mod transitive_reduction;
mod wcc;

// Backend-agnostic algorithms module
pub mod backend;

// Async traversals (BFS, K-Hop)
pub mod async_traversal;

// Test module
#[cfg(test)]
mod tests;
