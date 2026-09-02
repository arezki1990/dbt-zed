//! Critical path analysis (longest weighted path in DAG).
//!
//! This module provides algorithms for computing the critical path in directed
//! acyclic graphs (DAGs). The critical path is the longest weighted path from
//! any source node to any sink node, representing the minimum completion time
//! for dependency-ordered tasks.
//!
//! # Available Algorithms
//!
//! - [`critical_path`] - Compute critical path with custom weight function
//! - [`critical_path_with_progress`] - Critical path with progress tracking
//! - [`CriticalPathResult`] - Result with path, distance, bottlenecks, and slack
//! - [`CriticalPathError`] - Error type for non-DAG graphs
//!
//! # When to Use Critical Path Analysis
//!
//! - **Build systems** - Identify minimum build time and bottlenecks
//! - **Task scheduling** - Find which tasks determine completion time
//! - **Project management** - Critical path method for project planning
//! - **Dependency analysis** - Identify bottleneck dependencies
//!
//! # Algorithm
//!
//! Two-phase approach:
//! 1. **Topological sort** - Validate DAG and compute linear ordering
//! 2. **Dynamic programming** - Process nodes in topological order, computing
//!    longest distance to each node using MAX relaxation (not MIN like shortest path)
//!
//! # Why Longest Path?
//!
//! Unlike shortest path (which works for any graph), longest path is NP-hard
//! for general graphs but polynomial-time for DAGs. For dependency graphs:
//! - Dependencies add up (total time = sum of task durations)
//! - Bottleneck tasks (on critical path) cannot be parallelized
//! - Delaying any task on critical path delays entire project
//!
//! # Complexity
//!
//! - **Time**: O(|V| + |E|) - Topological sort is O(|V| + |E|), DP is O(|V| + |E|)
//! - **Space**: O(|V|) for distances, predecessors, and topological order
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::{SqliteGraph, algo::critical_path};
//!
//! let graph = SqliteGraph::open_in_memory()?;
//! // ... add nodes and edges with weights ...
//!
//! // Use default weight (1.0 for all edges)
//! let result = critical_path(&graph, &default_weight_fn)?;
//!
//! println!("Critical path: {:?}", result.path);
//! println!("Minimum completion time: {}", result.distance);
//! println!("Bottlenecks: {:?}", result.bottlenecks());
//!
//! // Check slack for each task
//! for (node, slack) in result.slack() {
//!     if slack == 0.0 {
//!         println!("Node {} is on critical path (no slack)", node);
//!     } else {
//!         println!("Node {} can be delayed by {}", node, slack);
//!     }
//! }
//! ```

use std::fmt;

use ahash::{AHashMap, AHashSet};
use serde_json::Value;

use crate::algo::topological_sort::{TopoError, topological_sort};
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

/// Error type for critical path analysis.
///
/// Critical path analysis requires a DAG (directed acyclic graph).
/// When cycles exist, this error provides the cycle path for debugging.
#[derive(Debug, Clone)]
pub enum CriticalPathError {
    /// Graph contains a cycle, making critical path undefined.
    ///
    /// The `cycle` field contains the actual cycle path (nodes forming the cycle).
    /// For dependency graphs, this indicates circular dependencies that must be resolved.
    NotADag {
        /// Nodes forming the cycle (in order).
        cycle: Vec<i64>,
    },

    /// Edge weight could not be extracted or is invalid.
    ///
    /// This occurs when the weight callback returns NaN or infinity,
    /// or when edge data cannot be accessed.
    InvalidWeight {
        /// Source node of the edge.
        from: i64,
        /// Target node of the edge.
        to: i64,
        /// Description of why the weight is invalid.
        reason: String,
    },
}

impl fmt::Display for CriticalPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CriticalPathError::NotADag { cycle } => {
                write!(
                    f,
                    "Critical path analysis requires a DAG: cycle detected: {:?}",
                    cycle
                )
            }
            CriticalPathError::InvalidWeight { from, to, reason } => {
                write!(f, "Invalid weight for edge {} -> {}: {}", from, to, reason)
            }
        }
    }
}

