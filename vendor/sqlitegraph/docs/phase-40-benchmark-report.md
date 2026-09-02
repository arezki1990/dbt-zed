# Phase 40 Allocation-Aware Optimization - Benchmark Report

**Date:** 2026-01-25
**Plan:** 40-12 - Benchmark Gates and IO-12 Validation
**Duration:** ~4 minutes execution

## Executive Summary

Phase 40's allocation-aware optimization **did NOT achieve the IO-12 target** of Chain(500) <=75ms. The actual result of 213.24ms represents a 9% improvement over the Wave 1 baseline, but is still ~3x slower than the target.

**Key Finding:** The contiguous reservation mechanism implemented in Wave 2 (plans 40-07 through 40-11) did NOT produce the expected 3x performance improvement for chain traversal.

## IO-12 Target Validation

### Chain(500) Performance

| Metric | Baseline (40-06) | Actual (40-12) | Target | Status |
|--------|------------------|----------------|--------|--------|
| Chain(500) | 232.85ms | 213.24ms | <=75ms | FAIL |
| Improvement | - | +9.2% vs baseline | 3x | FAIL |

**Analysis:**
- Target: 75ms (3x improvement from baseline)
- Actual: 213.24ms (0.91x improvement from baseline)
- Gap: ~3x slower than target
- The contiguous allocation optimization did NOT achieve expected performance

### Chain(100) Performance

| Metric | Actual | Target | Status |
|--------|--------|--------|--------|
| Chain(100) | ~40ms | ~15ms (extrapolated) | Not measured in detail |

## Non-Chain Pattern Regression Tests

### Star Traversal

| Metric | Baseline (40-06) | Actual (40-12) | Change | Status |
|--------|------------------|----------------|--------|--------|
| Star(100) | ~24.85us | 23.094us | -7.1% | PASS (improvement) |

**Status:** PASS - No regression, actual improvement observed

### Random Traversal

| Metric | Baseline (40-06) | Actual (40-12) | Change | Status |
|--------|------------------|----------------|--------|--------|
| Random(100) | ~24.88us | 22.839us | -8.2% | PASS (improvement) |
| Random(500) | ~15.59us | 14.856us | -4.7% | PASS (improvement) |

**Status:** PASS - No regression, improvements observed across all random patterns

## Contiguous Allocation Metrics

### Reservation Rate

**Metric:** Contiguous reservation success rate for chains >= 10 nodes
**Target:** >=80%
**Actual:** NOT MEASURED

**Issue:** The contiguous reservation rate benchmark was not implemented as specified in the plan. The `io12_validation.rs` benchmark does not include metrics for:
- How often contiguous reservations succeed
- Whether clusters are actually stored contiguously
- The correlation between reservation success and traversal performance

### Fragmentation Increase

**Metric:** External fragmentation from contiguous reservations
**Target:** <=5%
**Actual:** NOT MEASURED

**Issue:** The fragmentation benchmark (`microbench_fragmentation.rs`) runs criterion-based microbenchmarks for `are_clusters_contiguous()` but does NOT measure:
- Actual fragmentation in the database file
- Fragmentation increase from chain graph operations
- Whether contiguous reservations cause fragmentation

## Snapshot Isolation

| Test Suite | Tests Run | Passed | Status |
|------------|-----------|--------|--------|
| acid_snapshot_test | 1 | 1 | PASS |

**Status:** PASS - Snapshot isolation test (test_snapshot_id_monotonic) passes

## Root Cause Analysis

### Why IO-12 Target Was Not Achieved

**Hypothesis 1: Contiguous reservations not happening**
- The `FreeSpaceManager::try_reserve_contiguous()` API exists
- The `AdjacencyWriter::write_cluster_with_hint()` exists
- However, the benchmark may not be using these code paths
- The chain traversal benchmark uses `GraphFile` directly at the edge/node store level
- The contiguous allocation optimization may not be integrated into the low-level graph creation used in benchmarks

**Hypothesis 2: Chain detection not working**
- The `LinearDetector` may not be detecting chains in the benchmark
- The threshold-gated activation may not be triggering
- Need to verify that `observe_with_cluster()` is being called with the right hints

**Hypothesis 3: Sequential read optimization not working**
- Even if clusters are allocated contiguously, the sequential read path may not be enabled
- The `SequentialClusterReader` may not be integrated into the traversal path

### Verification Needed

To understand the gap, we need to measure:

1. **Are contiguous reservations being attempted?**
   ```rust
   // Add metrics to FreeSpaceManager
   - reservation_attempts: AtomicU64
   - reservation_successes: AtomicU64
   ```

2. **Are clusters actually stored contiguously?**
   ```rust
   // Verify cluster offsets after graph creation
   - Collect cluster offsets for chain graphs
   - Run are_clusters_contiguous() on the offsets
   ```

3. **Is the sequential read path being used?**
   ```rust
   // Add metrics to native_bfs
   - sequential_reads: AtomicU64
   - random_reads: AtomicU64
   ```

## Recommendations

### For Phase 40 Completion

**Option A: Investigate and fix (additional work)**
- Add metrics to measure contiguous reservation rate
- Verify chain detection is working
- Profile to find the actual bottleneck (I/O vs CPU)
- This may require 1-2 additional plans

**Option B: Accept partial completion**
- Document that IO-12 target was not achieved
- Note that Wave 2 implementation is complete but not validated
- Create follow-up phase to complete validation

**Option C: Redefine IO-12 target**
- The original target of 75ms may have been unrealistic
- A 10% improvement (213ms) is still valuable
- Update target based on actual measurements

### For Immediate Next Steps

1. **Add metrics** to measure:
   - Contiguous reservation rate
   - Sequential read rate
   - Actual cluster contiguity

2. **Profile** the Chain(500) benchmark to identify bottleneck:
   ```bash
   cargo flamegraph --bench io12_validation -- chain_traversal/500
   ```

3. **Verify** that the allocation-aware code paths are actually being used in the benchmark

## Conclusion

**Phase 40 Wave 2 (Allocation-Aware Optimization) Implementation Status:**
- ✅ FreeSpaceManager API (40-07) - Complete
- ✅ Region accounting (40-08) - Complete
- ✅ AdjacencyWriter hints (40-09) - Complete
- ✅ Threshold-gated activation (40-10) - Complete
- ✅ WAL records (40-11) - Complete
- ❌ IO-12 validation (40-12) - FAILED

**Key Finding:** The implementation is complete but the expected performance improvement was NOT achieved. This indicates either:
1. The optimization code paths are not being used in the benchmark
2. The optimization does not provide the expected benefit
3. There is a bug in the implementation

**Recommendation:** Create Phase 41 to investigate the performance gap and either fix the issue or redefine the IO-12 target based on actual measurements.
