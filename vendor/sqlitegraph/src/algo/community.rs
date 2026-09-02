//! Community detection algorithms for graph analysis.
//!
//! This module provides algorithms for discovering communities (clusters) in graphs.
//! Community detection groups nodes that are more densely connected to each other
//! than to nodes outside the group.
//!
//! # Available Algorithms
//!
//! - [`label_propagation`] - Fast label propagation for community discovery
//! - [`louvain_communities`] - Louvain method for modularity optimization
//!
//! # When to Use Community Detection
//!
//! - **Label Propagation**: Fast community detection on large graphs, exploratory
//!   analysis where speed matters more than quality, baseline comparison for other
//!   clustering methods, incremental clustering where results update frequently
//! - **Louvain**: High-quality community detection where modularity matters,
//!   hierarchical clustering to reveal multi-scale structure, research applications
//!   requiring reproducible results, final clustering when offline computation is
//!   acceptable

use ahash::AHashMap;

use crate::progress::ProgressCallback;
use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Label Propagation algorithm for fast community detection.
///
/// Each node starts with its own label, then iteratively adopts the most frequent
/// label among its neighbors. Converges when no labels change or max_iterations reached.
///
/// This is a near-linear time algorithm suitable for large graphs. Uses deterministic
/// tiebreaking (smallest label wins) for reproducible results.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `max_iterations` - Maximum number of iterations to prevent infinite loops (typically 5-10)
///
/// # Returns
/// Communities as vectors of node IDs, sorted by smallest node ID in each community.
///
/// # Complexity
/// Time: O(k * |E|) where k = iterations (typically 5-10)
/// Space: O(|V|) for label storage
///
/// # Algorithm Details
/// - Initialize each node with unique label (node ID)
/// - Iteratively adopt most frequent neighbor label
/// - Bidirectional edges (both incoming and outgoing neighbors)
/// - Deterministic tiebreaking: smallest label wins
/// - Early stopping when converged (no labels change)
///
/// # References
/// - Raghavan, U. N., Albert, R., & Kumara, S. (2007). "Near linear time algorithm to detect community structures in large-scale networks."
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::label_propagation};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let communities = label_propagation(&graph, 10)?;
/// # Ok(())
/// # }
/// ```
pub fn label_propagation(
    graph: &SqliteGraph,
    max_iterations: usize,
) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Initialize: each node gets its own label
    let mut labels: AHashMap<i64, i64> = all_ids.iter().map(|&id| (id, id)).collect();

    // For deterministic results, process nodes in sorted order
    let mut node_order: Vec<i64> = all_ids.clone();
    node_order.sort();

    // Iterative label propagation
    for _iteration in 0..max_iterations {
        let mut any_changed = false;

        for &node in &node_order {
            // Count neighbor labels
            let mut label_counts: AHashMap<i64, usize> = AHashMap::new();

            // Count outgoing neighbors
            for &neighbor in &graph.fetch_outgoing(node)? {
                let neighbor_label = labels.get(&neighbor).unwrap_or(&neighbor);
                *label_counts.entry(*neighbor_label).or_insert(0) += 1;
            }

            // Count incoming neighbors
            for &neighbor in &graph.fetch_incoming(node)? {
                let neighbor_label = labels.get(&neighbor).unwrap_or(&neighbor);
                *label_counts.entry(*neighbor_label).or_insert(0) += 1;
            }

            // Find most frequent label (deterministic tiebreak: smallest label)
            if let Some((&_most_frequent_label, _)) = label_counts
                .iter()
                .max_by_key(|(_, count)| *count)
                .map(|(label, count)| (label, *count))
            {
                // In case of ties, max_by_key returns arbitrary one
                // So we need to find all with max count and take smallest label
                let max_count = *label_counts.values().max().unwrap_or(&0);
                let best_label = label_counts
                    .iter()
                    .filter(|(_, count)| **count == max_count)
                    .map(|(&label, _)| label)
                    .min()
                    .unwrap_or(node);

                if let Some(current_label) = labels.get(&node)
                    && *current_label != best_label
                {
                    labels.insert(node, best_label);
                    any_changed = true;
                }
            }
        }

        if !any_changed {
            break;
        }
    }

    // Group nodes by final label
    let mut communities_map: AHashMap<i64, Vec<i64>> = AHashMap::new();
    for (node, label) in &labels {
        communities_map.entry(*label).or_default().push(*node);
    }

    // Convert to sorted vector of communities
    let mut communities: Vec<Vec<i64>> = communities_map.into_values().collect();
    for community in &mut communities {
        community.sort();
    }
    communities.sort_by(|a, b| a.first().cmp(&b.first()));

    Ok(communities)
}

