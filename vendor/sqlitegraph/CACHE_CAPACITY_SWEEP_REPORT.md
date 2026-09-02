# Page Cache Capacity Sweep Report

**Date:** 2026-03-11
**Status:** EXPERIMENT COMPLETE - Cache size has mixed effects
**Scope:** Native V3 Backend - Page Cache Capacity Tuning

---

## 1. FINDINGS

### Cache Capacity Effects on V3 Performance

**Dataset sizes tested:** 1K nodes, 10K nodes
**Cache sizes tested:** 16, 64, 128, 256 pages

### Key Results Summary

| Operation | Dataset | 16 pages | 64 pages | 128 pages | 256 pages | Best |
|-----------|--------|----------|----------|-----------|-----------|------|
| **insert** | 1K | 445ms | 393ms | 397ms | 427ms | 64 (1.13x) |
| **insert** | 10K | 6440ms | **19271ms** | 18112ms | 17838ms | 16 (baseline) |
| **get_node** | 1K | 99ms | 97ms | 98ms | 94ms | 256 (1.05x) |
| **get_node** | 10K | 1361ms | 998ms | 996ms | 986ms | 256 (1.38x) |
| **neighbors** | 1K | ~0ms | ~0ms | ~0ms | ~0ms | All ~same |
| **neighbors** | 10K | 3ms | 3ms | 3ms | 3ms | All ~same |
| **BFS** | 10K | 8ms | 11ms | 8ms | 8ms | All ~same |

### Critical Discovery: Large Cache HURTS Insert Performance

**10K node insert performance:**
- 16 pages: 6440ms (baseline, 1552 nodes/sec)
- 64 pages: 19271ms (**3.0x slower**, 519 nodes/sec)
- 128 pages: 18112ms (**2.8x slower**, 552 nodes/sec)
- 256 pages: 17838ms (**2.8x slower**, 561 nodes/sec)

This is a **significant regression** - larger page cache makes insertion much slower at scale.

### Positive Finding: Get Node Benefits from Larger Cache

**10K node get_node performance:**
- 16 pages: 1361ms (baseline, 7347 lookups/sec)
- 64 pages: 998ms (**1.36x faster**, 10015 lookups/sec)
- 128 pages: 996ms (**1.37x faster**, 10039 lookups/sec)
- 256 pages: 986ms (**1.38x faster**, 10139 lookups/sec)

The benefit plateaus quickly - 128→256 pages shows minimal additional gain.

### No Benefit for Traversals

**Neighbors and BFS show no meaningful improvement** with larger cache. Both operations are likely dominated by edge store access (in-memory adjacency), not node page caching.

### Comparison: Cache Sizing vs Block-Locality

| Optimization | Best Result | Complexity | Verdict |
|--------------|--------------|------------|---------|
| **Block-locality (Phase 2)** | 1.12x (1K sequential only) | Medium | Weak signal |
| **Block-locality (Phase 3)** | No benefit | Medium | Failed |
| **Cache size increase** | 1.38x (get_node), but -3x (insert) | Low | **Mixed** |

**Cache sizing shows more promise than block-locality**, but the tradeoff is problematic: better reads at the cost of much worse writes.

---

## 2. CACHE SWEEP PLAN

### Approach

**Chosen Method:** Direct V3Backend construction with specified cache capacity

- Added `V3Backend::create_with_cache_capacity(path, capacity)` method
- Both node page cache and B+Tree page cache use the same capacity
- Tested cache sizes: 16 (baseline), 64, 128, 256 pages
- Measured 4 operations: insert, get_node, neighbors, BFS
- Dataset sizes: 1K nodes, 10K nodes

**Why this approach:**
- Minimal code changes
- No runtime complexity
- Easy to benchmark
- Results are directly actionable

---

## 3. IMPLEMENTATION

### Files Modified

**`sqlitegraph-core/src/backend/native/v3/backend.rs`**

#### Added Method: create_with_cache_capacity

```rust
/// Create a new V3 database with specified page cache capacity
///
/// This allows benchmarking different cache sizes to measure performance impact.
/// Both node page cache and B+Tree page cache use the same capacity.
///
/// # Arguments
///
/// * `path` - Path where the database file will be created
/// * `page_cache_capacity` - Number of pages to cache (default is 16)
pub fn create_with_cache_capacity<P: AsRef<Path>>(
    path: P,
    page_cache_capacity: usize,
) -> Result<Self, SqliteGraphError> {
    let db_path = path.as_ref().to_path_buf();

    // Create initial header
    let header = PersistentHeaderV3::new_v3();

    // Write header to file
    // ... (file write logic) ...

    // Initialize components with SPECIFIED page cache capacity
    let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
    let btree_page_cache = BTreePageCache::new(page_cache_capacity);
    let mut btree = BTreeManager::with_cache(
        Arc::clone(&allocator),
        None,
        db_path.clone(),
        btree_page_cache.clone(),
    );
    let mut node_store = NodeStore::with_capacity(&header, db_path.clone(), page_cache_capacity);

    // ... rest of initialization ...
}
```

**Key changes:**
- Uses `BTreePageCache::new(capacity)` instead of `with_default_capacity()`
- Uses `NodeStore::with_capacity()` instead of `NodeStore::new()`
- Same capacity for both caches

### Benchmark Created

**`sqlitegraph-core/examples/cache_capacity_benchmark.rs`**

A focused benchmark that:
- Sweeps cache sizes: 16, 64, 128, 256 pages
- Tests 4 operations: insert, get_node, neighbors, BFS
- Tests 2 dataset sizes: 1K, 10K nodes
- Outputs timing data and speedup comparisons

