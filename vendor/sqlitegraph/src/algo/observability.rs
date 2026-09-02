//! Observability algorithms for runtime analysis and impact estimation.
//!
//! This module provides algorithms for analyzing runtime behavior and estimating
//! the impact radius of changes or failures in graph-structured systems.
//!
//! # Available Algorithms
//!
//! ## Runtime Event Analysis
//!
//! - [`happens_before_analysis`] - Analyze trace events for concurrent memory access pairs
//! - [`VectorClock`] - Partial order data structure for event ordering
//! - [`HappensBeforeResult`] - Result with concurrent pairs and statistics
//! - [`TraceEvent`] - Runtime event representation (Read/Write operations)
//!
//! ## Impact Analysis
//!
//! - [`impact_radius`] - Compute blast zone using bounded reachability with edge weights
//! - [`impact_radius_with_progress`] - Impact radius with progress tracking
//! - [`ImpactRadiusConfig`] - Configuration for impact radius (max_distance, max_hops, weight_fn)
//! - [`ImpactRadiusResult`] - Result with blast_zone, distances, boundary, size
//! - [`WeightCallback`] - Weight callback type for edge-weighted analysis
//! - Default weight function (`|_, _, _| 1.0`) - Returns 1.0 for all edges (hop count)
//!
//! # When to Use Happens-Before Analysis
//!
//! ## Race Detection
//! - **Data race detection** - Identify unsynchronized concurrent memory accesses
//! - **Lock validation** - Verify synchronization primitives are used correctly
//! - **Memory model testing** - Validate concurrent program behavior
//!
//! ## Event Ordering
//! - **Causal ordering** - Determine if one event must precede another
//! - **Concurrency detection** - Find events that are not causally related
//! - **Trace debugging** - Understand execution order in concurrent systems
//!
//! # Algorithm
//!
//! Vector clocks implement a partial order on events in distributed/concurrent systems:
//!
//! 1. **Vector Clock Comparison** - For events A and B:
//!    - A happens-before B if A's clock <= B's clock element-wise, with at least one <
//!    - A is concurrent with B if neither happens-before the other
//!    - A == B if all clock elements are equal
//!
//! 2. **Clock Operations**:
//!    - `increment(thread_id)` - Increment clock for current thread
//!    - `merge(other)` - Take element-wise max with another clock (after synchronization)
//!    - `happens_before(other)` - Check if this clock precedes other
//!    - `is_concurrent(other)` - Check if clocks are unrelated
//!
//! 3. **Race Detection**:
//!    - Group events by memory location
//!    - For each location, compare vector clocks of all access pairs
//!    - Concurrent accesses to same location = potential data race
//!
//! # Complexity
//!
//! - **Time**: O(E * L) where E = events, L = accesses per location
//! - **Space**: O(E) for storing events grouped by location
//!
//! Where:
//! - E = number of trace events
//! - L = maximum number of accesses to any single location
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlitegraph::algo::observability::{
//!     happens_before_analysis, TraceEvent, Operation, VectorClock,
//! };
//!
//! // Create trace events from concurrent execution
//! let mut events = vec![
//!     // Thread 1 writes to location 100
//!     TraceEvent {
//!         event_id: 1,
//!         thread_id: 1,
//!         operation: Operation::Write,
//!         memory_location: 100,
//!         vector_clock: VectorClock::new().incremented(1),
//!     },
//!     // Thread 2 writes to location 100 (concurrent!)
//!     TraceEvent {
//!         event_id: 2,
//!         thread_id: 2,
//!         operation: Operation::Write,
//!         memory_location: 100,
//!         vector_clock: VectorClock::new().incremented(2),
//!     },
//! ];
//!
//! let result = happens_before_analysis(&events)?;
//!
//! println!("Detected {} potential data races", result.concurrent_pairs.len());
//! for (event_a, event_b) in &result.concurrent_pairs {
//!     println!("  Race: thread {} vs thread {} on location {}",
//!         event_a.thread_id, event_b.thread_id, event_a.memory_location);
//! }
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHashSet};
use serde_json::Value;

use crate::errors::SqliteGraphError;
use crate::graph::SqliteGraph;
use crate::progress::ProgressCallback;

/// Memory operation type in trace events.
///
/// Represents whether a trace event is a read or write operation.
/// Data races occur when at least one operation is a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    /// Read operation on a memory location.
    Read,
    /// Write operation on a memory location.
    Write,
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operation::Read => write!(f, "R"),
            Operation::Write => write!(f, "W"),
        }
    }
}

/// Trace event from concurrent program execution.
///
/// Represents a single memory access event in a concurrent execution trace.
/// Each event has a vector clock that captures its causal relationship
/// to other events.
///
/// # Fields
///
/// - `event_id` - Unique identifier for this event
/// - `thread_id` - Thread that performed this operation
/// - `operation` - Type of memory access (Read or Write)
/// - `memory_location` - Address or identifier of accessed memory
/// - `vector_clock` - Partial order information for this event
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEvent {
    /// Unique identifier for this event in the trace.
    pub event_id: i64,

    /// Thread that performed this memory operation.
    pub thread_id: i64,

    /// Type of memory operation (read or write).
    pub operation: Operation,

    /// Memory location being accessed.
    /// Can be a physical address or symbolic identifier.
    pub memory_location: i64,

    /// Vector clock capturing causal ordering information.
    pub vector_clock: VectorClock,
}

impl TraceEvent {
    /// Create a new trace event.
    ///
    /// # Arguments
    ///
    /// * `event_id` - Unique event identifier
    /// * `thread_id` - Thread performing the operation
    /// * `operation` - Read or Write
    /// * `memory_location` - Location being accessed
    /// * `vector_clock` - Cusal ordering information
    pub fn new(
        event_id: i64,
        thread_id: i64,
        operation: Operation,
        memory_location: i64,
        vector_clock: VectorClock,
    ) -> Self {
        Self {
            event_id,
            thread_id,
            operation,
            memory_location,
            vector_clock,
        }
    }

    /// Create a trace event with a fresh vector clock for a single thread.
    ///
    /// Convenience function that creates a vector clock with a single
    /// thread's clock set to 1.
    ///
    /// # Arguments
    ///
    /// * `event_id` - Unique event identifier
    /// * `thread_id` - Thread performing the operation
    /// * `operation` - Read or Write
    /// * `memory_location` - Location being accessed
    pub fn with_thread(
        event_id: i64,
        thread_id: i64,
        operation: Operation,
        memory_location: i64,
    ) -> Self {
        Self {
            event_id,
            thread_id,
            operation,
            memory_location,
            vector_clock: VectorClock::new().incremented(thread_id),
        }
    }
}

impl Hash for TraceEvent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.event_id.hash(state);
        self.thread_id.hash(state);
        self.operation.hash(state);
        self.memory_location.hash(state);
        // Note: vector_clock is not hashed as it's not Eq for comparison
    }
}

/// Vector clock for happens-before analysis.
///
/// A vector clock is a mapping from thread IDs to logical timestamps.
/// It implements a partial order on events in concurrent/distributed systems.
///
/// The happens-before relation is defined as:
/// - `A <= B` if for all threads t: clock_A[t] <= clock_B[t]
/// - `A < B` if `A <= B` and exists t where clock_A[t] < clock_B[t]
/// - `A` concurrent with `B` if neither `A <= B` nor `B <= A`
///
/// # Fields
///
/// - `clocks` - HashMap mapping thread_id -> logical timestamp
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorClock {
    /// Thread ID to logical timestamp mapping.
    clocks: AHashMap<i64, u64>,
}

