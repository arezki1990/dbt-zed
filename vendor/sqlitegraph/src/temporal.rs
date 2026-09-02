//! Temporal topology analysis over the MVCC version chain.
//!
//! Sweeps across retained [`VersionedSnapshot`](crate::mvcc::VersionedSnapshot)s
//! and tracks how the graph topology evolves across versions. Two analysis
//! layers are provided, each with a different topological invariant:
//!
//! 1. **H₀ — connected-component lifecycle** ([`scc_lineage_barcode`]):
//!    Tracks strongly-connected components by stable membership identity
//!    (Jaccard-overlap matching across adjacent versions). Each component
//!    gets an exact `(birth_version, death_version)` bar. This is the
//!    discrete analogue of 0-dimensional persistent homology with version
//!    number as the filtration parameter.
//!
//! 2. **Cycle-rank trajectory + circular-dependency barcode**
//!    ([`cycle_rank_snapshot`] + [`cycle_scc_barcode`]):
//!    - `cycle_rank_snapshot` computes β₁ = E − V + W (the cyclomatic number:
//!      the rank of the cycle space of the underlying undirected graph).
//!    - `cycle_scc_barcode` tracks the birth/death of **non-trivial SCCs**
//!      (size ≥ 2). A non-trivial SCC in a directed graph guarantees at least
//!      one directed cycle exists — for a call graph this is a circular
//!      dependency. The barcode shows when circular deps form and dissolve.
//!
//! # What this is, and what it is NOT
//!
//! **Is:** exact H₀ persistent homology over a version-chain filtration, plus
//! a cycle-rank (β₁) scalar trajectory, plus an SCC-based circular-dependency
//! lifecycle barcode. These three together give a research-grade picture of
//! topological evolution for code graphs.
//!
//! **Is NOT:** full multi-dimensional persistent homology with individual
//! cycle-generator tracking (true H₁). Classical H₁ PH would track each
//! *independent* cycle as its own bar via a cycle-basis matching across
//! versions — that requires a different algorithm (cycle basis per version +
//! generator matching) and is a separate research effort. The
//! [`cycle_scc_barcode`] here detects cycle *presence* (via SCCs) and tracks
//! cycle-containing components across versions, which is the practically
//! useful signal for code-graph evolution but is not the same invariant as
//! classical H₁.
//!
//! # Background: SCCs, β₀, β₁
//!
//! A **strongly connected component** (SCC) is a maximal group of nodes where
//! every node can reach every other node following directed edges. Tarjan's
//! algorithm finds all SCCs in O(V + E). The number of SCCs is β₀ (the
//! 0th Betti number) for the directed connectivity structure.
//!
//! **β₁** (the 1st Betti number / cyclomatic number) counts independent
//! cycles in the underlying undirected graph: β₁ = E − V + W, where W is the
//! number of weakly connected components. For code graphs this is the total
//! "excess connectivity" — how many edges beyond a tree.
//!
//! A non-trivial SCC (size ≥ 2) guarantees at least one directed cycle: every
//! pair of nodes in the SCC is mutually reachable, so a circular path exists.
//! This is why tracking non-trivial SCCs is the circular-dependency signal.

use std::collections::{HashMap, HashSet};

use ahash::AHashSet;

use crate::mvcc::{SnapshotState, VersionedSnapshot};

// ════════════════════════════════════════════════════════════════════════════
// Data types
// ════════════════════════════════════════════════════════════════════════════

/// One measurement point in the temporal persistence sweep.
#[derive(Debug, Clone)]
pub struct TemporalPersistencePoint {
    /// Version number at this measurement.
    pub version: u64,
    /// Total nodes in this snapshot (V).
    pub active: usize,
    /// Number of strongly connected components (β₀).
    pub n_components: usize,
    /// Size of the largest SCC.
    pub largest_size: usize,
    /// `largest_size / active`, or 0.0 when `active == 0`.
    pub fraction_largest: f32,
    /// Cyclomatic number β₁ = E − V + W (independent undirected cycles).
    pub cycle_rank: usize,
    /// Number of non-trivial SCCs (size ≥ 2) — directed-cycle-containing.
    pub n_nontrivial_sccs: usize,
    /// Size of the largest non-trivial SCC, or 0 if none exist.
    pub largest_nontrivial_size: usize,
}