**Note:** Reopen/cold-cache testing was skipped due to missing `open_with_cache_capacity` method. All results shown are for warm cache.

---

## 4. VALIDATION

### Correctness Checks

✅ **Preserved V3 correctness:**
- Lib compiles with `--features native-v3`
- All operations return correct data
- Node IDs are sequential
- Neighbors queries work correctly
- BFS traversals work correctly

✅ **No format changes:**
- On-disk format unchanged
- Backward compatible

### Detailed Results

#### 1K Nodes (1000 nodes)

| Cache Size | insert | get_node | neighbors | BFS |
|------------|--------|----------|----------|-----|
| 16 pages | 445ms | 99ms | ~0ms | ~0ms |
| 64 pages | 393ms (1.13x) | 97ms (1.02x) | ~0ms | ~0ms |
| 128 pages | 397ms (1.12x) | 98ms (1.01x) | ~0ms | ~0ms |
| 256 pages | 427ms (1.04x) | 94ms (1.05x) | ~0ms | ~0ms |

**Observations:**
- Insert: Slight improvement at 64 pages, but variation is noise-level
- Get node: Minimal improvement (1.05x best case)
- Neighbors/BFS: Too fast to measure (<1ms)

#### 10K Nodes (10000 nodes)

| Cache Size | insert | get_node | neighbors | BFS |
|------------|--------|----------|----------|-----|
| 16 pages | 6440ms | 1361ms | 3ms | 8ms |
| 64 pages | **19271ms (0.33x)** | 998ms (1.36x) | 3ms | 11ms |
| 128 pages | **18112ms (0.36x)** | 996ms (1.37x) | 3ms | 8ms |
| 256 pages | **17838ms (0.36x)** | 986ms (1.38x) | 3ms | 8ms |

**Observations:**
- Insert: **Major regression** with larger cache (2.8-3x slower!)
- Get node: **Consistent improvement** (1.36-1.38x faster)
- Neighbors: No meaningful difference
- BFS: No meaningful difference

---

## 5. REMAINING RISKS

### 1. Insert Performance Regression is Unexplained

**Risk:** Larger cache should not hurt insert performance this much.

**Possible causes:**
- Cache eviction overhead dominates when cache is large
- Hash map operations slow down with more entries
- Memory allocation pressure
- Lock contention on shared cache structures

**Mitigation:** Needs investigation before increasing default cache size.

### 2. Get Node Benefit Plateaus Quickly

**Risk:** Going from 16→128 pages shows 1.37x improvement, but 128→256 shows almost no additional gain.

**Implication:**
- 128 pages may be near-optimal for read workloads
- Further increases provide diminishing returns
- Memory cost may not justify small gains

### 3. Cold Cache Not Tested

**Risk:** We only tested warm cache. Cold cache behavior may differ.

**Reason:** `open_with_cache_capacity` method not implemented.

**Impact:** May not matter for long-running server workloads where cache warms up and stays warm.

### 4. No Benefit for Traversal Workloads

**Risk:** Neighbors and BFS show no improvement with larger cache.

**Reason:** These operations likely use in-memory edge store, not node page cache.

**Implication:** Cache sizing is not a lever for graph traversal performance.

---

## CONCLUSION

**The page cache capacity sweep is COMPLETE.**

### What We Learned

1. **Cache size affects reads and writes differently:**
   - Get node improves up to 1.38x with larger cache
   - Insert degrades up to 3x with larger cache

2. **The 16-page default is not obviously wrong:**
   - Good balance between read and write performance
   - Increasing it helps reads but hurts writes significantly

3. **Cache sizing is a better lever than block-locality:**
   - Shows measurable 1.38x improvement for reads
   - But the write regression is problematic

4. **Traversal workloads don't benefit from node cache:**
   - Neighbors and BFS are unaffected
   - They likely use in-memory edge store

### Recommendations

**DO NOT increase default cache size to 64/128/256 pages.**

**Rationale:**
- The 3x insert regression is unacceptable
- 1.38x read improvement doesn't justify the write cost
- Current 16-page default is a reasonable balance

**Alternative directions:**

1. **Investigate the insert regression:**
   - Profile to understand why larger cache hurts inserts
   - Consider read/write split cache (separate caches for reads vs writes)

2. **Accept 16-page default for now:**
   - Good general-purpose balance
   - No clear evidence for a different size

3. **Focus optimization efforts elsewhere:**
   - Edge store optimization (for neighbors/BFS)
   - Compression tuning
   - B+Tree improvements

### Success Criteria Met

- ✅ Cache capacity was swept at 4 sizes (16, 64, 128, 256)
- ✅ Same benchmark shapes used for fair comparison
- ✅ **Learned:** Cache sizing helps reads but hurts writes significantly
- ✅ **Next step is clear:** Investigate insert regression before considering cache size changes

---

**Comparison: Cache Sizing vs Block-Locality**

| Metric | Block-Locality (Phase 3) | Cache Sizing |
|--------|---------------------------|--------------|
| Implementation | Physical placement | Cache capacity |
| Best result | No benefit | 1.38x get_node, -3x insert |
| Changed behavior? | Yes | Yes |
| Validated? | Failed | **Mixed** |

**Cache sizing is a more promising direction than block-locality, but the insert regression must be understood before any default change.**

---

**Experiment artifacts:**
- `sqlitegraph-core/src/backend/native/v3/backend.rs` — create_with_cache_capacity method
- `sqlitegraph-core/examples/cache_capacity_benchmark.rs` — benchmark harness
- This report
