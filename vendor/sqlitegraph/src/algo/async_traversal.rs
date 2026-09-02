use crate::{
    backend::{AsyncGraphBackend, BackendDirection, NeighborQuery},
    errors::SqliteGraphError,
    snapshot::SnapshotId,
};
use ahash::AHashSet;

/// Asynchronously traverse graph neighbors using BFS, querying neighbors of the frontier concurrently.
pub async fn bfs_async<B: AsyncGraphBackend>(
    backend: &B,
    snapshot_id: SnapshotId,
    start: i64,
    max_depth: u32,
) -> Result<Vec<i64>, SqliteGraphError> {
    // Verify start node exists
    let _ = backend.get_node(snapshot_id, start).await?;

    let mut visited = Vec::new();
    let mut seen = AHashSet::new();
    let mut current_frontier = vec![start];
    seen.insert(start);

    let mut depth = 0;
    while !current_frontier.is_empty() && depth <= max_depth {
        // Record visited nodes in this frontier
        visited.extend(current_frontier.iter().copied());

        if depth >= max_depth {
            break;
        }

        // Fetch neighbors of the entire frontier concurrently
        let mut futures = Vec::with_capacity(current_frontier.len());
        for &node in &current_frontier {
            let query = NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            };
            futures.push(backend.neighbors(snapshot_id, node, query));
        }

        let results = futures::future::join_all(futures).await;
        let mut next_frontier = Vec::new();

        for res in results {
            let neighbors = res?;
            for next in neighbors {
                if seen.insert(next) {
                    next_frontier.push(next);
                }
            }
        }

        current_frontier = next_frontier;
        depth += 1;
    }

    Ok(visited)
}

/// Asynchronously traverse graph neighbors up to k-hops, querying neighbors concurrently.
pub async fn k_hop_async<B: AsyncGraphBackend>(
    backend: &B,
    snapshot_id: SnapshotId,
    start: i64,
    k: u32,
    direction: BackendDirection,
) -> Result<Vec<i64>, SqliteGraphError> {
    let _ = backend.get_node(snapshot_id, start).await?;

    let mut visited = Vec::new();
    let mut seen = AHashSet::new();
    let mut current_frontier = vec![start];
    seen.insert(start);

    let mut depth = 0;
    while !current_frontier.is_empty() && depth <= k {
        visited.extend(current_frontier.iter().copied());

        if depth >= k {
            break;
        }

        let mut futures = Vec::with_capacity(current_frontier.len());
        for &node in &current_frontier {
            let query = NeighborQuery {
                direction,
                edge_type: None,
            };
            futures.push(backend.neighbors(snapshot_id, node, query));
        }

        let results = futures::future::join_all(futures).await;
        let mut next_frontier = Vec::new();

        for res in results {
            let neighbors = res?;
            for next in neighbors {
                if seen.insert(next) {
                    next_frontier.push(next);
                }
            }
        }

        current_frontier = next_frontier;
        depth += 1;
    }

    Ok(visited)
}