/// One bar in the approximate persistence barcode (LIFO closure policy).
///
/// **Deprecated in favour of [`LineageBarcode`]**, which uses stable component
/// identity matching instead of the LIFO approximation. This struct and
/// [`compute_temporal_barcode`] are retained for backwards compatibility with
/// the original geographdb port. The LIFO policy closes the most-recently-born
/// bar when the component count drops, which is unreliable when multiple
/// components merge or split simultaneously.
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalBarcode {
    /// Version where this component first appeared.
    pub birth_version: u64,
    /// Version where this component was last seen, or `None` if it survived to
    /// the end of the sweep.
    pub death_version: Option<u64>,
    /// Largest size this component reached across its lifetime.
    pub peak_size: usize,
}

/// One bar in the exact lineage-tracked persistence barcode.
///
/// Unlike [`TemporalBarcode`] (which uses a LIFO count-delta approximation),
/// `LineageBarcode` tracks components by their **actual membership identity**
/// — two SCCs at adjacent versions are linked only if they share members
/// (Jaccard overlap). Birth and death are determined by real lineage, not by
/// component-count deltas.
///
/// Use [`scc_lineage_barcode`] for the exact H₀ component barcode, and
/// [`cycle_scc_barcode`] for the circular-dependency barcode (non-trivial
/// SCC lifecycle).
#[derive(Debug, Clone, PartialEq)]
pub struct LineageBarcode {
    /// Version where this lineage first appeared.
    pub birth_version: u64,
    /// Version where this lineage was last seen, or `None` if it survived to
    /// the end of the sweep.
    pub death_version: Option<u64>,
    /// Member-set size when this lineage was born.
    pub birth_size: usize,
    /// Peak member-set size across the lineage's life.
    pub peak_size: usize,
    /// Member-set size at the last observed version (or the sweep's last
    /// version if the lineage survived).
    pub final_size: usize,
    /// Number of versions this lineage was observed (including birth).
    pub versions_seen: usize,
}

// ════════════════════════════════════════════════════════════════════════════
// Topological primitives (operate on SnapshotState, no SQLite I/O)
// ════════════════════════════════════════════════════════════════════════════

/// Compute the strongly-connected components of a [`SnapshotState`].
///
/// Tarjan's single-pass algorithm operating directly on the in-memory
/// adjacency map (no SQLite I/O). Returns a vector of components, each a set
/// of node IDs.
pub fn strongly_connected_components_snapshot(state: &SnapshotState) -> Vec<HashSet<i64>> {
    let nodes: Vec<i64> = state.outgoing.keys().copied().collect();
    if nodes.is_empty() {
        return Vec::new();
    }

    let mut index_counter: i64 = 0;
    let mut stack: Vec<i64> = Vec::new();
    let mut on_stack: AHashSet<i64> = AHashSet::new();
    let mut indices: HashMap<i64, i64> = HashMap::new();
    let mut lowlink: HashMap<i64, i64> = HashMap::new();
    let mut components: Vec<HashSet<i64>> = Vec::new();

    fn strongconnect(
        v: i64,
        state: &SnapshotState,
        index_counter: &mut i64,
        stack: &mut Vec<i64>,
        on_stack: &mut AHashSet<i64>,
        indices: &mut HashMap<i64, i64>,
        lowlink: &mut HashMap<i64, i64>,
        components: &mut Vec<HashSet<i64>>,
    ) {
        indices.insert(v, *index_counter);
        lowlink.insert(v, *index_counter);
        *index_counter += 1;
        stack.push(v);
        on_stack.insert(v);

        if let Some(neighbors) = state.get_outgoing(v) {
            for &w in neighbors {
                if !indices.contains_key(&w) {
                    strongconnect(
                        w,
                        state,
                        index_counter,
                        stack,
                        on_stack,
                        indices,
                        lowlink,
                        components,
                    );
                    if let (Some(wl), Some(vl)) =
                        (lowlink.get(&w).copied(), lowlink.get(&v).copied())
                    {
                        lowlink.insert(v, vl.min(wl));
                    }
                } else if on_stack.contains(&w)
                    && let (Some(wi), Some(vl)) =
                        (indices.get(&w).copied(), lowlink.get(&v).copied())
                {
                    lowlink.insert(v, vl.min(wi));
                }
            }
        }

        if lowlink.get(&v) == indices.get(&v) {
            let mut component = HashSet::new();
            loop {
                let w = stack.pop().expect("stack non-empty for SCC root");
                on_stack.remove(&w);
                component.insert(w);
                if w == v {
                    break;
                }
            }
            components.push(component);
        }
    }

    for &node in &nodes {
        if !indices.contains_key(&node) {
            strongconnect(
                node,
                state,
                &mut index_counter,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut components,
            );
        }
    }

    components
}