impl VectorClock {
    /// Create a new empty vector clock.
    ///
    /// All threads start with implicit timestamp 0.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::observability::VectorClock;
    ///
    /// let vc = VectorClock::new();
    /// assert!(vc.is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            clocks: AHashMap::new(),
        }
    }

    /// Check if the vector clock is empty (no threads recorded).
    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty()
    }

    /// Get the clock value for a specific thread.
    ///
    /// Returns 0 if the thread is not in the clock (implicit value).
    pub fn get(&self, thread_id: i64) -> u64 {
        *self.clocks.get(&thread_id).unwrap_or(&0)
    }

    /// Increment the clock for a specific thread.
    ///
    /// Returns self for method chaining.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread whose clock should be incremented
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::observability::VectorClock;
    ///
    /// let mut vc = VectorClock::new();
    /// vc.increment(1);
    /// assert_eq!(vc.get(1), 1);
    /// vc.increment(1);
    /// assert_eq!(vc.get(1), 2);
    /// ```
    pub fn increment(&mut self, thread_id: i64) {
        *self.clocks.entry(thread_id).or_insert(0) += 1;
    }

    /// Return a new vector clock with the given thread incremented.
    ///
    /// Convenience method for creating incremented copies without mutation.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread whose clock should be incremented
    pub fn incremented(mut self, thread_id: i64) -> Self {
        self.increment(thread_id);
        self
    }

    /// Merge another vector clock into this one.
    ///
    /// Takes the element-wise maximum of both clocks.
    /// Used after thread synchronization (e.g., after acquiring a lock).
    ///
    /// # Arguments
    ///
    /// * `other` - Vector clock to merge with
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::observability::VectorClock;
    ///
    /// let mut vc1 = VectorClock::new().incremented(1);
    /// let vc2 = VectorClock::new().incremented(2);
    ///
    /// vc1.merge(&vc2);
    /// assert_eq!(vc1.get(1), 1);
    /// assert_eq!(vc1.get(2), 1);
    /// ```
    pub fn merge(&mut self, other: &VectorClock) {
        for (&thread_id, &their_clock) in &other.clocks {
            let my_clock = self.clocks.entry(thread_id).or_insert(0);
            *my_clock = (*my_clock).max(their_clock);
        }
    }

    /// Check if this vector clock happens-before another.
    ///
    /// Returns `true` if for all threads t: self[t] <= other[t],
    /// and there exists at least one thread where self[t] < other[t].
    ///
    /// This is the strict partial order: this event MUST complete
    /// before the other event can occur.
    ///
    /// # Arguments
    ///
    /// * `other` - Vector clock to compare against
    ///
    /// # Returns
    ///
    /// `true` if this clock happens-before the other, `false` otherwise
    /// (including when clocks are equal or concurrent).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::observability::VectorClock;
    ///
    /// let vc1 = VectorClock::new().incremented(1);
    /// let mut vc2 = VectorClock::new().incremented(1);
    /// vc2.increment(1);  // Thread 1: vc1 = 1, vc2 = 2
    ///
    /// assert!(vc1.happens_before(&vc2));
    /// assert!(!vc2.happens_before(&vc1));
    /// ```
    pub fn happens_before(&self, other: &VectorClock) -> bool {
        // Need at least one thread where self < other
        let mut found_strictly_less = false;

        // All threads in self must be <= in other
        for (&thread_id, &my_clock) in &self.clocks {
            let their_clock = other.get(thread_id);
            if my_clock > their_clock {
                return false; // self > other for this thread
            }
            if my_clock < their_clock {
                found_strictly_less = true;
            }
        }

        // Check threads only in other
        for (&thread_id, &their_clock) in &other.clocks {
            if !self.clocks.contains_key(&thread_id) {
                // self implicitly has 0 for this thread
                if 0 < their_clock {
                    found_strictly_less = true;
                }
            }
        }

        found_strictly_less
    }

    /// Check if this vector clock is concurrent with another.
    ///
    /// Two clocks are concurrent if neither happens-before the other.
    /// This means there exist threads t1, t2 such that:
    /// - self[t1] > other[t1]
    /// - self[t2] < other[t2]
    ///
    /// Concurrent events may execute in either order (potential race).
    ///
    /// # Arguments
    ///
    /// * `other` - Vector clock to compare against
    ///
    /// # Returns
    ///
    /// `true` if clocks are concurrent (neither happens-before the other).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlitegraph::algo::observability::VectorClock;
    ///
    /// let vc1 = VectorClock::new().incremented(1);  // Thread 1: 1, Thread 2: 0
    /// let vc2 = VectorClock::new().incremented(2);  // Thread 1: 0, Thread 2: 1
    ///
    /// assert!(vc1.is_concurrent(&vc2));
    /// ```
    pub fn is_concurrent(&self, other: &VectorClock) -> bool {
        // Concurrent if neither happens-before the other
        !self.happens_before(other) && !other.happens_before(self)
    }

    /// Get all thread IDs with non-zero clocks.
    pub fn threads(&self) -> impl Iterator<Item = i64> + '_ {
        self.clocks.keys().copied()
    }

    /// Get the number of threads with non-zero clocks.
    pub fn len(&self) -> usize {
        self.clocks.len()
    }
}

impl Default for VectorClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of happens-before analysis on trace events.
///
/// Contains pairs of events that are concurrent (potential data races)
/// and summary statistics about the analysis.
///
/// # Fields
///
/// - `concurrent_pairs` - Pairs of events accessing the same location concurrently
/// - `total_events` - Total number of events analyzed
/// - `conflicts_detected` - Number of concurrent pairs (potential data races)
#[derive(Debug, Clone)]
pub struct HappensBeforeResult {
    /// Pairs of events that access the same memory location concurrently.
    /// Each pair represents a potential data race.
    pub concurrent_pairs: Vec<(TraceEvent, TraceEvent)>,

    /// Total number of events in the trace.
    pub total_events: usize,

    /// Number of concurrent pairs detected (potential data races).
    pub conflicts_detected: usize,
}

impl HappensBeforeResult {
    /// Create a new happens-before result.
    fn new(concurrent_pairs: Vec<(TraceEvent, TraceEvent)>, total_events: usize) -> Self {
        let conflicts_detected = concurrent_pairs.len();
        Self {
            concurrent_pairs,
            total_events,
            conflicts_detected,
        }
    }

    /// Check if any potential data races were detected.
    pub fn has_races(&self) -> bool {
        !self.concurrent_pairs.is_empty()
    }

    /// Get the number of unique memory locations with races.
    pub fn raced_locations(&self) -> AHashSet<i64> {
        let mut locations = AHashSet::new();
        for (event_a, _) in &self.concurrent_pairs {
            locations.insert(event_a.memory_location);
        }
        locations
    }
}