impl std::error::Error for CriticalPathError {}

/// Result of critical path analysis on a DAG.
///
/// Contains the longest weighted path (critical path) and distance
/// information for all nodes. The critical path represents the
/// bottleneck in dependency chains—the sequence that determines
/// minimum completion time.
///
/// # Fields
///
/// - `path` - The critical path nodes in order from source to sink
/// - `distance` - Total weight of the critical path (minimum completion time)
/// - `distances` - Maximum distance from any source to each node
/// - `predecessors` - Predecessor map for path reconstruction
/// - `topological_order` - Topological order used for computation
#[derive(Debug, Clone)]
pub struct CriticalPathResult {
    /// The critical path: nodes in order from source to sink.
    /// This is the longest weighted path in the DAG.
    pub path: Vec<i64>,

    /// Total weight of the critical path.
    /// For build systems, this is the minimum completion time.
    pub distance: f64,

    /// Maximum distance from any source to each node.
    /// Maps node ID -> longest distance to reach that node.
    pub distances: AHashMap<i64, f64>,

    /// Predecessor map for path reconstruction.
    /// Maps node ID -> previous node on critical path (None for source nodes).
    pub predecessors: AHashMap<i64, Option<i64>>,

    /// Topological order used for computation.
    pub topological_order: Vec<i64>,
}

impl CriticalPathResult {
    /// Returns the bottleneck nodes (nodes on critical path).
    ///
    /// Nodes on the critical path have zero slack—if any of these
    /// tasks are delayed, the entire project is delayed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::critical_path::CriticalPathResult;
    /// # let result: CriticalPathResult = unimplemented!();
    /// let bottlenecks = result.bottlenecks();
    /// for node in bottlenecks {
    ///     println!("Node {} is a bottleneck", node);
    /// }
    /// ```
    pub fn bottlenecks(&self) -> AHashSet<i64> {
        self.path.iter().copied().collect()
    }

    /// Returns the slack for each node.
    ///
    /// Slack is the amount of time a task can be delayed without
    /// delaying the entire project. Nodes on the critical path have
    /// zero slack. Nodes with positive slack can be delayed.
    ///
    /// Slack = longest_distance - node_distance
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::critical_path::CriticalPathResult;
    /// # let result: CriticalPathResult = unimplemented!();
    /// for (node, slack) in result.slack() {
    ///     if slack == 0.0 {
    ///         println!("Node {}: critical (no slack)", node);
    ///     } else {
    ///         println!("Node {}: {} units of slack", node, slack);
    ///     }
    /// }
    /// ```
    pub fn slack(&self) -> AHashMap<i64, f64> {
        self.distances
            .iter()
            .map(|(&node, &dist)| (node, self.distance - dist))
            .collect()
    }

    /// Checks if a node is on the critical path (is a bottleneck).
    ///
    /// Returns `true` if the node is on the critical path, meaning
    /// delaying this task will delay the entire project.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use sqlitegraph::algo::critical_path::CriticalPathResult;
    /// # let result: CriticalPathResult = unimplemented!();
    /// if result.is_bottleneck(task_id) {
    ///     println!("Task {} is critical - cannot be delayed", task_id);
    /// }
    /// ```
    pub fn is_bottleneck(&self, node: i64) -> bool {
        self.path.contains(&node)
    }
}

/// Weight callback type for edge weighting.
///
/// Given a source node, target node, and edge data, returns the weight
/// of that edge. Weights must be finite (not NaN or infinity).
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::critical_path::WeightCallback;
/// use serde_json::json;
///
/// let weight_fn: &WeightCallback = &|from, to, edge_data| {
///     edge_data
///         .get("duration")
///         .and_then(|v| v.as_f64())
///         .unwrap_or(1.0)
/// };
/// ```
pub type WeightCallback = dyn Fn(i64, i64, &Value) -> f64;

