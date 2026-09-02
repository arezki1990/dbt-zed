# Main V3 Benchmark Rerun Report

**Date:** 2026-03-11
**Status:** BENCHMARK COMPLETE - Critical corruption bug found in reopen
**Scope:** Native V3 Backend - Current performance state after latest fixes
**Build:** Release mode (`--bench` profile)

---

## 1. FINDINGS

### Summary: V3 is excellent for graph traversals, poor for point operations

**V3 Strengths:**
- **BFS traversal:** 2.3x faster than SQLite on small, 1.4x faster on medium
- **Edge insertion:** 10x faster than SQLite (major win)

**V3 Weaknesses:**
- **Node insertion:** 10-38x slower than SQLite (critical gap)
- **Point reads (get_node):** 25-190x slower than SQLite (critical gap)
- **Neighbors query:** 23-160x slower than SQLite (surprising, given BFS strength)
- **Reopen cost:** 20x slower than SQLite on small
- **Corruption bug:** Medium reopen CRASHES with "used_bytes exceeds page boundary"

### Critical Bug Discovered

**Error:** `Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 17764 > 4096`

**Location:** `reopen/cost/v3/medium` benchmark (10K nodes, 50K edges)

**Impact:** V3 cannot reliably reopen databases after medium-scale writes. This is a correctness issue that must be fixed before V3 can be recommended for general workloads.

### Recent Fix Impact Assessment

The cache clone fix eliminated the 3x write regression seen with larger cache sizes. However, the write path is still fundamentally slow compared to SQLite. The root cause is now likely:

1. **Per-node page allocations** instead of batch operations
2. **B+Tree overhead** on every insert (SQLite uses WAL + bulk commit)
3. **WAL writes** on each operation instead of batched commits

---

## 2. BENCH RERUN PLAN

**Benchmark harness:** `benches/sqlite_v3_comparison.rs`

**Categories measured:**
- write/insert_nodes (1K and 10K nodes)
- write/insert_edges (1K and 10K nodes with 5x edges)
- read/get_node (point lookup)
- read/neighbors (adjacency fetch)
- traversal/bfs (graph traversal)
- reopen/cost (database open time)
- query/by_kind (O(n) scan - V3 limitation)
- query/by_name_pattern (prefix matching)

**Build mode:** Release (`--bench` profile = optimized + debuginfo)

**Dataset sizes:**
- Small: 1K nodes, 5K edges
- Medium: 10K nodes, 50K edges

---

## 3. EXECUTION NOTES

**Completed benchmarks:**
- All write benchmarks (insert_nodes, insert_edges)
- All read benchmarks (get_node, neighbors)
- All traversal benchmarks (BFS)
- Reopen/cost/small
- Query benchmarks (by_kind, by_name_pattern)

**Failed benchmarks:**
- `reopen/cost/v3/medium` - CRASHED with corruption error

**Execution time:** ~5 minutes for release build + ~3 minutes for benchmark runs

**Warnings:** Several benchmarks couldn't complete target sample size in allotted time (extended automatically by criterion).

---

## 4. BEFORE vs AFTER RESULTS

### Complete Comparison Table (Release Mode)

| Category | Size | SQLite (ms) | V3 Current (ms) | V3/SQLite | Notes |
|----------|------|-------------|-----------------|-----------|-------|
| **insert_nodes** | small (1K) | 3.28 | 34.76 | **10.6x slower** | Node write is critical path |
| **insert_nodes** | medium (10K) | 34.50 | 1,314.49 | **38.1x slower** | Gap widens at scale |
| **insert_edges** | small | 38.47 | 3.74 | **10.3x faster** | V3 strength - adjacency store |
| **insert_edges** | medium | 417.40 | 60.70 | **6.9x faster** | Maintains advantage at scale |
| **get_node** | small | 0.042 | 1.06 | **25.2x slower** | Point read is very slow |
| **get_node** | medium | 0.107 | 20.34 | **190.2x slower** | Gap explodes at scale |
| **neighbors** | small | 0.044 | 1.03 | **23.4x slower** | Surprising given BFS strength |
| **neighbors** | medium | 0.133 | 21.24 | **159.7x slower** | Similar to get_node |
| **bfs** | small | 2.77 | 1.22 | **2.3x faster** | V3 strength confirmed |
| **bfs** | medium | 28.99 | 20.23 | **1.4x faster** | Maintains traversal advantage |
| **reopen_cost** | small | 0.55 | 10.95 | **19.9x slower** | V3 has higher startup cost |
| **reopen_cost** | medium | 0.51 | **CRASH** | CORRUPTION | **CRITICAL BUG** |
| **query_by_kind** | tiny (100) | 0.014 | 0.025 | 1.8x slower | O(n) scan limitation |
| **query_by_kind** | small (1K) | 0.028 | 0.044 | 1.5x slower | Acceptable for small graphs |
| **query_by_name** | small | 0.060 | 0.085 | 1.4x slower | Prefix match is fast enough |