/// Louvain method for community detection via modularity optimization.
///
/// Iteratively moves nodes to maximize modularity (how many edges are within
/// communities vs between communities). This is a simplified single-pass version.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `max_iterations` - Maximum number of iterations to prevent infinite loops (typically 10-20)
///
/// # Returns
/// Communities as vectors of node IDs, sorted by smallest node ID in each community.
///
/// # Complexity
/// Time: O(k * |V| * |E|) where k = iterations
/// Space: O(|V|) for community assignments and degrees
///
/// # Algorithm Details
/// Simplified single-pass modularity optimization (no multi-level aggregation):
/// 1. Initialize each node in its own community
/// 2. Calculate total edges (m) and node degrees
/// 3. Iteratively move nodes to maximize modularity delta:
///    ΔQ = (2*edges_to_community - node_degree*community_degree/m) / (2*m)
/// 4. Stop when no moves improve modularity
///
/// Modularity measures edge density within communities vs random expectation.
/// Higher values indicate better community structure (typical range: 0.3-0.7).
///
/// # Caveats
/// - Simplified version (no multi-level aggregation)
/// - May converge to local optima (not guaranteed global optimum)
/// - Performance depends on graph structure and edge distribution
///
/// # References
/// - Blondel, V. D., et al. (2008). "Fast unfolding of communities in large networks."
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::louvain_communities};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let communities = louvain_communities(&graph, 10)?;
/// # Ok(())
/// # }
/// ```
pub fn louvain_communities(
    graph: &SqliteGraph,
    max_iterations: usize,
) -> Result<Vec<Vec<i64>>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Calculate total edges (m) and node degrees
    let mut total_edges = 0usize;
    let mut degrees: AHashMap<i64, usize> = AHashMap::new();

    for &id in &all_ids {
        let out_count = graph.fetch_outgoing(id)?.len();
        let in_count = graph.fetch_incoming(id)?.len();
        let degree = out_count + in_count;
        degrees.insert(id, degree);
        total_edges += degree;
    }

    // Total edges m (undirected: each edge counted twice, so m = sum_degrees / 2)
    let m = total_edges as f64 / 2.0;

    if m == 0.0 {
        // No edges - each node is its own community
        let mut communities: Vec<Vec<i64>> = all_ids.iter().map(|&id| vec![id]).collect();
        communities.sort();
        return Ok(communities);
    }

    // Initialize: each node in its own community
    let mut communities: AHashMap<i64, i64> = all_ids.iter().map(|&id| (id, id)).collect();

    // For deterministic results, process nodes in sorted order
    let mut node_order: Vec<i64> = all_ids.clone();
    node_order.sort();

    // Iterative modularity optimization
    for _iteration in 0..max_iterations {
        let mut any_moved = false;

        for &node in &node_order {
            let current_community = *communities.get(&node).unwrap_or(&node);
            let node_degree = *degrees.get(&node).unwrap_or(&0) as f64;

            // Find neighbor communities
            let mut community_connections: AHashMap<i64, f64> = AHashMap::new();

            // Count outgoing edges
            for &neighbor in &graph.fetch_outgoing(node)? {
                let neighbor_community = *communities.get(&neighbor).unwrap_or(&neighbor);
                *community_connections
                    .entry(neighbor_community)
                    .or_insert(0.0) += 1.0;
            }

            // Count incoming edges
            for &neighbor in &graph.fetch_incoming(node)? {
                let neighbor_community = *communities.get(&neighbor).unwrap_or(&neighbor);
                *community_connections
                    .entry(neighbor_community)
                    .or_insert(0.0) += 1.0;
            }

            // Calculate modularity delta for moving to each neighbor's community
            let mut best_community = current_community;
            let mut best_delta = 0.0f64;

            for (&target_community, &edges_to_community) in &community_connections {
                if target_community == current_community {
                    continue;
                }

                // Calculate sum of degrees in target community
                let community_degree: f64 = communities
                    .iter()
                    .filter(|(_, comm)| **comm == target_community)
                    .map(|(&node, _)| *degrees.get(&node).unwrap_or(&0) as f64)
                    .sum();

                // Modularity delta formula:
                // ΔQ = (edges_in / m) - (edges_total / m)^2
                // Simplified for single node move:
                // ΔQ = [(2*edges_to_community - node_degree*community_degree/m) / (2*m)]

                let delta =
                    (2.0 * edges_to_community - node_degree * community_degree / m) / (2.0 * m);

                if delta > best_delta {
                    best_delta = delta;
                    best_community = target_community;
                }
            }

            // Move node if it improves modularity
            if best_community != current_community {
                communities.insert(node, best_community);
                any_moved = true;
            }
        }

        if !any_moved {
            break;
        }
    }

    // Group nodes by final community
    let mut communities_map: AHashMap<i64, Vec<i64>> = AHashMap::new();
    for (node, community) in &communities {
        communities_map.entry(*community).or_default().push(*node);
    }

    // Convert to sorted vector of communities
    let mut result: Vec<Vec<i64>> = communities_map.into_values().collect();
    for community in &mut result {
        community.sort();
    }
    result.sort_by(|a, b| a.first().cmp(&b.first()));

    Ok(result)
}