/// Analyze trace events for concurrent memory access pairs (race detection).
///
/// Performs happens-before analysis using vector clocks to identify potential
/// data races. Events accessing the same memory location with concurrent
/// vector clocks represent potential races.
///
/// # Arguments
///
/// * `events` - Slice of trace events from concurrent execution
///
/// # Returns
///
/// `Ok(HappensBeforeResult)` containing:
/// - `concurrent_pairs` - Event pairs with concurrent access to same location
/// - `total_events` - Number of events analyzed
/// - `conflicts_detected` - Number of concurrent pairs
///
/// # Algorithm
///
/// 1. Group events by memory location
/// 2. For each location, compare all pairs of memory accesses
/// 3. If vector clocks are concurrent, add to result
/// 4. Return result with statistics
///
/// # Complexity
///
/// - **Time**: O(E * L^2) where E = events, L = max accesses per location
/// - **Space**: O(E) for storing events grouped by location
///
/// # Edge Cases
///
/// - **Empty trace**: Returns empty result with total_events = 0
/// - **Single thread**: No concurrent events (all ordered by thread clock)
/// - **Read-only pairs**: Not reported as races (only writes cause races)
/// - **Same event**: Never compared with itself
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::observability::{
///     happens_before_analysis, TraceEvent, Operation,
/// };
///
/// // Create trace with concurrent writes to same location
/// let events = vec![
///     TraceEvent::with_thread(1, 1, Operation::Write, 100),
///     TraceEvent::with_thread(2, 2, Operation::Write, 100),
/// ];
///
/// let result = happens_before_analysis(&events)?;
///
/// assert!(result.has_races());
/// assert_eq!(result.conflicts_detected, 1);
/// ```
pub fn happens_before_analysis(
    events: &[TraceEvent],
) -> Result<HappensBeforeResult, SqliteGraphError> {
    let total_events = events.len();

    // Group events by memory location
    let mut by_location: AHashMap<i64, Vec<&TraceEvent>> = AHashMap::new();
    for event in events {
        by_location
            .entry(event.memory_location)
            .or_default()
            .push(event);
    }

    // Find concurrent pairs at each location
    let mut concurrent_pairs = Vec::new();

    for events_at_location in by_location.values() {
        // Compare all pairs at this location
        for (i, event_a) in events_at_location.iter().enumerate() {
            for event_b in events_at_location.iter().skip(i + 1) {
                // Check if concurrent (neither happens-before the other)
                if event_a.vector_clock.is_concurrent(&event_b.vector_clock) {
                    // Only report if at least one is a write (real race potential)
                    if event_a.operation == Operation::Write
                        || event_b.operation == Operation::Write
                    {
                        concurrent_pairs.push(((*event_a).clone(), (*event_b).clone()));
                    }
                }
            }
        }
    }

    Ok(HappensBeforeResult::new(concurrent_pairs, total_events))
}

/// Weight callback type for edge weighting in impact radius computation.
///
/// Given a source node, target node, and edge data, returns the weight
/// of that edge. Weights must be finite (not NaN or infinity).
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::observability::WeightCallback;
/// use serde_json::json;
///
/// let weight_fn: &WeightCallback = &|from, to, edge_data| {
///     edge_data
///         .get("distance")
///         .and_then(|v| v.as_f64())
///         .unwrap_or(1.0)
/// };
/// ```
pub type WeightCallback = dyn Fn(i64, i64, &Value) -> f64;

/// Default weight function that returns 1.0 for all edges.
///
/// Use this for unweighted graphs where edge weights don't matter.
/// The impact radius will be computed in terms of hop count.
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::observability::{impact_radius, ImpactRadiusConfig}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build unweighted graph ...
///
/// let config = ImpactRadiusConfig {
///     max_distance: 3.0,
///     max_hops: None,
///     weight_fn: &|_, _, _| 1.0,
/// };
///
/// let result = impact_radius(&graph, source, &config)?;
/// println!("Blast zone: {} nodes within 3 hops", result.size);
/// ```
/// Configuration for impact radius computation.
///
/// Controls the blast zone computation by specifying distance limits,
/// hop limits, and the weight function for edge weighting.
///
/// # Fields
///
/// - `max_distance` - Maximum weighted distance from source (blast radius limit)
/// - `max_hops` - Optional hop count limit (None = unlimited)
/// - `weight_fn` - Callback to extract edge weight from (from, to, edge_data)
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::algo::observability::{impact_radius, ImpactRadiusConfig};
///
/// // Unweighted: 3 hops
/// let config = ImpactRadiusConfig {
///     max_distance: 3.0,
///     max_hops: None,
///     weight_fn: &|_, _, _| 1.0,
/// };
///
/// // Weighted with hop limit: 10.0 distance OR 5 hops
/// let config = ImpactRadiusConfig {
///     max_distance: 10.0,
///     max_hops: Some(5),
///     weight_fn: &|_from, _to, edge_data| {
///         edge_data.get("cost").and_then(|v| v.as_f64()).unwrap_or(1.0)
///     },
/// };
/// ```
#[derive(Clone)]
pub struct ImpactRadiusConfig<'a> {
    /// Maximum weighted distance from source node.
    /// Nodes beyond this distance are excluded from the blast zone.
    pub max_distance: f64,

    /// Optional maximum number of hops from source.
    /// When set, stops traversal after this many edges even if within distance.
    /// Use None to allow unlimited hops.
    pub max_hops: Option<usize>,

    /// Callback to extract edge weight.
    /// Called for each edge during traversal.
    pub weight_fn: &'a WeightCallback,
}

/// Result of impact radius computation.
///
/// Contains the blast zone (nodes within max_distance), their distances
/// from the source, the boundary nodes (at exactly max_distance), and summary
/// statistics.
///
/// # Fields
///
/// - `blast_zone` - Set of nodes within max_distance from source
/// - `distances` - Distance from source to each node in blast zone
/// - `boundary` - Nodes at exactly max_distance (boundary of blast zone)
/// - `size` - Total number of nodes in blast zone
///
/// # Example
///
/// ```rust,ignore
/// # use sqlitegraph::algo::observability::ImpactRadiusResult;
/// # let result: ImpactRadiusResult = unimplemented!();
/// println!("Blast zone size: {}", result.size);
/// println!("Boundary nodes: {:?}", result.boundary);
///
/// // Check if a node is affected
/// if result.blast_zone.contains(&node_id) {
///     let distance = result.distances.get(&node_id).unwrap();
///     println!("Node {} is at distance {}", node_id, distance);
/// }
///
/// // Check if a node is on the boundary
/// if result.boundary.contains(&node_id) {
///     println!("Node {} is at the blast zone boundary", node_id);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ImpactRadiusResult {
    /// Nodes within max_distance from source (the blast zone).
    pub blast_zone: AHashSet<i64>,

    /// Distance from source to each node in blast zone.
    pub distances: AHashMap<i64, f64>,

    /// Nodes at exactly max_distance (boundary of blast zone).
    pub boundary: AHashSet<i64>,

    /// Total number of nodes in blast zone.
    pub size: usize,
}

impl ImpactRadiusResult {
    /// Check if a node is within the blast zone.
    ///
    /// Returns true if the node is within max_distance from the source.
    pub fn is_affected(&self, node: i64) -> bool {
        self.blast_zone.contains(&node)
    }

    /// Get the distance from source to a node.
    ///
    /// Returns None if the node is not in the blast zone.
    pub fn distance_to(&self, node: i64) -> Option<f64> {
        self.distances.get(&node).copied()
    }

    /// Check if a node is on the boundary of the blast zone.
    ///
    /// Returns true if the node is at exactly max_distance from source.
    pub fn is_boundary(&self, node: i64) -> bool {
        self.boundary.contains(&node)
    }
}