### Key Observations

1. **V3 is optimized for graph traversals, not point operations:** BFS is 1.4-2.3x faster, but point reads are 25-190x slower.

2. **Node insertion is the biggest performance gap:** 38x slower at medium scale. This is the primary bottleneck for write-heavy workloads.

3. **Edge insertion is V3's biggest strength:** 6-10x faster than SQLite. The in-memory adjacency store is working well.

4. **The get_node regression is alarming:** 190x slower at medium scale. This suggests B+Tree lookups are not optimized.

5. **Reopen corruption is a correctness crisis:** V3 cannot reliably reopen databases after medium-scale writes.

---

## 5. NEXT USEFUL TARGET

### Recommended Target: **Fix the medium reopen corruption bug FIRST**

**Rationale:**
1. **Correctness before performance:** A database that crashes on reopen is not usable.
2. **Blocks all other work:** Can't trust any V3 data until this is fixed.
3. **May explain other issues:** The corruption could be contributing to slow reads/writes.

**Error message:** `Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 17764 > 4096`

**Likely cause:** Page corruption during write or WAL checkpoint. The "used_bytes" field (offset 32-33 in page header) is being written with a value that exceeds the 4096-byte page size.

**Investigation approach:**
1. Add forensics to track used_bytes writes
2. Check for integer overflow in page packing
3. Verify WAL replay logic preserves page boundaries
4. Check if B+Tree split/merge corrupts page headers

### Secondary Target (after corruption fix): **Optimize get_node performance**

**Rationale:**
1. **190x gap is actionable:** B+Tree lookups should be O(log n), not 190x slower than SQLite.
2. **Blocks read-heavy workloads:** Most query patterns depend on fast node lookup.
3. **May improve insert too:** Node insertion requires page lookups.

**Specific optimization paths:**
1. **Add traversal cache warmup:** Pre-load B+Tree internal pages during open
2. **Implement read-ahead:** Batch load adjacent leaf pages
3. **Optimize B+Tree descent path:** Reduce page loads per lookup
4. **Consider larger page size:** 8KB or 16KB pages would reduce tree height

### NOT Recommended (at this time):

- **Sequential/predictive prefetch:** V3 already wins at BFS; this is premature optimization
- **Cache tuning:** Cache clone fix is done; further tuning is marginal vs corruption bug
- **Block-locality:** Three phases showed no meaningful benefit
- **Query indexes:** query_by_kind is only 1.5x slower; not the bottleneck

---

## 6. REMAINING RISKS

### 1. Corruption Severity Unknown

**Risk:** The "used_bytes exceeds page boundary" corruption may affect more than just reopen.

**Evidence:**
- Only medium reopen crashed
- Small reopen succeeded
- Write benchmarks completed without error

**Mitigation:** Need forensic investigation to understand:
- When does corruption occur? (during write, flush, or reopen?)
- Which pages are affected? (node pages, B+Tree pages, WAL?)
- Is data actually lost, or is it a metadata-only issue?

### 2. Performance Gap at Scale

**Risk:** The 190x get_node slowdown may worsen beyond 10K nodes.

**Evidence:**
- Small gap: 25x
- Medium gap: 190x
- Trend: Gap widens 7.6x when dataset grows 10x

**Implication:** At 100K nodes, get_node could be 1400x slower than SQLite.

### 3. Query Semantics Not Validated

**Risk:** The benchmark notes semantic differences (GLOB vs substring) but doesn't validate correctness.

**Evidence:**
- `query_by_name_pattern` uses different semantics (SQLite GLOB vs V3 substring)
- No correctness tests verify results match
- Performance comparison assumes equivalent results

**Mitigation:** Add correctness assertions to query benchmarks.

### 4. Benchmark Variance

**Risk:** Some benchmarks show high variance (large error bars).

**Evidence:**
- `reopen/cost/sqlite/small`: +/- 295209 ns (54% of mean)
- `traversal/bfs/v3/medium`: +/- 6178302 ns (30% of mean)

**Implication:** Some measurements may not be stable enough for optimization decisions.

---

## CONCLUSION

**Native V3 is a specialized graph database:**

- **Excellent for:** Graph traversals (BFS), edge-heavy workloads
- **Poor for:** Node insertion, point reads (get_node), read-after-write patterns
- **Broken for:** Reopening medium databases (corruption bug)

**Recommended action order:**
1. **Fix corruption bug** (reopen/critical)
2. **Optimize get_node** (190x gap is actionable)
3. **Then consider:** Insert optimization, query indexing, cache tuning

**Success criteria for next phase:**
- Corruption bug fixed with forensic root cause
- get_node within 10x of SQLite
- Reopen cost within 5x of SQLite

---

**Benchmark artifacts:**
- Raw output: `/tmp/bench_run.log`
- Criterion results: `target/criterion/`
- This report
