//! Centrality algorithms for graph analysis.
//!
//! This module provides algorithms for measuring node importance in a graph
//! based on position and connectivity. Centrality measures help identify
//! influential nodes, bridges between communities, and key control points.
//!
//! # Available Algorithms
//!
//! - [`pagerank`] - PageRank centrality for identifying influential nodes
//! - [`betweenness_centrality`] - Betweenness centrality for finding bridge nodes
//!
//! # When to Use Centrality Algorithms
//!
//! - **PageRank**: Identify influential nodes in citation networks, rank web pages,
//!   find key entities in knowledge graphs, recommendation systems based on graph
//!   structure
//! - **Betweenness Centrality**: Find bridge nodes connecting communities,
//!   identify bottlenecks in communication networks, detect control points in flow
//!   networks, analyze information flow in social networks

use std::collections::VecDeque;

use ahash::AHashMap;

use crate::progress::ProgressCallback;
use crate::{errors::SqliteGraphError, graph::SqliteGraph};

/// Computes PageRank scores for all nodes in the graph.
///
/// PageRank measures node importance based on link structure. Nodes with many
/// incoming links from important nodes receive higher scores. Originally developed
/// by Google for ranking web pages.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `damping` - Damping factor (typically 0.85), representing probability of continuing random walk
/// * `iterations` - Number of power iteration iterations (20-50 recommended, fixed not convergence-based)
///
/// # Returns
/// Vector of (node_id, score) tuples sorted by score descending. Scores sum to approximately 1.0.
///
/// # Complexity
/// Time: O(k * |E|) where k = iterations
/// Space: O(|V|) for score storage
///
/// # Algorithm Details
/// Uses power iteration method (fixed iteration count for determinism):
/// 1. Initialize all nodes with equal score (1.0 / node_count)
/// 2. For each iteration:
///    - new_score = (1-d)/n + d * sum(incoming_scores / outgoing_count)
///    - Handle dangling nodes (no outgoing edges) by redistributing their score equally
/// 3. Sort results by score descending
///
/// # References
/// - Page, L., Brin, S., Motwani, R., & Winograd, T. (1999). "The PageRank Citation Ranking: Bringing Order to the Web."
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::pagerank};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let scores = pagerank(&graph, 0.85, 20)?;
/// # Ok(())
/// # }
/// ```
pub fn pagerank(
    graph: &SqliteGraph,
    damping: f64,
    iterations: usize,
) -> Result<Vec<(i64, f64)>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        return Ok(Vec::new());
    }

    // Initialize all nodes with equal score
    let mut scores: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 1.0 / n as f64)).collect();

    // Pre-compute outgoing counts for all nodes
    let mut outgoing_counts: AHashMap<i64, usize> = AHashMap::new();
    for &id in &all_ids {
        let count = graph.fetch_outgoing(id)?.len();
        outgoing_counts.insert(id, count);
    }

    // Power iteration
    for _ in 0..iterations {
        let mut new_scores: AHashMap<i64, f64> = AHashMap::new();

        // Initialize with teleport probability (1-d)/n
        let base_score = (1.0 - damping) / n as f64;
        for &id in &all_ids {
            new_scores.insert(id, base_score);
        }

        // Track total dangling score to redistribute
        let mut dangling_score = 0.0;

        // Distribute scores from outgoing edges
        for &id in &all_ids {
            let score = scores[&id];
            let out_count = outgoing_counts[&id];

            if out_count == 0 {
                // Dangling node - add score to redistribution pool
                dangling_score += score;
            } else {
                // Distribute score evenly to all outgoing neighbors
                let share = score / out_count as f64;
                for &neighbor in &graph.fetch_outgoing(id)? {
                    *new_scores
                        .get_mut(&neighbor)
                        .expect("invariant: neighbor in scores from initialization") +=
                        damping * share;
                }
            }
        }

        // Redistribute dangling score equally to all nodes
        let dangling_share = damping * dangling_score / n as f64;
        for (_, score) in new_scores.iter_mut() {
            *score += dangling_share;
        }

        scores = new_scores;
    }

    // Convert to sorted vector
    let mut result: Vec<(i64, f64)> = scores.into_iter().collect();
    result.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    Ok(result)
}