/// Computes the impact radius from a source node using bounded weighted BFS.
///
/// Impact radius computes the "blast zone" - all nodes within a specified
/// weighted distance from a source node. This is useful for:
///
/// - **Failure impact analysis**: What could be affected if this node fails?
/// - **Change propagation**: What might need testing after this change?
/// - **Security blast zone**: What's the maximum potential damage radius?
/// - **Network lateral movement**: How far could an attacker propagate?
///
/// # Arguments
///
/// * `graph` - The graph to analyze
/// * `source` - The source node ID (center of blast zone)
/// * `config` - Configuration for radius computation
///
/// # Returns
///
/// `Ok(ImpactRadiusResult)` containing:
/// - `blast_zone` - All nodes within max_distance
/// - `distances` - Distance from source to each node
/// - `boundary` - Nodes at exactly max_distance
/// - `size` - Total nodes in blast zone
///
/// # Algorithm
///
/// Bounded breadth-first search with distance tracking:
/// 1. Initialize: distances[source] = 0.0, queue = [(source, 0 hops)]
/// 2. While queue not empty:
///    - Pop (node, hops)
///    - Skip if dist > max_distance (early termination)
///    - Skip if max_hops Some() and hops >= max_hops
///    - Add node to blast_zone
///    - For each neighbor via fetch_outgoing:
///      - Compute weight via weight_fn
///      - Validate weight.is_finite()
///      - new_dist = dist + weight
///      - If new_dist <= max_distance AND (not visited OR shorter path):
///        - Update distances, enqueue (neighbor, hops + 1)
/// 3. Extract boundary: nodes where |dist - max_distance| < epsilon
/// 4. Return result
///
/// # Complexity
///
/// - **Time**: O(|V| + |E|) within the blast zone
/// - **Space**: O(|V|) for distances and blast zone
///
/// # Edge Cases
///
/// - **Empty graph**: Returns {source} with distance 0.0
/// - **Source not in graph**: Returns {source} with distance 0.0
/// - **Disconnected**: Only reaches nodes in source's component
/// - **Zero max_distance**: Returns {source} only
/// - **Invalid weights**: Returns error if weight_fn returns non-finite value
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{SqliteGraph, algo::observability::{impact_radius, ImpactRadiusConfig, default_weight_fn}};
///
/// let graph = SqliteGraph::open_in_memory()?;
/// // ... build graph ...
///
/// // Find all nodes within 3 hops of source
/// let config = ImpactRadiusConfig {
///     max_distance: 3.0,
///     max_hops: None,
///     weight_fn: &|_, _, _| 1.0,
/// };
///
/// let result = impact_radius(&graph, source, &config)?;
///
/// println!("Blast zone: {} nodes", result.size);
/// for &node in &result.blast_zone {
///     let dist = result.distances[&node];
///     println!("  Node {}: distance {:.2}", node, dist);
/// }
///
/// println!("Boundary: {:?}", result.boundary);
/// ```
pub fn impact_radius(
    graph: &SqliteGraph,
    source: i64,
    config: &ImpactRadiusConfig,
) -> Result<ImpactRadiusResult, SqliteGraphError> {
    let max_distance = config.max_distance;
    let max_hops = config.max_hops;
    let weight_fn = config.weight_fn;

    // Initialize distances and BFS queue
    let mut distances: AHashMap<i64, f64> = AHashMap::new();
    let mut blast_zone: AHashSet<i64> = AHashSet::new();
    let mut queue: VecDeque<(i64, f64, usize)> = VecDeque::new();

    // Source node is always in blast zone with distance 0
    distances.insert(source, 0.0);
    queue.push_back((source, 0.0, 0));

    // BFS with distance tracking
    while let Some((node, dist, hops)) = queue.pop_front() {
        // Early termination: already beyond max distance
        if dist > max_distance {
            continue;
        }

        // Hop limit check
        if max_hops.is_some_and(|limit| hops >= limit) {
            // Still process this node, but don't enqueue more
        }

        // Add node to blast zone
        blast_zone.insert(node);

        // Get outgoing neighbors
        let outgoing = graph.fetch_outgoing(node)?;

        for neighbor in outgoing {
            // Compute edge weight
            let edge_data = &serde_json::json!({});
            let weight = weight_fn(node, neighbor, edge_data);

            // Validate weight
            if !weight.is_finite() {
                return Err(SqliteGraphError::invalid_input(format!(
                    "Invalid weight for edge {} -> {}: weight must be finite, got {}",
                    node, neighbor, weight
                )));
            }

            let new_dist = dist + weight;

            // Check distance bound before enqueuing (early termination)
            if new_dist > max_distance {
                continue;
            }

            // Check hop limit before enqueuing
            if max_hops.is_some_and(|limit| hops + 1 > limit) {
                continue;
            }

            // Relax edge: update if not visited or found shorter path
            let should_enqueue = match distances.get(&neighbor) {
                Some(&old_dist) => new_dist < old_dist,
                None => true,
            };

            if should_enqueue {
                distances.insert(neighbor, new_dist);
                queue.push_back((neighbor, new_dist, hops + 1));
            }
        }
    }

    // Ensure source is in blast zone even if graph is empty
    blast_zone.insert(source);
    distances.entry(source).or_insert(0.0);

    // Extract boundary: nodes at exactly max_distance (within epsilon)
    let epsilon = 1e-9;
    let boundary: AHashSet<i64> = distances
        .iter()
        .filter(|(_, dist)| (*dist - max_distance).abs() < epsilon)
        .map(|(&node, _)| node)
        .collect();

    let size = blast_zone.len();

    Ok(ImpactRadiusResult {
        blast_zone,
        distances,
        boundary,
        size,
    })
}