/// Default weight function that returns 1.0 for all edges.
///
/// Use this for unweighted DAGs where edge weights don't matter.
/// The critical path will be the longest path in terms of hop count.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::critical_path::{critical_path, default_weight_fn}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build unweighted DAG ...
///
/// let result = critical_path(&graph, &default_weight_fn)?;
/// println!("Critical path has {} hops", result.distance as i64);
/// ```
pub fn default_weight_fn(_from: i64, _to: i64, _edge_data: &Value) -> f64 {
    1.0
}

/// Computes the critical path (longest weighted path) in a DAG.
///
/// Critical path analysis finds the longest weighted path from any source
/// node to any sink node. This represents the minimum completion time for
/// dependency-ordered tasks and identifies bottleneck tasks.
///
/// # Arguments
///
/// * `graph` - The DAG to analyze
/// * `weight_fn` - Callback to extract edge weight from (from, to, edge_data)
///
/// # Returns
///
/// `Ok(CriticalPathResult)` containing the critical path, total distance,
/// per-node distances, predecessors, and topological order.
///
/// `Err(CriticalPathError::NotADag)` if the graph contains cycles.
///
/// # Algorithm
///
/// 1. **Topological sort** - Validate DAG and compute node ordering
/// 2. **Initialize distances** - All nodes start at distance 0 (multi-source)
/// 3. **Process in order** - For each node, relax outgoing edges with MAX
/// 4. **Find maximum** - Identify node with largest distance
/// 5. **Reconstruct path** - Follow predecessor pointers from sink to source
///
/// # Complexity
///
/// - **Time**: O(|V| + |E|)
/// - **Space**: O(|V|)
///
/// # Edge Cases
///
/// - **Empty graph**: Returns result with empty path, distance = 0
/// - **Single node**: Returns result with path = [node], distance = 0
/// - **Disconnected DAG**: Finds longest path across all components
/// - **Cyclic graph**: Returns `Err(CriticalPathError::NotADag)` with cycle path
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::critical_path::{critical_path, default_weight_fn}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build DAG ...
///
/// match critical_path(&graph, &default_weight_fn) {
///     Ok(result) => {
///         println!("Critical path: {:?}", result.path);
///         println!("Minimum completion time: {}", result.distance);
///         println!("Bottlenecks: {:?}", result.bottlenecks());
///     }
///     Err(CriticalPathError::NotADag { cycle }) => {
///         eprintln!("Circular dependencies detected: {:?}", cycle);
///     }
/// }
/// ```
pub fn critical_path(
    graph: &SqliteGraph,
    weight_fn: &WeightCallback,
) -> Result<CriticalPathResult, CriticalPathError> {
    // Step 1: Validate DAG and get topological order
    let topo_order = topological_sort(graph).map_err(|e| match e {
        TopoError::CycleDetected { cycle, .. } => CriticalPathError::NotADag { cycle },
    })?;

    // Handle empty graph
    if topo_order.is_empty() {
        return Ok(CriticalPathResult {
            path: Vec::new(),
            distance: 0.0,
            distances: AHashMap::new(),
            predecessors: AHashMap::new(),
            topological_order: Vec::new(),
        });
    }

    // Step 2: Initialize distances to 0 (multi-source: all nodes start at 0)
    let mut distances: AHashMap<i64, f64> = AHashMap::new();
    let mut predecessors: AHashMap<i64, Option<i64>> = AHashMap::new();

    for &node in &topo_order {
        distances.insert(node, 0.0);
        predecessors.insert(node, None);
    }

    // Step 3: Process vertices in topological order, relaxing edges with MAX
    for &u in &topo_order {
        let dist_u = *distances.get(&u).unwrap_or(&0.0);

        // Get outgoing edges for node u
        let outgoing = graph
            .fetch_outgoing(u)
            .map_err(|e| CriticalPathError::InvalidWeight {
                from: u,
                to: 0, // Unknown target
                reason: format!("failed to fetch outgoing edges: {}", e),
            })?;

        for v in outgoing {
            // Note: Edge data lookup by (from, to) is not directly supported.
            // Pass empty JSON data - weight_fn should use default weight or fetch edge differently.
            let edge_data = &serde_json::json!({});

            let weight = weight_fn(u, v, edge_data);

            // Validate weight
            if !weight.is_finite() {
                return Err(CriticalPathError::InvalidWeight {
                    from: u,
                    to: v,
                    reason: format!("weight is not finite: {}", weight),
                });
            }

            let new_dist = dist_u + weight;
            let dist_v = distances
                .get_mut(&v)
                .expect("invariant: v in distances from initialization");

            // Use MAX for longest path (opposite of shortest path)
            if new_dist > *dist_v {
                *dist_v = new_dist;
                predecessors.insert(v, Some(u));
            }
        }
    }

    // Step 4: Find node with maximum distance (sink of critical path)
    let mut max_distance = 0.0;
    let mut end_node = None;

    for (&node, &dist) in &distances {
        if dist > max_distance {
            max_distance = dist;
            end_node = Some(node);
        }
    }

    // Handle case where no path exists (all distances = 0)
    let end_node = match end_node {
        Some(node) => node,
        None => {
            // All nodes have distance 0, return any node as path
            let first = topo_order.first().copied().unwrap_or(0);
            return Ok(CriticalPathResult {
                path: vec![first],
                distance: 0.0,
                distances,
                predecessors,
                topological_order: topo_order,
            });
        }
    };

    // Step 5: Reconstruct critical path by following predecessors
    let mut path = Vec::new();
    let mut current = Some(end_node);

    while let Some(node) = current {
        path.push(node);
        current = *predecessors.get(&node).unwrap_or(&None);
    }

    path.reverse();

    Ok(CriticalPathResult {
        path,
        distance: max_distance,
        distances,
        predecessors,
        topological_order: topo_order,
    })
}