/// Compute the weakly-connected components of a [`SnapshotState`].
///
/// Two nodes are weakly connected if there is an undirected path between them
/// (ignoring edge direction). Uses union-find over the union of outgoing and
/// incoming edges. Returns the number of weakly connected components (W),
/// used in the cyclomatic number formula β₁ = E − V + W.
fn count_weakly_connected_components(state: &SnapshotState) -> usize {
    if state.outgoing.is_empty() {
        return 0;
    }

    // Union-find over all node IDs.
    let mut parent: HashMap<i64, i64> = HashMap::new();
    fn find(parent: &mut HashMap<i64, i64>, x: i64) -> i64 {
        let mut root = x;
        while parent.get(&root).copied().unwrap_or(root) != root {
            root = parent[&root];
        }
        // Path compression.
        let mut curr = x;
        while parent.get(&curr).copied().unwrap_or(curr) != root {
            let next = parent[&curr];
            parent.insert(curr, root);
            curr = next;
        }
        root
    }
    fn union(parent: &mut HashMap<i64, i64>, a: i64, b: i64) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    // Initialize every node as its own root.
    for &node in state.outgoing.keys() {
        parent.entry(node).or_insert(node);
    }

    // Union along every directed edge (treated as undirected).
    for (&src, dsts) in &state.outgoing {
        for &dst in dsts {
            parent.entry(dst).or_insert(dst);
            union(&mut parent, src, dst);
        }
    }

    // Count distinct roots.
    let roots: AHashSet<i64> = state
        .outgoing
        .keys()
        .map(|&n| find(&mut parent, n))
        .collect();
    roots.len()
}

/// Compute the cyclomatic number (β₁) of a [`SnapshotState`].
///
/// β₁ = E − V + W, where E is the number of directed edges, V is the number
/// of nodes, and W is the number of weakly connected components. This counts
/// the number of independent cycles in the underlying undirected graph — the
/// "excess connectivity" beyond a spanning tree.
///
/// **Note:** β₁ counts undirected cycles. A directed cycle requires mutual
/// reachability (a non-trivial SCC of size ≥ 2), which is a stricter
/// condition. For code-graph analysis, pair this with
/// [`cycle_scc_barcode`] to distinguish "structural redundancy" (β₁) from
/// "actual circular dependencies" (non-trivial SCCs).
pub fn cycle_rank_snapshot(state: &SnapshotState) -> usize {
    let v = state.node_count() as i64;
    let e = state.edge_count() as i64;
    let w = count_weakly_connected_components(state) as i64;
    // β₁ = E − V + W. Use signed arithmetic: E − V can be negative for
    // forests (more nodes than edges), but the full expression is always ≥ 0
    // for a valid graph (it is the rank of the cycle space).
    let beta1 = e - v + w;
    beta1.max(0) as usize
}

// ════════════════════════════════════════════════════════════════════════════
// Sweep (scalar trajectory)
// ════════════════════════════════════════════════════════════════════════════

/// Sweep across a sequence of [`VersionedSnapshot`]s, computing the SCC
/// landscape and cycle-rank at each version.
///
/// The `versions` must be ordered by increasing version number (as produced by
/// [`SnapshotManager::versions`](crate::mvcc::SnapshotManager::versions)).
///
/// Returns one [`TemporalPersistencePoint`] per version.
pub fn temporal_persistence_sweep(versions: &[VersionedSnapshot]) -> Vec<TemporalPersistencePoint> {
    versions
        .iter()
        .map(|vs| {
            let sccs = strongly_connected_components_snapshot(&vs.state);
            let active: usize = sccs.iter().map(|c| c.len()).sum();
            let n_components = sccs.len();
            let largest_size = sccs.iter().map(|c| c.len()).max().unwrap_or(0);
            let fraction_largest = if active == 0 {
                0.0
            } else {
                largest_size as f32 / active as f32
            };
            let cycle_rank = cycle_rank_snapshot(&vs.state);
            let nontrivial: Vec<&HashSet<i64>> = sccs.iter().filter(|c| c.len() >= 2).collect();
            let n_nontrivial_sccs = nontrivial.len();
            let largest_nontrivial_size = nontrivial.iter().map(|c| c.len()).max().unwrap_or(0);

            TemporalPersistencePoint {
                version: vs.version,
                active,
                n_components,
                largest_size,
                fraction_largest,
                cycle_rank,
                n_nontrivial_sccs,
                largest_nontrivial_size,
            }
        })
        .collect()
}