/// Louvain method for community detection with progress callback reporting.
///
/// This is the progress-reporting variant of [`louvain_communities`]. See that function
/// for full algorithm documentation.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `max_iterations` - Maximum number of iterations
/// * `progress` - Callback for progress updates
///
/// # Progress Reporting
/// - Reports progress for each iteration pass: "Louvain pass X"
/// - Total is None (convergence unknown)
/// - Calls `on_complete()` when finished or converged
/// - Calls `on_error()` if an error occurs
///
/// # Example
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::louvain_communities_with_progress};
/// use sqlitegraph::progress::NoProgress;
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let progress = NoProgress;
/// let communities = louvain_communities_with_progress(&graph, 10, &progress)?;
/// # Ok(())
/// # }
/// ```
pub fn louvain_communities_with_progress<F>(
    graph: &SqliteGraph,
    max_iterations: usize,
    progress: &F,
) -> Result<Vec<Vec<i64>>, SqliteGraphError>
where
    F: ProgressCallback,
{
    let all_ids = graph.all_entity_ids()?;

    if all_ids.is_empty() {
        progress.on_complete();
        return Ok(Vec::new());
    }

    // Calculate total edges (m) and node degrees
    let mut total_edges = 0usize;
    let mut degrees: AHashMap<i64, usize> = AHashMap::new();

    for &id in &all_ids {
        let out_count = graph.fetch_outgoing(id)?.len();
        let in_count = graph.fetch_incoming(id)?.len();
        let degree = out_count + in_count;
        degrees.insert(id, degree);
        total_edges += degree;
    }

    // Total edges m (undirected: each edge counted twice, so m = sum_degrees / 2)
    let m = total_edges as f64 / 2.0;

    if m == 0.0 {
        progress.on_complete();
        // No edges - each node is its own community
        let mut communities: Vec<Vec<i64>> = all_ids.iter().map(|&id| vec![id]).collect();
        communities.sort();
        return Ok(communities);
    }

    // Initialize: each node in its own community
    let mut communities: AHashMap<i64, i64> = all_ids.iter().map(|&id| (id, id)).collect();

    // For deterministic results, process nodes in sorted order
    let mut node_order: Vec<i64> = all_ids.clone();
    node_order.sort();

    // Iterative modularity optimization with progress reporting
    for iteration in 0..max_iterations {
        progress.on_progress(
            iteration + 1,
            None,
            &format!("Louvain pass {}", iteration + 1),
        );

        let mut any_moved = false;

        for &node in &node_order {
            let current_community = *communities.get(&node).unwrap_or(&node);
            let node_degree = *degrees.get(&node).unwrap_or(&0) as f64;

            // Find neighbor communities
            let mut community_connections: AHashMap<i64, f64> = AHashMap::new();

            // Count outgoing edges
            for &neighbor in &graph.fetch_outgoing(node)? {
                let neighbor_community = *communities.get(&neighbor).unwrap_or(&neighbor);
                *community_connections
                    .entry(neighbor_community)
                    .or_insert(0.0) += 1.0;
            }

            // Count incoming edges
            for &neighbor in &graph.fetch_incoming(node)? {
                let neighbor_community = *communities.get(&neighbor).unwrap_or(&neighbor);
                *community_connections
                    .entry(neighbor_community)
                    .or_insert(0.0) += 1.0;
            }

            // Calculate modularity delta for moving to each neighbor's community
            let mut best_community = current_community;
            let mut best_delta = 0.0f64;

            for (&target_community, &edges_to_community) in &community_connections {
                if target_community == current_community {
                    continue;
                }

                // Calculate sum of degrees in target community
                let community_degree: f64 = communities
                    .iter()
                    .filter(|(_, comm)| **comm == target_community)
                    .map(|(&node, _)| *degrees.get(&node).unwrap_or(&0) as f64)
                    .sum();

                // Modularity delta formula:
                // ΔQ = (edges_in / m) - (edges_total / m)^2
                // Simplified for single node move:
                // ΔQ = [(2*edges_to_community - node_degree*community_degree/m) / (2*m)]

                let delta =
                    (2.0 * edges_to_community - node_degree * community_degree / m) / (2.0 * m);

                if delta > best_delta {
                    best_delta = delta;
                    best_community = target_community;
                }
            }

            // Move node if it improves modularity
            if best_community != current_community {
                communities.insert(node, best_community);
                any_moved = true;
            }
        }

        if !any_moved {
            break;
        }
    }

    progress.on_complete();

    // Group nodes by final community
    let mut communities_map: AHashMap<i64, Vec<i64>> = AHashMap::new();
    for (node, community) in &communities {
        communities_map.entry(*community).or_default().push(*node);
    }

    // Convert to sorted vector of communities
    let mut result: Vec<Vec<i64>> = communities_map.into_values().collect();
    for community in &mut result {
        community.sort();
    }
    result.sort_by(|a, b| a.first().cmp(&b.first()));

    Ok(result)
}