/// Computes PageRank scores with progress callback reporting.
///
/// This is the progress-reporting variant of [`pagerank`]. See that function
/// for full algorithm documentation.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `damping` - Damping factor (typically 0.85)
/// * `iterations` - Number of power iteration iterations
/// * `progress` - Callback for progress updates
///
/// # Progress Reporting
/// - Reports progress at each iteration: "PageRank iteration X/Y"
/// - Calls `on_complete()` when finished
/// - Calls `on_error()` if an error occurs
///
/// # Example
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::pagerank_with_progress};
/// use sqlitegraph::progress::NoProgress;
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let progress = NoProgress;
/// let scores = pagerank_with_progress(&graph, 0.85, 20, &progress)?;
/// # Ok(())
/// # }
/// ```
pub fn pagerank_with_progress<F>(
    graph: &SqliteGraph,
    damping: f64,
    iterations: usize,
    progress: &F,
) -> Result<Vec<(i64, f64)>, SqliteGraphError>
where
    F: ProgressCallback,
{
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        progress.on_complete();
        return Ok(Vec::new());
    }

    // Initialize all nodes with equal score
    let mut scores: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 1.0 / n as f64)).collect();

    // Pre-compute outgoing counts for all nodes
    let mut outgoing_counts: AHashMap<i64, usize> = AHashMap::new();
    for &id in &all_ids {
        let count = graph.fetch_outgoing(id)?.len();
        outgoing_counts.insert(id, count);
    }

    // Power iteration with progress reporting
    for iteration in 0..iterations {
        progress.on_progress(
            iteration + 1,
            Some(iterations),
            &format!("PageRank iteration {}", iteration + 1),
        );

        let mut new_scores: AHashMap<i64, f64> = AHashMap::new();

        // Initialize with teleport probability (1-d)/n
        let base_score = (1.0 - damping) / n as f64;
        for &id in &all_ids {
            new_scores.insert(id, base_score);
        }

        // Track total dangling score to redistribute
        let mut dangling_score = 0.0;

        // Distribute scores from outgoing edges
        for &id in &all_ids {
            let score = scores[&id];
            let out_count = outgoing_counts[&id];

            if out_count == 0 {
                // Dangling node - add score to redistribution pool
                dangling_score += score;
            } else {
                // Distribute score evenly to all outgoing neighbors
                let share = score / out_count as f64;
                for &neighbor in &graph.fetch_outgoing(id)? {
                    *new_scores
                        .get_mut(&neighbor)
                        .expect("invariant: neighbor in scores from initialization") +=
                        damping * share;
                }
            }
        }

        // Redistribute dangling score equally to all nodes
        let dangling_share = damping * dangling_score / n as f64;
        for (_, score) in new_scores.iter_mut() {
            *score += dangling_share;
        }

        scores = new_scores;
    }

    progress.on_complete();

    // Convert to sorted vector
    let mut result: Vec<(i64, f64)> = scores.into_iter().collect();
    result.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    Ok(result)
}

/// Computes betweenness centrality for all nodes in the graph.
///
/// Betweenness centrality measures how often a node appears on shortest paths
/// between other nodes. Bridge nodes (connecting different parts of the graph)
/// score higher. Useful for finding bottlenecks or control points in networks.
///
/// # Arguments
/// * `graph` - The graph to analyze
///
/// # Returns
/// Vector of (node_id, centrality) tuples sorted by centrality descending.
/// Values are normalized by default (divide by 2 for undirected graphs).
///
/// # Complexity
/// Time: O(|V| * |E|) for unweighted graphs (Brandes' algorithm)
/// Space: O(|V| + |E|) for BFS traversal and accumulation
///
/// # Algorithm Details
/// Implements Brandes' algorithm for unweighted graphs:
/// 1. For each node s, run BFS to compute shortest paths
/// 2. Track predecessors and path counts during BFS
/// 3. Accumulate dependency values (how much s depends on each node)
/// 4. Sum dependencies across all source nodes
///
/// Handles disconnected components gracefully (pairs with no path are ignored).
///
/// # Caveats
/// - Expensive for large graphs (O(VE) time complexity)
/// - Does not support edge weights (unweighted only)
/// - For graphs > 10K nodes, consider sampling approximation
///
/// # References
/// - Brandes, U. (2001). "A Faster Algorithm for Betweenness Centrality."
///
/// # Example
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::betweenness_centrality};
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let centrality = betweenness_centrality(&graph)?;
/// # Ok(())
/// # }
/// ```
pub fn betweenness_centrality(graph: &SqliteGraph) -> Result<Vec<(i64, f64)>, SqliteGraphError> {
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        return Ok(Vec::new());
    }

    // Initialize centrality scores
    let mut centrality: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

    // Brandes' algorithm: for each node as source
    for &s in &all_ids {
        // BFS from s
        let mut dist: AHashMap<i64, i64> = AHashMap::new();
        let mut sigma: AHashMap<i64, f64> = AHashMap::new(); // number of shortest paths
        let mut predecessors: AHashMap<i64, Vec<i64>> = AHashMap::new();

        // Initialize source
        dist.insert(s, 0);
        sigma.insert(s, 1.0);

        let mut queue = VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            for &w in &graph.fetch_outgoing(v)? {
                // First time discovering w
                if !dist.contains_key(&w) {
                    dist.insert(w, dist[&v] + 1);
                    queue.push_back(w);
                }

                // Found another shortest path to w through v
                if dist.get(&w) == Some(&(dist[&v] + 1)) {
                    *sigma.entry(w).or_insert(0.0) += sigma[&v];
                    predecessors.entry(w).or_default().push(v);
                }
            }
        }

        // Accumulate centrality (dependency propagation)
        let mut delta: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

        // Process nodes in reverse order of distance from s
        let mut nodes: Vec<i64> = dist.keys().copied().collect();
        nodes.sort_by_key(|&id| std::cmp::Reverse(dist[&id]));

        for w in nodes {
            if w == s {
                continue;
            }

            for &v in predecessors.get(&w).unwrap_or(&vec![]) {
                let contribution = (sigma[&v] / sigma[&w]) * (1.0 + delta[&w]);
                *delta
                    .get_mut(&v)
                    .expect("invariant: v in delta from initialization") += contribution;
            }

            if w != s {
                *centrality
                    .get_mut(&w)
                    .expect("invariant: w in centrality from initialization") += delta[&w];
            }
        }
    }

    // Convert to sorted vector
    let mut result: Vec<(i64, f64)> = centrality.into_iter().collect();
    result.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    Ok(result)
}