/// Computes the impact radius with progress tracking.
///
/// Same algorithm as [`impact_radius`] but reports progress during execution.
/// Suitable for long-running computations on large graphs.
///
/// # Progress Reporting
///
/// The callback receives:
/// - `current`: Current number of nodes visited
/// - `total`: None (unknown total for BFS)
/// - `message`: "Impact radius: visited {current}, blast_zone {size}"
///
/// Progress is reported every ~10 nodes to avoid excessive callback overhead.
///
/// # Arguments
///
/// * `graph` - The graph to analyze
/// * `source` - The source node ID
/// * `config` - Configuration for radius computation
/// * `progress` - Progress callback for reporting execution status
///
/// # Example
///
/// ```rust,ignore
/// use sqlitegraph::{
///     algo::observability::{impact_radius_with_progress, ImpactRadiusConfig},
///     progress::ConsoleProgress
/// };
///
/// let config = ImpactRadiusConfig {
///     max_distance: 10.0,
///     max_hops: None,
///     weight_fn: &|_, _, _| 1.0,
/// };
///
/// let progress = ConsoleProgress::new();
/// let result = impact_radius_with_progress(&graph, source, &config, &progress)?;
/// // Output: Impact radius: visited 10, blast_zone 10
/// // Output: Impact radius: visited 20, blast_zone 18
/// ```
pub fn impact_radius_with_progress<F>(
    graph: &SqliteGraph,
    source: i64,
    config: &ImpactRadiusConfig,
    progress: &F,
) -> Result<ImpactRadiusResult, SqliteGraphError>
where
    F: ProgressCallback,
{
    let max_distance = config.max_distance;
    let max_hops = config.max_hops;
    let weight_fn = config.weight_fn;

    // Initialize distances and BFS queue
    let mut distances: AHashMap<i64, f64> = AHashMap::new();
    let mut blast_zone: AHashSet<i64> = AHashSet::new();
    let mut queue: VecDeque<(i64, f64, usize)> = VecDeque::new();
    let mut nodes_visited = 0;

    // Source node is always in blast zone with distance 0
    distances.insert(source, 0.0);
    queue.push_back((source, 0.0, 0));

    // BFS with distance tracking
    while let Some((node, dist, hops)) = queue.pop_front() {
        nodes_visited += 1;

        // Report progress every 10 nodes
        if nodes_visited % 10 == 0 {
            progress.on_progress(
                nodes_visited,
                None,
                &format!(
                    "Impact radius: visited {}, blast_zone {}",
                    nodes_visited,
                    blast_zone.len()
                ),
            );
        }

        // Early termination: already beyond max distance
        if dist > max_distance {
            continue;
        }

        // Hop limit check
        if max_hops.is_some_and(|limit| hops >= limit) {
            // Still process this node, but don't enqueue more
        }

        // Add node to blast zone
        blast_zone.insert(node);

        // Get outgoing neighbors
        let outgoing = graph.fetch_outgoing(node)?;

        for neighbor in outgoing {
            // Compute edge weight
            let edge_data = &serde_json::json!({});
            let weight = weight_fn(node, neighbor, edge_data);

            // Validate weight
            if !weight.is_finite() {
                progress.on_error(&std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Invalid weight for edge {} -> {}: {}",
                        node, neighbor, weight
                    ),
                ));
                return Err(SqliteGraphError::invalid_input(format!(
                    "Invalid weight for edge {} -> {}: weight must be finite, got {}",
                    node, neighbor, weight
                )));
            }

            let new_dist = dist + weight;

            // Check distance bound before enqueuing (early termination)
            if new_dist > max_distance {
                continue;
            }

            // Check hop limit before enqueuing
            if max_hops.is_some_and(|limit| hops + 1 > limit) {
                continue;
            }

            // Relax edge: update if not visited or found shorter path
            let should_enqueue = match distances.get(&neighbor) {
                Some(&old_dist) => new_dist < old_dist,
                None => true,
            };

            if should_enqueue {
                distances.insert(neighbor, new_dist);
                queue.push_back((neighbor, new_dist, hops + 1));
            }
        }
    }

    // Report completion
    progress.on_complete();

    // Ensure source is in blast zone even if graph is empty
    blast_zone.insert(source);
    distances.entry(source).or_insert(0.0);

    // Extract boundary: nodes at exactly max_distance (within epsilon)
    let epsilon = 1e-9;
    let boundary: AHashSet<i64> = distances
        .iter()
        .filter(|(_, dist)| (*dist - max_distance).abs() < epsilon)
        .map(|(&node, _)| node)
        .collect();

    let size = blast_zone.len();

    Ok(ImpactRadiusResult {
        blast_zone,
        distances,
        boundary,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_weight_fn(_from: i64, _to: i64, _edge_data: &Value) -> f64 {
        1.0
    }

    // Helper: Create a simple trace event
    fn make_event(
        event_id: i64,
        thread_id: i64,
        operation: Operation,
        memory_location: i64,
        vc_clocks: Vec<(i64, u64)>,
    ) -> TraceEvent {
        let mut vc = VectorClock::new();
        for (tid, clock) in vc_clocks {
            vc.clocks.insert(tid, clock);
        }
        TraceEvent::new(event_id, thread_id, operation, memory_location, vc)
    }

    // Vector Clock Tests

    #[test]
    fn test_vector_clock_new() {
        // Scenario: New vector clock is empty
        // Expected: No clocks set, is_empty returns true
        let vc = VectorClock::new();
        assert!(vc.is_empty(), "New vector clock should be empty");
        assert_eq!(vc.len(), 0, "Length should be 0");
        assert_eq!(vc.get(1), 0, "Missing thread should return 0");
    }

    #[test]
    fn test_vector_clock_increment() {
        // Scenario: Increment thread's clock
        // Expected: Clock value increases by 1
        let mut vc = VectorClock::new();
        vc.increment(1);
        assert_eq!(vc.get(1), 1, "First increment should set to 1");
        assert!(!vc.is_empty(), "Should not be empty after increment");

        vc.increment(1);
        assert_eq!(vc.get(1), 2, "Second increment should set to 2");

        vc.increment(2);
        assert_eq!(vc.get(1), 2, "Thread 1 should still be 2");
        assert_eq!(vc.get(2), 1, "Thread 2 should be 1");
    }

    #[test]
    fn test_vector_clock_incremented() {
        // Scenario: Create incremented copy
        // Expected: Returns new clock with thread incremented
        let vc1 = VectorClock::new().incremented(5);
        assert_eq!(vc1.get(5), 1, "Thread 5 should be 1");
        assert_eq!(vc1.get(1), 0, "Thread 1 should be 0");
    }

    #[test]
    fn test_vector_clock_happens_before_simple() {
        // Scenario: Simple happens-before relationship
        // Clock A: {1: 1}, Clock B: {1: 2}
        // Expected: A happens-before B (A < B for thread 1)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);

        let mut vc_b = VectorClock::new();
        vc_b.increment(1);
        vc_b.increment(1);

        assert!(vc_a.happens_before(&vc_b), "A should happen-before B");
        assert!(!vc_b.happens_before(&vc_a), "B should not happen-before A");
    }

    #[test]
    fn test_vector_clock_happens_before_partial_order() {
        // Scenario: Partial order across multiple threads
        // Clock A: {1: 1, 2: 0}, Clock B: {1: 1, 2: 1}
        // Expected: A happens-before B (B has progressed on thread 2)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);

        let mut vc_b = VectorClock::new();
        vc_b.increment(1);
        vc_b.increment(2);

        assert!(
            vc_a.happens_before(&vc_b),
            "A should happen-before B (progressed on thread 2)"
        );
        assert!(!vc_b.happens_before(&vc_a), "B should not happen-before A");
    }

    #[test]
    fn test_vector_clock_happens_before_equal() {
        // Scenario: Equal vector clocks
        // Expected: Neither happens-before the other (need strict < for at least one)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);

        let mut vc_b = VectorClock::new();
        vc_b.increment(1);

        assert!(
            !vc_a.happens_before(&vc_b),
            "Equal clocks should not satisfy happens-before (need strict <)"
        );
        assert!(
            !vc_b.happens_before(&vc_a),
            "Equal clocks should not satisfy happens-before (need strict <)"
        );
    }

    #[test]
    fn test_vector_clock_happens_before_empty() {
        // Scenario: Empty clock compared to non-empty
        // Expected: Empty clock happens-before non-empty (0 <= n for all)
        let vc_empty = VectorClock::new();
        let vc_nonempty = VectorClock::new().incremented(1);

        assert!(
            vc_empty.happens_before(&vc_nonempty),
            "Empty clock should happen-before non-empty"
        );
        assert!(
            !vc_nonempty.happens_before(&vc_empty),
            "Non-empty should not happen-before empty"
        );
    }

    #[test]
    fn test_vector_clock_is_concurrent() {
        // Scenario: Concurrent clocks
        // Clock A: {1: 1}, Clock B: {2: 1}
        // Expected: Concurrent (neither <= the other)
        let vc_a = VectorClock::new().incremented(1);
        let vc_b = VectorClock::new().incremented(2);

        assert!(vc_a.is_concurrent(&vc_b), "Clocks should be concurrent");
        assert!(vc_b.is_concurrent(&vc_a), "Concurrency should be symmetric");
    }

    #[test]
    fn test_vector_clock_is_concurrent_complex() {
        // Scenario: Complex concurrency
        // Clock A: {1: 2, 2: 1}, Clock B: {1: 1, 2: 2}
        // Expected: Concurrent (A > B on thread 1, A < B on thread 2)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);
        vc_a.increment(1);
        vc_a.increment(2);

        let mut vc_b = VectorClock::new();
        vc_b.increment(1);
        vc_b.increment(2);
        vc_b.increment(2);

        assert!(
            vc_a.is_concurrent(&vc_b),
            "Should be concurrent (different ordering per thread)"
        );
    }

    #[test]
    fn test_vector_clock_is_concurrent_ordered() {
        // Scenario: Ordered clocks are not concurrent
        // Clock A: {1: 1}, Clock B: {1: 2}
        // Expected: Not concurrent (A happens-before B)
        let vc_a = VectorClock::new().incremented(1);
        let mut vc_b = VectorClock::new();
        vc_b.increment(1);
        vc_b.increment(1);

        assert!(
            !vc_a.is_concurrent(&vc_b),
            "Ordered clocks should not be concurrent"
        );
        assert!(
            !vc_b.is_concurrent(&vc_a),
            "Ordered clocks should not be concurrent"
        );
    }

    #[test]
    fn test_vector_clock_merge() {
        // Scenario: Merge two clocks
        // Clock A: {1: 1, 2: 0}, Clock B: {1: 0, 2: 1}
        // Expected: A after merge = {1: 1, 2: 1} (element-wise max)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);

        let vc_b = VectorClock::new().incremented(2);

        vc_a.merge(&vc_b);

        assert_eq!(vc_a.get(1), 1, "Thread 1 should be max(1, 0) = 1");
        assert_eq!(vc_a.get(2), 1, "Thread 2 should be max(0, 1) = 1");
    }

    #[test]
    fn test_vector_clock_merge_existing() {
        // Scenario: Merge updates existing values
        // Clock A: {1: 1}, Clock B: {1: 3}
        // Expected: A after merge = {1: 3} (max)
        let mut vc_a = VectorClock::new();
        vc_a.increment(1);

        let mut vc_b = VectorClock::new();
        vc_b.increment(1);
        vc_b.increment(1);
        vc_b.increment(1);

        vc_a.merge(&vc_b);

        assert_eq!(vc_a.get(1), 3, "Thread 1 should be max(1, 3) = 3");
    }

    #[test]
    fn test_vector_clock_merge_empty_into_nonempty() {
        // Scenario: Merge empty clock into non-empty
        // Expected: Non-empty clock unchanged
        let mut vc = VectorClock::new();
        vc.increment(1);
        let original = vc.clone();

        vc.merge(&VectorClock::new());

        assert_eq!(vc.get(1), original.get(1), "Clock should be unchanged");
    }

    #[test]
    fn test_vector_clock_merge_empty_into_empty() {
        // Scenario: Merge empty clock into empty clock
        // Expected: Both remain empty
        let mut vc = VectorClock::new();
        vc.merge(&VectorClock::new());
        assert!(vc.is_empty());
    }

    // Trace Event Tests

    #[test]
    fn test_trace_event_new() {
        // Scenario: Create trace event
        // Expected: All fields set correctly
        let vc = VectorClock::new().incremented(1);
        let event = TraceEvent::new(10, 5, Operation::Read, 100, vc.clone());

        assert_eq!(event.event_id, 10);
        assert_eq!(event.thread_id, 5);
        assert_eq!(event.operation, Operation::Read);
        assert_eq!(event.memory_location, 100);
        assert_eq!(event.vector_clock.get(1), 1);
    }

    #[test]
    fn test_trace_event_with_thread() {
        // Scenario: Create trace event with convenience method
        // Expected: Vector clock automatically set
        let event = TraceEvent::with_thread(1, 5, Operation::Write, 100);

        assert_eq!(event.event_id, 1);
        assert_eq!(event.thread_id, 5);
        assert_eq!(event.operation, Operation::Write);
        assert_eq!(event.memory_location, 100);
        assert_eq!(event.vector_clock.get(5), 1);
    }

    // Happens-Before Analysis Tests

    #[test]
    fn test_happens_before_empty() {
        // Scenario: Empty trace
        // Expected: Empty result, no conflicts
        let events: Vec<TraceEvent> = vec![];
        let result =
            happens_before_analysis(&events).expect("Analysis should succeed on empty trace");

        assert_eq!(result.total_events, 0);
        assert_eq!(result.conflicts_detected, 0);
        assert!(result.concurrent_pairs.is_empty());
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_single_event() {
        // Scenario: Single event trace
        // Expected: No pairs to compare, no conflicts
        let events = vec![TraceEvent::with_thread(1, 1, Operation::Write, 100)];
        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.total_events, 1);
        assert_eq!(result.conflicts_detected, 0);
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_single_thread() {
        // Scenario: All events from same thread (ordered by clock)
        // Expected: No concurrent events (single thread is totally ordered)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            make_event(2, 1, Operation::Read, 100, vec![(1, 2)]),
            make_event(3, 1, Operation::Write, 100, vec![(1, 3)]),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.total_events, 3);
        assert_eq!(result.conflicts_detected, 0);
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_concurrent_writes() {
        // Scenario: Two threads write to same location concurrently
        // Expected: One concurrent pair detected
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 100),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.total_events, 2);
        assert_eq!(result.conflicts_detected, 1);
        assert!(result.has_races());

        let (event_a, event_b) = &result.concurrent_pairs[0];
        assert_eq!(event_a.thread_id, 1);
        assert_eq!(event_b.thread_id, 2);
        assert_eq!(event_a.memory_location, 100);
        assert_eq!(event_b.memory_location, 100);
    }

    #[test]
    fn test_happens_before_read_write_conflict() {
        // Scenario: Concurrent read and write to same location
        // Expected: One concurrent pair detected (read-write race)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Read, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 100),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.conflicts_detected, 1);
        assert!(result.has_races());
    }

    #[test]
    fn test_happens_before_read_only_no_race() {
        // Scenario: Two threads read same location concurrently
        // Expected: No race detected (read-only is safe)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Read, 100),
            TraceEvent::with_thread(2, 2, Operation::Read, 100),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.conflicts_detected, 0);
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_ordered_events() {
        // Scenario: Events from same thread are ordered
        // Thread 1: event 1 then event 2 (clock: 1 then 2)
        // Expected: No concurrent pairs (happens-before relationship)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            make_event(2, 1, Operation::Write, 100, vec![(1, 2)]),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.conflicts_detected, 0);
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_different_locations() {
        // Scenario: Concurrent writes to different locations
        // Expected: No race detected (different memory locations)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 200),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.conflicts_detected, 0);
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_multiple_locations() {
        // Scenario: Multiple memory locations with mixed access patterns
        // Location 100: concurrent writes (race)
        // Location 200: same thread (no race)
        // Location 300: read-only (no race)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 100),
            TraceEvent::with_thread(1, 1, Operation::Write, 200),
            make_event(4, 1, Operation::Write, 200, vec![(1, 2)]),
            TraceEvent::with_thread(2, 2, Operation::Read, 300),
            make_event(6, 1, Operation::Read, 300, vec![(1, 1)]),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(result.total_events, 6);
        assert_eq!(
            result.conflicts_detected, 1,
            "Should detect 1 race at location 100"
        );
        assert!(result.has_races());

        let raced = result.raced_locations();
        assert!(raced.contains(&100));
        assert!(!raced.contains(&200));
        assert!(!raced.contains(&300));
    }

    #[test]
    fn test_happens_before_synchronized_threads() {
        // Scenario: Synchronized threads (vector clocks merged)
        // Thread 1 writes, syncs, thread 2 writes
        // Expected: No race (happens-before due to synchronization)
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            // After sync: both threads know about each other
            make_event(2, 1, Operation::Write, 100, vec![(1, 2), (2, 1)]),
            make_event(3, 2, Operation::Write, 100, vec![(1, 2), (2, 2)]),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        assert_eq!(
            result.conflicts_detected, 0,
            "Synchronized access should not race"
        );
        assert!(!result.has_races());
    }

    #[test]
    fn test_happens_before_three_threads() {
        // Scenario: Three threads accessing same location
        // Expected: Multiple concurrent pairs detected
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 100),
            TraceEvent::with_thread(3, 3, Operation::Write, 100),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        // All three threads are concurrent with each other
        // Pairs: (1,2), (1,3), (2,3) = 3 pairs
        assert_eq!(result.conflicts_detected, 3);
        assert!(result.has_races());
    }

    #[test]
    fn test_happens_before_result_raced_locations() {
        // Scenario: Multiple locations with races
        // Expected: raced_locations returns unique locations
        let events = vec![
            TraceEvent::with_thread(1, 1, Operation::Write, 100),
            TraceEvent::with_thread(2, 2, Operation::Write, 100),
            TraceEvent::with_thread(1, 1, Operation::Write, 200),
            TraceEvent::with_thread(2, 2, Operation::Write, 200),
            TraceEvent::with_thread(1, 1, Operation::Write, 300),
            TraceEvent::with_thread(2, 2, Operation::Write, 300),
        ];

        let result = happens_before_analysis(&events).expect("Analysis should succeed");

        let locations = result.raced_locations();
        assert_eq!(locations.len(), 3);
        assert!(locations.contains(&100));
        assert!(locations.contains(&200));
        assert!(locations.contains(&300));
    }

    #[test]
    fn test_operation_display() {
        // Scenario: Display operation types
        // Expected: Read displays as "R", Write as "W"
        assert_eq!(format!("{}", Operation::Read), "R");
        assert_eq!(format!("{}", Operation::Write), "W");
    }

    // Impact Radius Tests

    /// Helper: Create a linear chain graph for impact radius tests
    /// Creates chain: 0 -> 1 -> 2 -> 3 -> 4
    fn create_impact_chain() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        for i in 0..5 {
            let entity = crate::GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Create chain
        for i in 0..entity_ids.len().saturating_sub(1) {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[i],
                to_id: entity_ids[i + 1],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create a diamond graph for impact radius tests
    /// 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
    fn create_impact_diamond() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        for i in 0..4 {
            let entity = crate::GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
        for (from_idx, to_idx) in edges {
            let edge = crate::GraphEdge {
                id: 0,
                from_id: entity_ids[from_idx],
                to_id: entity_ids[to_idx],
                edge_type: "next".to_string(),
                data: serde_json::json!({}),
            };
            graph.insert_edge(&edge).expect("Failed to insert edge");
        }

        graph
    }

    /// Helper: Create disconnected components for impact radius tests
    /// Component 1: 0 -> 1, Component 2: 2 -> 3
    fn create_impact_disconnected() -> SqliteGraph {
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        for i in 0..4 {
            let entity = crate::GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({"index": i}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Component 1: 0 -> 1
        let edge1 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge1).expect("Failed to insert edge");

        // Component 2: 2 -> 3
        let edge2 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[2],
            to_id: entity_ids[3],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge2).expect("Failed to insert edge");

        graph
    }

    #[test]
    fn test_impact_radius_empty() {
        // Scenario: Empty graph
        // Expected: Returns {source} with distance 0
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        let config = ImpactRadiusConfig {
            max_distance: 10.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result = impact_radius(&graph, 999, &config)
            .expect("Impact radius should succeed on empty graph");

        assert_eq!(result.size, 1, "Empty graph should have blast zone size 1");
        assert!(
            result.blast_zone.contains(&999),
            "Source should be in blast zone"
        );
        assert_eq!(
            *result.distances.get(&999).unwrap(),
            0.0,
            "Source distance should be 0"
        );
        assert!(
            result.boundary.is_empty(),
            "Empty graph should have no boundary nodes"
        );
    }

    #[test]
    fn test_impact_radius_linear_chain() {
        // Scenario: Linear chain 0 -> 1 -> 2 -> 3 -> 4 with max_distance = 2.0
        // Expected: Nodes 0, 1, 2 in blast zone (within 2 hops)
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 2.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.size, 3, "Should have 3 nodes within 2 hops");
        assert!(
            result.blast_zone.contains(&entity_ids[0]),
            "Node 0 should be in blast zone"
        );
        assert!(
            result.blast_zone.contains(&entity_ids[1]),
            "Node 1 should be in blast zone"
        );
        assert!(
            result.blast_zone.contains(&entity_ids[2]),
            "Node 2 should be in blast zone"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[3]),
            "Node 3 should NOT be in blast zone"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[4]),
            "Node 4 should NOT be in blast zone"
        );

        // Check distances
        assert_eq!(
            *result.distances.get(&entity_ids[0]).unwrap(),
            0.0,
            "Node 0 distance = 0"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[1]).unwrap(),
            1.0,
            "Node 1 distance = 1"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[2]).unwrap(),
            2.0,
            "Node 2 distance = 2"
        );
    }

    #[test]
    fn test_impact_radius_boundary_detection() {
        // Scenario: Linear chain with max_distance = 2.0
        // Expected: Node 2 is at the boundary (exactly max_distance)
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 2.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.boundary.len(), 1, "Should have 1 boundary node");
        assert!(
            result.boundary.contains(&entity_ids[2]),
            "Node 2 should be on boundary"
        );
        assert!(
            !result.boundary.contains(&entity_ids[0]),
            "Node 0 should NOT be on boundary"
        );
        assert!(
            !result.boundary.contains(&entity_ids[1]),
            "Node 1 should NOT be on boundary"
        );
    }

    #[test]
    fn test_impact_radius_max_hops() {
        // Scenario: Linear chain with max_distance = 10.0 but max_hops = 2
        // Expected: Only 3 nodes (0, 1, 2) due to hop limit
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 10.0,
            max_hops: Some(2),
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.size, 3, "Should have 3 nodes due to hop limit");
        assert!(
            result.blast_zone.contains(&entity_ids[0]),
            "Node 0 should be in blast zone"
        );
        assert!(
            result.blast_zone.contains(&entity_ids[1]),
            "Node 1 should be in blast zone"
        );
        assert!(
            result.blast_zone.contains(&entity_ids[2]),
            "Node 2 should be in blast zone"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[3]),
            "Node 3 should NOT be in blast zone"
        );
    }

    #[test]
    fn test_impact_radius_weight_extraction() {
        // Scenario: Custom weight_fn extracts weights
        // Expected: Custom weights are used for distance calculation
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Custom weight function: weight 2.0 per edge
        let custom_weight_fn = |_from: i64, _to: i64, _edge_data: &Value| -> f64 { 2.0 };

        let config = ImpactRadiusConfig {
            max_distance: 4.0,
            max_hops: None,
            weight_fn: &custom_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        // With weight 2.0 per edge:
        // Node 0: dist 0
        // Node 1: dist 2
        // Node 2: dist 4 (boundary)
        // Node 3: dist 6 (excluded)
        assert_eq!(result.size, 3, "Should have 3 nodes with custom weights");
        assert_eq!(
            *result.distances.get(&entity_ids[1]).unwrap(),
            2.0,
            "Node 1 distance = 2"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[2]).unwrap(),
            4.0,
            "Node 2 distance = 4"
        );
    }

    #[test]
    fn test_impact_radius_unweighted() {
        // Scenario: default_weight_fn uses 1.0 (hop count)
        // Expected: Distance equals hop count
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 5.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        // With default weight 1.0, distance = hop count
        for (i, entity_id) in entity_ids.iter().enumerate().take(5) {
            let expected_dist = i as f64;
            let actual_dist = result.distances.get(entity_id).copied().unwrap_or(999.0);
            assert_eq!(
                actual_dist, expected_dist,
                "Node {} should have distance {}",
                i, expected_dist
            );
        }
    }

    #[test]
    fn test_impact_radius_disconnected() {
        // Scenario: Disconnected components, start in component 1
        // Expected: Only reaches nodes in source's component
        let graph = create_impact_disconnected();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 10.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        // Start from node 0 (component 1: 0 -> 1)
        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.size, 2, "Should only reach component 1 (2 nodes)");
        assert!(
            result.blast_zone.contains(&entity_ids[0]),
            "Node 0 should be in blast zone"
        );
        assert!(
            result.blast_zone.contains(&entity_ids[1]),
            "Node 1 should be in blast zone"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[2]),
            "Node 2 should NOT be in blast zone (different component)"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[3]),
            "Node 3 should NOT be in blast zone (different component)"
        );
    }

    #[test]
    fn test_impact_radius_diamond() {
        // Scenario: Diamond graph 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        // Expected: Reaches all nodes from source 0
        let graph = create_impact_diamond();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 10.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.size, 4, "Should reach all 4 nodes");
        for &id in &entity_ids {
            assert!(
                result.blast_zone.contains(&id),
                "Node {} should be in blast zone",
                id
            );
        }

        // Check distances
        assert_eq!(
            *result.distances.get(&entity_ids[0]).unwrap(),
            0.0,
            "Node 0 distance = 0"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[1]).unwrap(),
            1.0,
            "Node 1 distance = 1"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[2]).unwrap(),
            1.0,
            "Node 2 distance = 1"
        );
        assert_eq!(
            *result.distances.get(&entity_ids[3]).unwrap(),
            2.0,
            "Node 3 distance = 2"
        );
    }

    #[test]
    fn test_impact_radius_result_is_affected() {
        // Scenario: Check is_affected() helper method
        // Expected: Correctly reports affected nodes
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 2.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert!(
            result.is_affected(entity_ids[0]),
            "Node 0 should be affected"
        );
        assert!(
            result.is_affected(entity_ids[1]),
            "Node 1 should be affected"
        );
        assert!(
            result.is_affected(entity_ids[2]),
            "Node 2 should be affected"
        );
        assert!(
            !result.is_affected(entity_ids[3]),
            "Node 3 should NOT be affected"
        );
        assert!(
            !result.is_affected(entity_ids[4]),
            "Node 4 should NOT be affected"
        );
    }

    #[test]
    fn test_impact_radius_result_distance_to() {
        // Scenario: Check distance_to() helper method
        // Expected: Returns distance or None for non-affected nodes
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 2.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(
            result.distance_to(entity_ids[0]),
            Some(0.0),
            "Node 0 distance"
        );
        assert_eq!(
            result.distance_to(entity_ids[1]),
            Some(1.0),
            "Node 1 distance"
        );
        assert_eq!(
            result.distance_to(entity_ids[2]),
            Some(2.0),
            "Node 2 distance"
        );
        assert_eq!(
            result.distance_to(entity_ids[3]),
            None,
            "Node 3 should return None"
        );
        assert_eq!(
            result.distance_to(999),
            None,
            "Non-existent node should return None"
        );
    }

    #[test]
    fn test_impact_radius_result_is_boundary() {
        // Scenario: Check is_boundary() helper method
        // Expected: Correctly identifies boundary nodes
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 2.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert!(
            !result.is_boundary(entity_ids[0]),
            "Node 0 should NOT be boundary"
        );
        assert!(
            !result.is_boundary(entity_ids[1]),
            "Node 1 should NOT be boundary"
        );
        assert!(
            result.is_boundary(entity_ids[2]),
            "Node 2 should be boundary"
        );
        assert!(
            !result.is_boundary(entity_ids[3]),
            "Node 3 should NOT be boundary"
        );
    }

    #[test]
    fn test_impact_radius_with_progress() {
        // Scenario: Progress variant matches non-progress variant
        // Expected: Same results, progress callback called
        use crate::progress::NoProgress;

        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 3.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let progress = NoProgress;
        let result_with = impact_radius_with_progress(&graph, entity_ids[0], &config, &progress)
            .expect("Impact radius with progress should succeed");
        let result_without =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(
            result_with.size, result_without.size,
            "Progress and non-progress results should match"
        );
        assert_eq!(
            result_with.blast_zone, result_without.blast_zone,
            "Blast zones should match"
        );
        assert_eq!(
            result_with.boundary, result_without.boundary,
            "Boundaries should match"
        );
    }

    #[test]
    fn test_impact_radius_zero_max_distance() {
        // Scenario: max_distance = 0
        // Expected: Only source node in blast zone
        let graph = create_impact_chain();
        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        let config = ImpactRadiusConfig {
            max_distance: 0.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        assert_eq!(result.size, 1, "Should only have source node");
        assert!(
            result.blast_zone.contains(&entity_ids[0]),
            "Source should be in blast zone"
        );
        assert!(
            !result.blast_zone.contains(&entity_ids[1]),
            "Node 1 should NOT be in blast zone"
        );
    }

    #[test]
    fn test_impact_radius_shorter_path() {
        // Scenario: Graph with multiple paths, should find shortest
        // Create a graph where node 2 can be reached via two paths
        let graph = SqliteGraph::open_in_memory().expect("Failed to create graph");

        // Create nodes: 0, 1, 2, 3
        for i in 0..4 {
            let entity = crate::GraphEntity {
                id: 0,
                kind: "node".to_string(),
                name: format!("node_{}", i),
                file_path: Some(format!("node_{}.rs", i)),
                data: serde_json::json!({}),
            };
            graph
                .insert_entity(&entity)
                .expect("Failed to insert entity");
        }

        let entity_ids: Vec<i64> = graph.list_entity_ids().expect("Failed to get IDs");

        // Path 1: 0 -> 1 -> 3 (2 hops)
        let edge1 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[1],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge1).expect("Failed to insert edge");

        let edge2 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[1],
            to_id: entity_ids[3],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge2).expect("Failed to insert edge");

        // Path 2: 0 -> 2 -> 3 (2 hops)
        let edge3 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[2],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge3).expect("Failed to insert edge");

        let edge4 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[2],
            to_id: entity_ids[3],
            edge_type: "next".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge4).expect("Failed to insert edge");

        // Direct path: 0 -> 3 (1 hop)
        let edge5 = crate::GraphEdge {
            id: 0,
            from_id: entity_ids[0],
            to_id: entity_ids[3],
            edge_type: "direct".to_string(),
            data: serde_json::json!({}),
        };
        graph.insert_edge(&edge5).expect("Failed to insert edge");

        let config = ImpactRadiusConfig {
            max_distance: 10.0,
            max_hops: None,
            weight_fn: &default_weight_fn,
        };

        let result =
            impact_radius(&graph, entity_ids[0], &config).expect("Impact radius should succeed");

        // Node 3 should have distance 1 (shortest path)
        assert_eq!(
            *result.distances.get(&entity_ids[3]).unwrap(),
            1.0,
            "Node 3 should have distance 1 via direct edge"
        );
    }
}