// ════════════════════════════════════════════════════════════════════════════
// Approximate barcode (LIFO — retained for backwards compat, deprecated)
// ════════════════════════════════════════════════════════════════════════════

/// Derive a persistence barcode from a sweep using a LIFO count-delta policy.
///
/// **Deprecated.** This is the original geographdb port. It tracks the
/// component *count* across versions and uses a LIFO (last-in-first-out)
/// closure policy: when the count drops, the most-recently-born bars die.
/// This is unreliable when multiple components merge or split simultaneously.
///
/// For exact results use [`scc_lineage_barcode`] (identity-based H₀) or
/// [`cycle_scc_barcode`] (circular-dependency lifecycle).
pub fn compute_temporal_barcode(points: &[TemporalPersistencePoint]) -> Vec<TemporalBarcode> {
    let mut open: Vec<(u64, usize)> = Vec::new();
    let mut bars: Vec<TemporalBarcode> = Vec::new();

    let mut prev_count = 0usize;

    for pt in points {
        let curr = pt.n_components;

        if curr > prev_count {
            for _ in 0..(curr - prev_count) {
                open.push((pt.version, pt.largest_size));
            }
        }

        for ob in &mut open {
            ob.1 = ob.1.max(pt.largest_size);
        }

        if curr < prev_count {
            let died = prev_count - curr;
            let close_from = open.len().saturating_sub(died);
            for (birth, peak) in open.drain(close_from..) {
                bars.push(TemporalBarcode {
                    birth_version: birth,
                    death_version: Some(pt.version),
                    peak_size: peak,
                });
            }
        }

        prev_count = curr;
    }

    for (birth, peak) in open {
        bars.push(TemporalBarcode {
            birth_version: birth,
            death_version: None,
            peak_size: peak,
        });
    }

    bars.sort_by_key(|b| b.birth_version);
    bars
}

// ════════════════════════════════════════════════════════════════════════════
// Exact lineage-tracked barcodes (identity-based matching)
// ════════════════════════════════════════════════════════════════════════════

/// Jaccard similarity between two sets: |intersection| / |union|.
fn jaccard(a: &HashSet<i64>, b: &HashSet<i64>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let union_size = a.union(b).count();
    if union_size == 0 {
        return 0.0;
    }
    let inter_size = a.intersection(b).count();
    inter_size as f64 / union_size as f64
}

/// Greedily match SCCs across two adjacent versions by maximum Jaccard overlap.
///
/// Returns a mapping from indices in `next` to indices in `prev` (the matched
/// predecessor). An unmatched `next` index means the SCC was newly born; an
/// unmatched `prev` index means the lineage died.
///
/// Both slices are filtered to the relevant size range (caller's responsibility).
fn match_sccs(prev: &[HashSet<i64>], next: &[HashSet<i64>]) -> Vec<Option<usize>> {
    // (jaccard, prev_idx, next_idx) for all pairs with overlap > 0.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, s_prev) in prev.iter().enumerate() {
        for (j, s_next) in next.iter().enumerate() {
            let sim = jaccard(s_prev, s_next);
            if sim > 0.0 {
                pairs.push((sim, i, j));
            }
        }
    }
    // Greedy: highest-overlap pairs assigned first.
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut prev_used = vec![false; prev.len()];
    let mut next_match: Vec<Option<usize>> = vec![None; next.len()];

    for (_, pi, ni) in pairs {
        if !prev_used[pi] && next_match[ni].is_none() {
            next_match[ni] = Some(pi);
            prev_used[pi] = true;
        }
    }
    next_match
}