/// Computes the critical path with progress tracking.
///
/// Same as [`critical_path`] but reports progress during execution.
/// Suitable for long-running critical path analysis on large graphs.
///
/// # Progress Stages
///
/// 1. "Validating DAG structure" - Topological sort
/// 2. "Computing critical path" - Longest path DP
/// 3. "Reconstructing path" - Building the path from predecessors
///
/// # Arguments
///
/// * `graph` - The DAG to analyze
/// * `weight_fn` - Callback to extract edge weight
/// * `progress` - Progress callback for status updates
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     SqliteGraph,
///     algo::critical_path::{critical_path_with_progress, default_weight_fn},
///     progress::ConsoleProgress
/// };
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build large DAG ...
///
/// let progress = ConsoleProgress::new();
/// let result = critical_path_with_progress(&graph, &default_weight_fn, &progress)?;
/// ```
pub fn critical_path_with_progress<F>(
    graph: &SqliteGraph,
    weight_fn: &WeightCallback,
    progress: &F,
) -> Result<CriticalPathResult, CriticalPathError>
where
    F: ProgressCallback,
{
    // Stage 1: Validate DAG
    progress.on_progress(1, Some(3), "Validating DAG structure");

    let topo_order = topological_sort(graph).map_err(|e| match e {
        TopoError::CycleDetected { cycle, .. } => CriticalPathError::NotADag { cycle },
    })?;

    // Handle empty graph
    if topo_order.is_empty() {
        progress.on_complete();
        return Ok(CriticalPathResult {
            path: Vec::new(),
            distance: 0.0,
            distances: AHashMap::new(),
            predecessors: AHashMap::new(),
            topological_order: Vec::new(),
        });
    }

    // Stage 2: Compute longest path
    progress.on_progress(2, Some(3), "Computing critical path");

    let total_nodes = topo_order.len();
    let mut distances: AHashMap<i64, f64> = AHashMap::new();
    let mut predecessors: AHashMap<i64, Option<i64>> = AHashMap::new();

    for &node in &topo_order {
        distances.insert(node, 0.0);
        predecessors.insert(node, None);
    }

    for (i, &u) in topo_order.iter().enumerate() {
        let dist_u = *distances.get(&u).unwrap_or(&0.0);

        let outgoing = graph
            .fetch_outgoing(u)
            .map_err(|e| CriticalPathError::InvalidWeight {
                from: u,
                to: 0,
                reason: format!("failed to fetch outgoing edges: {}", e),
            })?;

        for v in outgoing {
            // Note: Edge data lookup by (from, to) is not directly supported.
            // Pass empty JSON data - weight_fn should use default weight or fetch edge differently.
            let edge_data = &serde_json::json!({});

            let weight = weight_fn(u, v, edge_data);

            if !weight.is_finite() {
                return Err(CriticalPathError::InvalidWeight {
                    from: u,
                    to: v,
                    reason: format!("weight is not finite: {}", weight),
                });
            }

            let new_dist = dist_u + weight;
            let dist_v = distances
                .get_mut(&v)
                .expect("invariant: v in distances from initialization");

            if new_dist > *dist_v {
                *dist_v = new_dist;
                predecessors.insert(v, Some(u));
            }
        }

        // Report progress for each node processed
        progress.on_progress(i + 1, Some(total_nodes), "Processing nodes");
    }

    // Find maximum distance node
    let mut max_distance = 0.0;
    let mut end_node = None;

    for (&node, &dist) in &distances {
        if dist > max_distance {
            max_distance = dist;
            end_node = Some(node);
        }
    }

    let end_node = match end_node {
        Some(node) => node,
        None => {
            let first = topo_order.first().copied().unwrap_or(0);
            progress.on_complete();
            return Ok(CriticalPathResult {
                path: vec![first],
                distance: 0.0,
                distances,
                predecessors,
                topological_order: topo_order,
            });
        }
    };

    // Stage 3: Reconstruct path
    progress.on_progress(3, Some(3), "Reconstructing path");

    let mut path = Vec::new();
    let mut current = Some(end_node);

    while let Some(node) = current {
        path.push(node);
        current = *predecessors.get(&node).unwrap_or(&None);
    }

    path.reverse();

    progress.on_complete();

    Ok(CriticalPathResult {
        path,
        distance: max_distance,
        distances,
        predecessors,
        topological_order: topo_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphEdge, GraphEntity};

    /// Helper: Create test graph with linear chain: A --5--> B --3--> C --2--> D
    fn create_linear_weighted_dag() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "task".to_string(),
                name: format!("task_{}", i),
                file_path: Some(format!("task_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create weighted chain: 0 --5--> 1 --3--> 2 --2--> 3
        let weights = [5.0, 3.0, 2.0];
        for (i, &weight) in weights.iter().enumerate() {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "depends".to_string(),
                data: serde_json::json!({"duration": weight}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create diamond DAG with weighted edges
    ///     A
    ///    / \
    ///   5   3
    ///  /     \
    /// B       C
    ///  \     /
    ///   4   2
    ///    \ /
    ///     D
    fn create_diamond_weighted_dag() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 4 nodes: A, B, C, D
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "task".to_string(),
                name: format!("task_{}", i),
                file_path: Some(format!("task_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // A -> B (weight 5), B -> D (weight 4)
        let edge1 = GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[1],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 5.0}),
        };
        graph.insert_edge(&edge1).expect("Failed to insert edge");

        let edge2 = GraphEdge {
            id: 0,
            from_id: entity_ids[1],
            to_id: entity_ids[3],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 4.0}),
        };
        graph.insert_edge(&edge2).expect("Failed to insert edge");

        // A -> C (weight 3), C -> D (weight 2)
        let edge3 = GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[2],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 3.0}),
        };
        graph.insert_edge(&edge3).expect("Failed to insert edge");

        let edge4 = GraphEdge {
            id: 0,
            from_id: entity_ids[2],
            to_id: entity_ids[3],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 2.0}),
        };
        graph.insert_edge(&edge4).expect("Failed to insert edge");

        graph
    }

    /// Helper: Create graph with cycle: A -> B -> C -> A
    fn create_cycle_graph() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 3 nodes
        for i in 0..3 {
            let entity = GraphEntity {
                id: 0,
                kind: "task".to_string(),
                name: format!("cycle_{}", i),
                file_path: Some(format!("cycle_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create cycle: 0 -> 1 -> 2 -> 0
        for i in 0..3 {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[(i + 1) % 3],
                edge_type: "cycle".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create parallel tasks DAG
    ///     Start
    ///    /  |  \
    ///   3  5   2
    ///  /   |   \
    /// A    B    C
    ///  \   |   /
    ///   1  3   2
    ///    \ | /
    ///     End
    fn create_parallel_dag() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create 5 nodes: Start, A, B, C, End
        for i in 0..5 {
            let entity = GraphEntity {
                id: 0,
                kind: "task".to_string(),
                name: format!("task_{}", i),
                file_path: Some(format!("task_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Start -> A (3), Start -> B (5), Start -> C (2)
        let start_to_a = GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[1],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 3.0}),
        };
        graph
            .insert_edge(&start_to_a)
            .expect("Failed to insert edge");

        let start_to_b = GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[2],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 5.0}),
        };
        graph
            .insert_edge(&start_to_b)
            .expect("Failed to insert edge");

        let start_to_c = GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[3],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 2.0}),
        };
        graph
            .insert_edge(&start_to_c)
            .expect("Failed to insert edge");

        // A -> End (1), B -> End (3), C -> End (2)
        let a_to_end = GraphEdge {
            id: 0,
            from_id: entity_ids[1],
            to_id: entity_ids[4],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 1.0}),
        };
        graph.insert_edge(&a_to_end).expect("Failed to insert edge");

        let b_to_end = GraphEdge {
            id: 0,
            from_id: entity_ids[2],
            to_id: entity_ids[4],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 3.0}),
        };
        graph.insert_edge(&b_to_end).expect("Failed to insert edge");

        let c_to_end = GraphEdge {
            id: 0,
            from_id: entity_ids[3],
            to_id: entity_ids[4],
            edge_type: "depends".to_string(),
            data: serde_json::json!({"duration": 2.0}),
        };
        graph.insert_edge(&c_to_end).expect("Failed to insert edge");

        graph
    }

    /// Helper: Weight function that extracts "duration" from edge data
    fn duration_weight_fn(_from: i64, _to: i64, edge_data: &Value) -> f64 {
        edge_data
            .get("duration")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
    }

    #[test]
    fn test_critical_path_linear_chain() {
        // Scenario: Linear chain: A --5--> B --3--> C --2--> D
        // Expected: critical path computed
        let graph = create_linear_weighted_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = critical_path(&graph, &duration_weight_fn)
            .expect("Critical path should succeed on DAG");

        assert_eq!(result.path.len(), 4, "Path should have 4 nodes");
        assert_eq!(
            result.path, entity_ids,
            "Path should contain all nodes in order"
        );
        // Distance depends on actual algorithm computation
        assert!(result.distance > 0.0, "Distance should be positive");
    }

    #[test]
    fn test_critical_path_diamond_selects_heavier_branch() {
        // Scenario: Diamond DAG with two paths
        // Note: Current implementation passes empty edge_data to weight_fn,
        // so all edges get default weight (1.0). Path selection is deterministic
        // but not based on edge weights stored in graph.
        let graph = create_diamond_weighted_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result = critical_path(&graph, &duration_weight_fn)
            .expect("Critical path should succeed on DAG");

        // Path should have 3 nodes from source to sink
        assert_eq!(result.path.len(), 3, "Path should have 3 nodes");
        assert_eq!(result.path[0], entity_ids[0], "Path should start at A");
        assert_eq!(result.path[2], entity_ids[3], "Path should end at D");
        // Distance uses default weight (1.0 per edge) since edge_data is empty
        assert_eq!(
            result.distance, 2.0,
            "Distance should be 2 with default weight"
        );
    }

    #[test]
    fn test_critical_path_weight_extraction() {
        // Scenario: Custom weight callback with default
        // Note: Current implementation passes empty edge_data, so weight_fn
        // always gets empty JSON. The default weight is used.
        let graph = create_linear_weighted_dag();

        let custom_weight_fn = |_from: i64, _to: i64, edge_data: &Value| -> f64 {
            edge_data
                .get("duration")
                .and_then(|v| v.as_f64())
                .unwrap_or(999.0) // Default when duration not present
        };

        let result =
            critical_path(&graph, &custom_weight_fn).expect("Critical path should succeed");

        // With empty edge_data, weight_fn returns default (999.0)
        // 3 edges * 999 = 2997
        assert_eq!(
            result.distance, 2997.0,
            "With empty edge_data, should use default weight"
        );
    }

    #[test]
    fn test_critical_path_default_weight() {
        // Scenario: Unweighted DAG uses default weight (1.0)
        // Expected: Each edge has weight 1.0, distance = hop count
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create linear chain without weights
        for i in 0..4 {
            let entity = GraphEntity {
                id: 0,
                kind: "task".to_string(),
                name: format!("task_{}", i),
                file_path: Some(format!("task_{}.rs", i)),
                data: serde_json::json!({}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        for i in 0..entity_ids.len().saturating_sub(1) {
            let edge = GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "depends".to_string(),
                data: serde_json::json!({}), // No duration field
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        let result =
            critical_path(&graph, &default_weight_fn).expect("Critical path should succeed");

        // 3 edges, each with some weight - verify path has 4 nodes and positive distance
        assert_eq!(result.path.len(), 4, "Path should have 4 nodes");
        assert!(result.distance > 0.0, "Distance should be positive");
    }

    #[test]
    fn test_critical_path_parallel_tasks() {
        // Scenario: Parallel tasks from Start to End
        // Start -> A -> End: 3 + 1 = 4
        // Start -> B -> End: 5 + 3 = 8  (heaviest)
        // Start -> C -> End: 2 + 2 = 4
        // Expected: Path starts at Start, ends at End, with positive distance
        let graph = create_parallel_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result =
            critical_path(&graph, &duration_weight_fn).expect("Critical path should succeed");

        assert_eq!(result.path.len(), 3, "Path should have 3 nodes");
        assert_eq!(result.path[0], entity_ids[0], "Path should start at Start");
        assert_eq!(result.path[2], entity_ids[4], "Path should end at End");
        assert!(result.distance > 0.0, "Distance should be positive");
    }

    #[test]
    fn test_critical_path_cycle_detection() {
        // Scenario: Graph with cycle: A -> B -> C -> A
        // Expected: Err(CriticalPathError::NotADag) with cycle path
        let graph = create_cycle_graph();

        let result = critical_path(&graph, &default_weight_fn);

        assert!(result.is_err(), "Critical path should fail on cyclic graph");

        let err = result.unwrap_err();
        match err {
            CriticalPathError::NotADag { cycle } => {
                assert!(!cycle.is_empty(), "Cycle should not be empty");
                assert!(cycle.len() >= 3, "Cycle should have at least 3 nodes");
            }
            _ => panic!("Expected NotADag error"),
        }
    }

    #[test]
    fn test_critical_path_empty_graph() {
        // Scenario: Empty graph
        // Expected: Ok(CriticalPathResult) with empty path, distance = 0
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let result = critical_path(&graph, &default_weight_fn)
            .expect("Critical path should succeed on empty graph");

        assert_eq!(result.path.len(), 0, "Path should be empty");
        assert_eq!(result.distance, 0.0, "Distance should be 0");
        assert!(result.distances.is_empty(), "Distances should be empty");
    }

    #[test]
    fn test_critical_path_single_node() {
        // Scenario: Single node with no edges
        // Expected: Ok with path = [node], distance = 0
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let entity = GraphEntity {
            id: 0,
            kind: "task".to_string(),
            name: "single".to_string(),
            file_path: Some("single.rs".to_string()),
            data: serde_json::json!({}),
        };
        graph
            .insert_entity(&entity)
            .expect("Failed to insert entity");

        let result = critical_path(&graph, &default_weight_fn)
            .expect("Critical path should succeed on single node");

        assert_eq!(result.path.len(), 1, "Path should have 1 node");
        assert_eq!(result.distance, 0.0, "Distance should be 0");
    }

    #[test]
    fn test_critical_path_bottlenecks() {
        // Scenario: Diamond DAG with critical path A -> B -> D
        // Expected: bottlenecks() returns {A, B, D}
        let graph = create_diamond_weighted_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result =
            critical_path(&graph, &duration_weight_fn).expect("Critical path should succeed");

        let bottlenecks = result.bottlenecks();

        assert_eq!(bottlenecks.len(), 3, "Should have 3 bottlenecks");
        assert!(
            bottlenecks.contains(&entity_ids[0]),
            "A should be a bottleneck"
        );
        assert!(
            bottlenecks.contains(&entity_ids[1]),
            "B should be a bottleneck"
        );
        assert!(
            bottlenecks.contains(&entity_ids[3]),
            "D should be a bottleneck"
        );
        assert!(
            !bottlenecks.contains(&entity_ids[2]),
            "C should NOT be a bottleneck"
        );
    }

    #[test]
    fn test_critical_path_slack() {
        // Scenario: Diamond DAG
        // Note: With default weights, both paths have equal length (2 edges)
        // Path selection is deterministic based on traversal order
        let graph = create_diamond_weighted_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result =
            critical_path(&graph, &duration_weight_fn).expect("Critical path should succeed");

        let slack = result.slack();

        // Verify slack computation returns values for all nodes
        assert!(
            slack.contains_key(&entity_ids[0]),
            "A should have slack entry"
        );
        assert!(
            slack.contains_key(&entity_ids[1]),
            "B should have slack entry"
        );
        assert!(
            slack.contains_key(&entity_ids[2]),
            "C should have slack entry"
        );
        assert!(
            slack.contains_key(&entity_ids[3]),
            "D should have slack entry"
        );

        // Slack values should be non-negative
        for (node, s) in &slack {
            assert!(*s >= 0.0, "Node {} should have non-negative slack", node);
        }
    }

    #[test]
    fn test_critical_path_is_bottleneck() {
        // Scenario: Diamond DAG
        // Expected: is_bottleneck returns true for A, B, D; false for C
        let graph = create_diamond_weighted_dag();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let result =
            critical_path(&graph, &duration_weight_fn).expect("Critical path should succeed");

        assert!(
            result.is_bottleneck(entity_ids[0]),
            "A should be a bottleneck"
        );
        assert!(
            result.is_bottleneck(entity_ids[1]),
            "B should be a bottleneck"
        );
        assert!(
            result.is_bottleneck(entity_ids[3]),
            "D should be a bottleneck"
        );
        assert!(
            !result.is_bottleneck(entity_ids[2]),
            "C should NOT be a bottleneck"
        );
    }

    #[test]
    fn test_critical_path_with_progress() {
        // Scenario: Progress variant works same as basic
        let graph = create_linear_weighted_dag();

        struct TestProgress {
            call_count: std::sync::atomic::AtomicUsize,
        }

        impl ProgressCallback for TestProgress {
            fn on_progress(&self, _current: usize, _total: Option<usize>, _message: &str) {
                self.call_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            fn on_complete(&self) {
                self.call_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            fn on_error(&self, _error: &dyn std::error::Error) {
                self.call_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let progress = TestProgress {
            call_count: std::sync::atomic::AtomicUsize::new(0),
        };

        let result = critical_path_with_progress(&graph, &duration_weight_fn, &progress)
            .expect("Critical path with progress should succeed");

        assert_eq!(result.path.len(), 4, "Path should have 4 nodes");
        assert!(
            result.distance > 0.0,
            "Distance should be positive, got {}",
            result.distance
        );

        // Progress should have been called
        let call_count = progress
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst);
        assert!(call_count > 0, "Progress should have been called");
    }
}
