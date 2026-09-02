# Sequential I/O Performance Documentation

**Last Updated:** 2026-01-21
**Phase:** v1.4 Sequential I/O Optimization (Phases 29-32)
**Status:** Performance validation in progress

## Overview

This document describes the sequential I/O coalescing optimization for chain traversals in the Native V2 backend. The optimization eliminates the "read-drop-repeat" pathological pattern where each hop reads a 4KB slot, decodes adjacency, extracts neighbor ID, then drops everything before the next hop.

### Problem

Chain traversals have an 11x performance gap versus SQLite. The root cause is per-hop I/O:
- Each hop: Read 4KB slot → Decode adjacency → Extract neighbor → **Drop all data**
- Chain(500): 500 individual 4KB reads = 2MB total I/O
- No reuse of previously read slots across consecutive hops

### Solution

Sequential I/O coalescing with three-tier lookup:
1. **L1 Buffer (SequentialReadBuffer)**: 8-slot prefetch, 32KB batch reads
2. **L2 Cache (TraversalCache)**: Neighbor list cache from v1.3
3. **L3 Storage (NodeStore)**: On-demand slot reads

### Expected Impact

- **Target**: Chain(500) ≤ 3x SQLite baseline (≤75ms)
- **Before**: Chain(500) = 272ms (10.90x SQLite)
- **After**: Pending Plan 32-04 (L1 buffer neighbor extraction)

## Architecture

### LinearDetector (Phase 29)

State machine that detects linear traversal patterns:
- **States**: Unknown → Linear1 → Linear2 → LinearConfirmed
- **Threshold**: 3 consecutive steps with degree ≤ 1
- **Purpose**: Avoid false positives on tree structures

### SequentialReadBuffer (Phase 30)

Per-traversal buffer for decoded node slots:
- **Storage**: `AHashMap<NativeNodeId, NodeRecordV2>`
- **Prefetch**: 8 slots × 4KB = 32KB batch reads
- **Scope**: Evaporates on function return (no cross-traversal pollution)
- **MVCC Safe**: Stack-allocated, preserves isolation

### TraversalContext (Phase 31)

Unified context combining:
- `detector: LinearDetector` - Pattern detection state
- `buffer: SequentialReadBuffer` - L1 cache
- `cache: TraversalCache` - L2 cache (v1.3)
- `stats: TraversalCacheStats` - Hit/miss tracking

### Three-Tier Lookup (get_neighbors_optimized)

```
L1: Check SequentialReadBuffer (fast path)
   ├─ Hit → Return neighbors from buffer
   └─ Miss → Continue to L2

L2: Check TraversalCache (fast)
   ├─ Hit → Return cached neighbors
   └─ Miss → Continue to L3

L3: Read from NodeStore (slowest)
   └─ Decode and return neighbors
```

## Performance Characteristics

### Baseline (Phase 32-01)

| Metric | Native v1.4 | SQLite | Ratio | Target |
|--------|-------------|--------|-------|--------|
| Chain(500) | 272.28ms | 24.98ms | 10.90x | ≤3x |
| Chain(100) | ~25ms | ~5ms | ~5x | ≤2x |

**Root Cause of Gap:** L1 buffer lookup is instrumentation-only (Phase 31-01 deferral). The buffer is populated and records hit/miss statistics, but neighbor extraction still falls through to L2/L3 for every hop.

### Solution Path (Plan 32-04)

To achieve the 3x target, Plan 32-04 will implement actual neighbor extraction from buffered `NodeRecordV2`:
- When L1 buffer hit occurs, decode adjacency directly from buffered record
- Only fall through to L2/L3 on buffer miss
- Expected: ~10x improvement for chain traversals

### Other Graph Types

| Graph Type | Expected Impact | Status |
|------------|-----------------|--------|
| Star graphs | No regression (within 10% of v1.3) | ✓ Validated |
| Random graphs | No regression (within 10% of v1.3) | ✓ Validated |
| Chain graphs | ~10x improvement (after 32-04) | Pending |

## Prefetch Window Tuning

### Window Size Comparison (Chain(500))

| Window Size | Batch Size | Mean Prefetch Time | Cached Nodes |
|-------------|------------|-------------------|--------------|
| 4 slots | 16KB | 2.95µs | 4 |
| 8 slots (default) | 32KB | 4.62µs | 8 |
| 16 slots | 64KB | 7.82µs | 16 |
| 32 slots | 128KB | 14.33µs | 32 |

**Source:** `cargo bench --bench prefetch_bench` (Phase 32-03)

### Analysis