/// Core lineage-tracking engine shared by [`scc_lineage_barcode`] and
/// [`cycle_scc_barcode`].
///
/// For each version, computes SCCs, filters to those with `min_size` or more
/// members, then tracks lineages by Jaccard-overlap matching across adjacent
/// versions. A lineage is born when an SCC appears with no membership-overlapping
/// predecessor; it dies when no successor shares its members.
fn lineage_barcode(versions: &[VersionedSnapshot], min_size: usize) -> Vec<LineageBarcode> {
    if versions.is_empty() {
        return Vec::new();
    }

    // Precompute filtered SCCs per version: Vec<(version, Vec<HashSet>)>.
    let sccs_per_version: Vec<(u64, Vec<HashSet<i64>>)> = versions
        .iter()
        .map(|vs| {
            let filtered = strongly_connected_components_snapshot(&vs.state)
                .into_iter()
                .filter(|c| c.len() >= min_size)
                .collect();
            (vs.version, filtered)
        })
        .collect();

    // Active lineages: each tracks (birth_version, birth_size, peak_size,
    // final_size, versions_seen, current_member_set).
    struct Lineage {
        birth_version: u64,
        birth_size: usize,
        peak_size: usize,
        final_size: usize,
        versions_seen: usize,
        members: HashSet<i64>,
    }

    let mut active: Vec<Lineage> = Vec::new();
    let mut bars: Vec<LineageBarcode> = Vec::new();

    for (i, &(version, ref sccs)) in sccs_per_version.iter().enumerate() {
        if i == 0 {
            // First version: every SCC is a birth.
            for scc in sccs {
                active.push(Lineage {
                    birth_version: version,
                    birth_size: scc.len(),
                    peak_size: scc.len(),
                    final_size: scc.len(),
                    versions_seen: 1,
                    members: scc.clone(),
                });
            }
            continue;
        }

        // Match current SCCs against the previous version's lineages.
        let prev_members: Vec<&HashSet<i64>> = active.iter().map(|l| &l.members).collect();
        let prev_owned: Vec<HashSet<i64>> = prev_members.into_iter().cloned().collect();
        let next_match = match_sccs(&prev_owned, sccs);

        // Update matched lineages; close unmatched ones (deaths).
        let mut still_active = Vec::with_capacity(active.len());
        for (li, lineage) in active.drain(..).enumerate() {
            // Did any current SCC match this lineage?
            let matched_next = next_match.iter().position(|&m| m == Some(li));
            if let Some(ni) = matched_next {
                let scc = &sccs[ni];
                still_active.push(Lineage {
                    birth_version: lineage.birth_version,
                    birth_size: lineage.birth_size,
                    peak_size: lineage.peak_size.max(scc.len()),
                    final_size: scc.len(),
                    versions_seen: lineage.versions_seen + 1,
                    members: scc.clone(),
                });
            } else {
                // Lineage died at this version.
                bars.push(LineageBarcode {
                    birth_version: lineage.birth_version,
                    death_version: Some(version),
                    birth_size: lineage.birth_size,
                    peak_size: lineage.peak_size,
                    final_size: lineage.final_size,
                    versions_seen: lineage.versions_seen,
                });
            }
        }

        // Newly born SCCs: those with no predecessor match.
        for (ni, match_opt) in next_match.iter().enumerate() {
            if match_opt.is_none() {
                still_active.push(Lineage {
                    birth_version: version,
                    birth_size: sccs[ni].len(),
                    peak_size: sccs[ni].len(),
                    final_size: sccs[ni].len(),
                    versions_seen: 1,
                    members: sccs[ni].clone(),
                });
            }
        }

        active = still_active;
    }

    // Close remaining active lineages (survived to end of sweep).
    for lineage in active {
        bars.push(LineageBarcode {
            birth_version: lineage.birth_version,
            death_version: None,
            birth_size: lineage.birth_size,
            peak_size: lineage.peak_size,
            final_size: lineage.final_size,
            versions_seen: lineage.versions_seen,
        });
    }

    bars.sort_by_key(|b| (b.birth_version, b.peak_size));
    bars
}