/// Computes betweenness centrality with progress callback reporting.
///
/// This is the progress-reporting variant of [`betweenness_centrality`]. See that function
/// for full algorithm documentation.
///
/// # Arguments
/// * `graph` - The graph to analyze
/// * `progress` - Callback for progress updates
///
/// # Progress Reporting
/// - Reports progress for each source node: "Betweenness: processing source X/Y"
/// - Total is the number of nodes in the graph
/// - Calls `on_complete()` when finished
/// - Calls `on_error()` if an error occurs
///
/// # Example
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use sqlitegraph::{SqliteGraph, algo::betweenness_centrality_with_progress};
/// use sqlitegraph::progress::NoProgress;
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... add nodes and edges ...
/// let progress = NoProgress;
/// let centrality = betweenness_centrality_with_progress(&graph, &progress)?;
/// # Ok(())
/// # }
/// ```
pub fn betweenness_centrality_with_progress<F>(
    graph: &SqliteGraph,
    progress: &F,
) -> Result<Vec<(i64, f64)>, SqliteGraphError>
where
    F: ProgressCallback,
{
    let all_ids = graph.all_entity_ids()?;
    let n = all_ids.len();

    if n == 0 {
        progress.on_complete();
        return Ok(Vec::new());
    }

    // Initialize centrality scores
    let mut centrality: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

    // Brandes' algorithm: for each node as source
    for (idx, &s) in all_ids.iter().enumerate() {
        progress.on_progress(
            idx + 1,
            Some(n),
            &format!("Betweenness: processing source {}/{}", idx + 1, n),
        );

        // BFS from s
        let mut dist: AHashMap<i64, i64> = AHashMap::new();
        let mut sigma: AHashMap<i64, f64> = AHashMap::new(); // number of shortest paths
        let mut predecessors: AHashMap<i64, Vec<i64>> = AHashMap::new();

        // Initialize source
        dist.insert(s, 0);
        sigma.insert(s, 1.0);

        let mut queue = VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            for &w in &graph.fetch_outgoing(v)? {
                // First time discovering w
                if !dist.contains_key(&w) {
                    dist.insert(w, dist[&v] + 1);
                    queue.push_back(w);
                }

                // Found another shortest path to w through v
                if dist.get(&w) == Some(&(dist[&v] + 1)) {
                    *sigma.entry(w).or_insert(0.0) += sigma[&v];
                    predecessors.entry(w).or_default().push(v);
                }
            }
        }

        // Accumulate centrality (dependency propagation)
        let mut delta: AHashMap<i64, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

        // Process nodes in reverse order of distance from s
        let mut nodes: Vec<i64> = dist.keys().copied().collect();
        nodes.sort_by_key(|&id| std::cmp::Reverse(dist[&id]));

        for w in nodes {
            if w == s {
                continue;
            }

            for &v in predecessors.get(&w).unwrap_or(&vec![]) {
                let contribution = (sigma[&v] / sigma[&w]) * (1.0 + delta[&w]);
                *delta
                    .get_mut(&v)
                    .expect("invariant: v in delta from initialization") += contribution;
            }

            if w != s {
                *centrality
                    .get_mut(&w)
                    .expect("invariant: w in centrality from initialization") += delta[&w];
            }
        }
    }

    progress.on_complete();

    // Convert to sorted vector
    let mut result: Vec<(i64, f64)> = centrality.into_iter().collect();
    result.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    Ok(result)
}