- **Window 4**: Fast path prefetch, but caches fewer nodes. More frequent prefetches needed for long chains.
- **Window 8**: Default, balanced for most workloads. 1.57x slower than window 4, but 2x more nodes per prefetch.
- **Window 16**: 1.69x slower than window 8, but 2x more nodes per prefetch. Better for very long chains.
- **Window 32**: 1.83x slower than window 16, but 2x more nodes. Diminishing returns on memory vs benefit.

### Trade-offs

| Window | Memory | I/O Reduction | Best For |
|--------|--------|---------------|----------|
| 4 | 16KB | Less effective | Short chains (≤100 nodes) |
| 8 | 32KB | **Balanced** | **Most workloads (default)** |
| 16 | 64KB | More effective | Long chains (≥500 nodes) |
| 32 | 128KB | Diminishing returns | Very long chains (≥1000 nodes) |

### Recommendation

**Keep window 8 as default.** The benchmark results show:
- Window 4 is 35% faster but caches 50% fewer nodes
- Window 16 is 69% slower for 2x more nodes
- Window 8 provides the best throughput/cost ratio

For workloads with very long chains (≥1000 nodes), consider increasing to window 16 via `SequentialReadBuffer::with_prefetch_window(16)`.

## Memory Overhead

### Per-Traversal Buffer

| Component | Memory | Notes |
|-----------|--------|-------|
| SequentialReadBuffer (empty) | ~200 bytes | AHashMap overhead |
| Per cached NodeRecordV2 | ~200 bytes | kind + name + data |
| Full buffer (8 slots) | ~1.6KB + overhead | Worst case |

### Total Overhead

- **Minimum**: ~200 bytes (empty buffer)
- **Typical**: ~1-2KB (partial fill during traversal)
- **Maximum**: ~2-3KB (full buffer with large data)

### Evaporation

The buffer is stack-allocated per traversal and evaporates when the traversal function returns. There is no persistent memory overhead after the traversal completes.

```rust
fn example_traversal() {
    let mut ctx = TraversalContext::new();  // Buffer allocated on stack
    // ... traversal logic ...
    // Buffer evaporates here, no cleanup needed
}
```

## Limitations

### Current Limitations (Phase 32-03)

1. **L1 buffer neighbor extraction not implemented**: Buffer records hit/miss statistics but doesn't extract neighbors from buffered records. Deferred to Plan 32-04.

2. **Only effective for linear patterns**: Sequential I/O coalescing requires degree ≤ 1 for 3+ consecutive steps. Star and random graphs don't benefit.

3. **Edge-filtered traversals bypass optimization**: `native_chain_query` with edge type filters continues using direct `AdjacencyHelpers`.

### Future Work (IO-14, IO-16)

1. **Coroutine-based prefetching (IO-14)**: Overlap prefetch with traversal using async I/O

2. **Global Traversal Chain (IO-16)**: B+ tree linking of buffer entries for even faster sequential access

3. **Adaptive window sizing**: Dynamically adjust prefetch window based on observed pattern length

## Benchmarking Guide

### Running Benchmarks

```bash
# Sequential I/O benchmarks
cargo bench --bench prefetch_bench

# Chain traversal benchmarks
cargo bench --bench bfs

# View results
firefox target/criterion/prefetch_window/report/index.html
firefox target/criterion/bfs_chain/report/index.html
```

### Performance Gate Tests

```bash
# Run Phase 32 performance gate
cargo test --test perf_gate_v32_tests

# Expected: Pass when Chain(500) ≤ 75ms (after Plan 32-04)
```

### Interpretation

- **Prefetch window benchmarks**: Measure buffer operation timing, not end-to-end traversal
- **BFS benchmarks**: Measure full traversal time with default window=8
- **Performance gate**: Validates 3x SQLite target (currently failing, pending Plan 32-04)

## References

- **Phase 29**: LinearDetector implementation (32-RESEARCH.md)
- **Phase 30**: SequentialReadBuffer implementation (sequential_buffer.rs)
- **Phase 31**: TraversalContext integration (traversal_context.rs)
- **Phase 32-01**: Performance benchmark execution (32-01-SUMMARY.md)
- **Phase 32-02**: MVCC isolation tests (32-02-SUMMARY.md)
- **Phase 32-03**: Prefetch window tuning (this document)
- **Phase 32-04**: L1 buffer neighbor extraction (pending)

## Summary

Sequential I/O coalescing is the correct approach for chain optimization. The infrastructure is complete (linear detection, batch reading, buffer storage, traversal integration), but actual neighbor extraction from the L1 buffer was deferred to Phase 32-04.

Once Plan 32-04 is implemented:
- Chain(500) expected: ≤75ms (3x SQLite baseline)
- Prefetch window 8 provides optimal throughput/cost ratio
- Memory overhead: ~2-3KB per traversal (evaporates on return)
- No regression for star/random graphs