/// Exact H₀ component barcode via stable membership-identity matching.
///
/// Tracks ALL strongly-connected components (trivial and non-trivial) across
/// the version chain. Two SCCs at adjacent versions are linked if they share
/// members (Jaccard overlap > 0). Each component gets an exact
/// `(birth_version, death_version)` bar — no LIFO approximation.
///
/// This replaces the deprecated [`compute_temporal_barcode`] for H₀ analysis.
pub fn scc_lineage_barcode(versions: &[VersionedSnapshot]) -> Vec<LineageBarcode> {
    lineage_barcode(versions, 1)
}

/// Circular-dependency lifecycle barcode via non-trivial SCC tracking.
///
/// Tracks only non-trivial SCCs (size ≥ 2 — those containing at least one
/// directed cycle). For a call graph, each such SCC is a circular dependency.
/// The barcode shows when circular deps are born (a cycle forms), how large
/// they grow (peak_size), and when they die (a cycle is broken).
///
/// **Note:** this is SCC-based cycle detection, not classical H₁ persistent
/// homology. It detects cycle *presence* and tracks cycle-containing
/// components, but does not track individual cycle generators (independent
/// cycles within a single SCC). See the module-level doc for the distinction.
pub fn cycle_scc_barcode(versions: &[VersionedSnapshot]) -> Vec<LineageBarcode> {
    lineage_barcode(versions, 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn state_from(outgoing: &[(&i64, &[i64])]) -> SnapshotState {
        let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut inc: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut all_nodes: HashSet<i64> = HashSet::new();
        for &(src, dsts) in outgoing {
            all_nodes.insert(*src);
            for &d in dsts {
                out.entry(*src).or_default().push(d);
                inc.entry(d).or_default().push(*src);
                all_nodes.insert(d);
            }
            out.entry(*src).or_default();
        }
        // Ensure EVERY node is a key in outgoing — matches real cache behavior
        // where even destination-only nodes get an empty adjacency entry.
        for &n in &all_nodes {
            out.entry(n).or_default();
        }
        SnapshotState::new(&out, &inc)
    }

    fn vs(version: u64, outgoing: &[(&i64, &[i64])]) -> VersionedSnapshot {
        VersionedSnapshot {
            version,
            created_at: std::time::SystemTime::now(),
            state: std::sync::Arc::new(state_from(outgoing)),
        }
    }

    // ── strongly_connected_components_snapshot ─────────────────────────────

    #[test]
    fn scc_empty_state_has_zero_components() {
        let state = SnapshotState::new(&HashMap::new(), &HashMap::new());
        assert!(strongly_connected_components_snapshot(&state).is_empty());
    }

    #[test]
    fn scc_isolated_nodes_are_all_trivial() {
        let state = state_from(&[(&1, &[]), (&2, &[]), (&3, &[])]);
        let sccs = strongly_connected_components_snapshot(&state);
        assert_eq!(sccs.len(), 3);
        for c in &sccs {
            assert_eq!(c.len(), 1);
        }
    }

    #[test]
    fn scc_directed_cycle_is_one_component() {
        let state = state_from(&[(&1, &[2]), (&2, &[3]), (&3, &[1])]);
        let sccs = strongly_connected_components_snapshot(&state);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 3);
    }

    #[test]
    fn scc_dag_all_trivial() {
        let state = state_from(&[(&1, &[2]), (&2, &[3])]);
        let sccs = strongly_connected_components_snapshot(&state);
        assert_eq!(sccs.len(), 3);
    }

    // ── cycle_rank_snapshot (β₁) ───────────────────────────────────────────

    #[test]
    fn cycle_rank_empty_is_zero() {
        let state = SnapshotState::new(&HashMap::new(), &HashMap::new());
        assert_eq!(cycle_rank_snapshot(&state), 0);
    }

    #[test]
    fn cycle_rank_dag_is_zero() {
        // 1 → 2 → 3: tree (0 cycles). E=2, V=3, W=1 → β₁ = 2-3+1 = 0.
        let state = state_from(&[(&1, &[2]), (&2, &[3])]);
        assert_eq!(cycle_rank_snapshot(&state), 0);
    }

    #[test]
    fn cycle_rank_single_cycle_is_one() {
        // 1 → 2 → 3 → 1: E=3, V=3, W=1 → β₁ = 3-3+1 = 1.
        let state = state_from(&[(&1, &[2]), (&2, &[3]), (&3, &[1])]);
        assert_eq!(cycle_rank_snapshot(&state), 1);
    }

    #[test]
    fn cycle_rank_two_disconnected_cycles_is_two() {
        // 1→2→1 and 3→4→3: E=4, V=4, W=2 → β₁ = 4-4+2 = 2.
        let state = state_from(&[(&1, &[2]), (&2, &[1]), (&3, &[4]), (&4, &[3])]);
        assert_eq!(cycle_rank_snapshot(&state), 2);
    }

    // ── temporal_persistence_sweep (extended) ──────────────────────────────

    #[test]
    fn sweep_tracks_cycle_metrics() {
        let versions = [
            vs(1, &[(&1, &[]), (&2, &[]), (&3, &[])]),    // 3 isolated
            vs(2, &[(&1, &[2]), (&2, &[3]), (&3, &[1])]), // one 3-cycle
        ];
        let pts = temporal_persistence_sweep(&versions);
        assert_eq!(pts.len(), 2);
        // v1: no cycles.
        assert_eq!(pts[0].cycle_rank, 0);
        assert_eq!(pts[0].n_nontrivial_sccs, 0);
        // v2: one non-trivial SCC (the cycle), β₁ = 1.
        assert_eq!(pts[1].cycle_rank, 1);
        assert_eq!(pts[1].n_nontrivial_sccs, 1);
        assert_eq!(pts[1].largest_nontrivial_size, 3);
    }

    // ── scc_lineage_barcode (exact H₀) ─────────────────────────────────────

    #[test]
    fn lineage_barcode_stable_component_survives() {
        // Same graph at v1 and v2 → one bar, no death.
        let versions = [
            vs(1, &[(&1, &[2]), (&2, &[1])]),
            vs(2, &[(&1, &[2]), (&2, &[1])]),
        ];
        let bars = scc_lineage_barcode(&versions);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].birth_version, 1);
        assert_eq!(bars[0].death_version, None);
        assert_eq!(bars[0].versions_seen, 2);
    }

    #[test]
    fn lineage_barcode_birth_and_death_exact() {
        // v1: 3 isolated nodes → 3 trivial SCCs (3 bars born).
        // v2: same → all 3 survive.
        // v3: empty → all 3 die.
        let versions = [
            vs(1, &[(&1, &[]), (&2, &[]), (&3, &[])]),
            vs(2, &[(&1, &[]), (&2, &[]), (&3, &[])]),
            vs(3, &[]),
        ];
        let bars = scc_lineage_barcode(&versions);
        // 3 bars, all born at v1, all die at v3.
        assert_eq!(bars.len(), 3);
        for b in &bars {
            assert_eq!(b.birth_version, 1);
            assert_eq!(b.death_version, Some(3));
            assert_eq!(b.versions_seen, 2); // seen at v1 and v2, not v3
        }
    }

    #[test]
    fn lineage_barcode_merges_tracked_by_identity() {
        // v1: {1,2} cycle + {3,4} cycle → 2 non-trivial SCCs.
        // v2: {1,2,3,4} all in one cycle → 1 SCC. One lineage survives,
        //     the other dies (merge).
        let versions = [
            vs(1, &[(&1, &[2]), (&2, &[1]), (&3, &[4]), (&4, &[3])]),
            vs(2, &[(&1, &[2]), (&2, &[3]), (&3, &[4]), (&4, &[1])]),
        ];
        let bars = scc_lineage_barcode(&versions);
        // All SCCs (trivial + non-trivial): v1 has 4 nodes, all in 2 SCCs.
        // But with min_size=1, every node is its own SCC if not in a cycle...
        // Actually v1 has 2 SCCs of size 2 each → 2 bars.
        // v2: 1 SCC of size 4 → the {1,2} SCC at v1 overlaps with the size-4 SCC,
        //     so it continues; {3,4} also overlaps, but greedy matching picks one.
        //     One survives (peak grows to 4), one dies (merged away).
        let survived = bars.iter().filter(|b| b.death_version.is_none()).count();
        let died = bars.iter().filter(|b| b.death_version.is_some()).count();
        assert_eq!(survived, 1);
        assert_eq!(died, 1);
        // The surviving bar grew to size 4.
        let survivor = bars.iter().find(|b| b.death_version.is_none()).unwrap();
        assert_eq!(survivor.peak_size, 4);
    }

    // ── cycle_scc_barcode (circular-dependency lifecycle) ──────────────────

    #[test]
    fn cycle_barcode_no_cycles_no_bars() {
        // DAG at both versions → no non-trivial SCCs → no bars.
        let versions = [
            vs(1, &[(&1, &[2]), (&2, &[3])]),
            vs(2, &[(&1, &[2]), (&2, &[3])]),
        ];
        let bars = cycle_scc_barcode(&versions);
        assert!(bars.is_empty(), "no cycles → no bars");
    }

    #[test]
    fn cycle_barcode_born_then_broken() {
        // v1: DAG (no cycle).
        // v2: 1→2→1 cycle forms.
        // v3: DAG again (cycle broken).
        let versions = [
            vs(1, &[(&1, &[2])]),
            vs(2, &[(&1, &[2]), (&2, &[1])]),
            vs(3, &[(&1, &[2])]),
        ];
        let bars = cycle_scc_barcode(&versions);
        // One circular dependency: born at v2, dies at v3.
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].birth_version, 2);
        assert_eq!(bars[0].death_version, Some(3));
        assert_eq!(bars[0].peak_size, 2);
        assert_eq!(bars[0].birth_size, 2);
    }

    #[test]
    fn cycle_barcode_grows_then_shrinks() {
        // v1: 2-node cycle {1,2}.
        // v2: grows to 3-node cycle {1,2,3}.
        // v3: back to 2-node cycle {1,2} (node 3 leaves).
        let versions = [
            vs(1, &[(&1, &[2]), (&2, &[1])]),
            vs(2, &[(&1, &[2]), (&2, &[3]), (&3, &[1])]),
            vs(3, &[(&1, &[2]), (&2, &[1])]),
        ];
        let bars = cycle_scc_barcode(&versions);
        // One lineage: born at v1, peaks at size 3 (v2), survives to v3.
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].birth_version, 1);
        assert_eq!(bars[0].death_version, None);
        assert_eq!(bars[0].peak_size, 3);
        assert_eq!(bars[0].final_size, 2);
    }

    #[test]
    fn cycle_barcode_two_independent_cycles() {
        // v1: {1,2} and {3,4} cycles — two independent circular deps.
        // v2: both survive.
        let versions = [
            vs(1, &[(&1, &[2]), (&2, &[1]), (&3, &[4]), (&4, &[3])]),
            vs(2, &[(&1, &[2]), (&2, &[1]), (&3, &[4]), (&4, &[3])]),
        ];
        let bars = cycle_scc_barcode(&versions);
        assert_eq!(bars.len(), 2);
        assert!(bars.iter().all(|b| b.versions_seen == 2));
        assert!(bars.iter().all(|b| b.death_version.is_none()));
    }

    // ── compute_temporal_barcode (deprecated, retained) ────────────────────

    #[test]
    fn deprecated_barcode_basic() {
        let pts = temporal_persistence_sweep(&[
            vs(1, &[(&1, &[]), (&2, &[]), (&3, &[])]),
            vs(2, &[(&1, &[2]), (&2, &[3]), (&3, &[1])]),
        ]);
        let bars = compute_temporal_barcode(&pts);
        // v1: 3 components → 3 births. v2: 1 component → 2 deaths.
        // 1 survives.
        assert!(!bars.is_empty());
    }

    // ── jaccard ────────────────────────────────────────────────────────────

    #[test]
    fn jaccard_identical_sets_is_one() {
        let a: HashSet<i64> = [1, 2, 3].into_iter().collect();
        let b: HashSet<i64> = [1, 2, 3].into_iter().collect();
        assert!((jaccard(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn jaccard_disjoint_is_zero() {
        let a: HashSet<i64> = [1, 2].into_iter().collect();
        let b: HashSet<i64> = [3, 4].into_iter().collect();
        assert!(jaccard(&a, &b).abs() < 1e-9);
    }

    #[test]
    fn jaccard_half_overlap() {
        let a: HashSet<i64> = [1, 2].into_iter().collect();
        let b: HashSet<i64> = [2, 3].into_iter().collect();
        // |{2}| / |{1,2,3}| = 1/3
        assert!((jaccard(&a, &b) - 1.0 / 3.0).abs() < 1e-9);
    }
}
