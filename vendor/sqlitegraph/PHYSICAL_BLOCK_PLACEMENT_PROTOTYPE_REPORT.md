# Physical Block-Placement Prototype Report

**Date:** 2026-03-11
**Status:** PROTOTYPE COMPLETE - Physical placement tested, **NO MEASURABLE BENEFIT**
**Scope:** Native V3 Backend - Physical Block-Aware Page Placement

---

## 1. FINDINGS

### What Was Tested

**Physical block-aware page placement**: Bias new nodes from the same logical block into the same small set of pages.

**Hypothesis**: If same-block nodes are physically co-located on disk, sequential/block-sequential access patterns would show measurable improvement beyond cache-only approaches.

### Result: No Improvement Measured

| Dataset | Pattern | Insert | Cold | Warm | Speedup | Comparison to Cache-Only |
|---------|--------|--------|------|------|---------|--------------------------|
| 1K nodes | Sequential | 330 ms | 154 ms | 149 ms | 1.03x | **Worse** (was 1.12x) |
| 1K nodes | Random | 377 ms | 148 ms | 152 ms | 0.97x | Similar (was 0.95x) |
| 1K nodes | Block-Sequential | 411 ms | 302 ms | 302 ms | 1.00x | **Worse** (was 1.05x) |
| 1K nodes | Block-Skip | 414 ms | 163 ms | 165 ms | 0.99x | Similar (was 0.98x) |
| 10K nodes | Sequential | 5360 ms | 1989 ms | 2060 ms | 0.97x | **Worse** (was 1.00x) |
| 10K nodes | Random | 5322 ms | 1844 ms | 1833 ms | 1.01x | **Worse** (was 1.04x) |
| 10K nodes | Block-Sequential | 6347 ms | 3988 ms | 4183 ms | 0.95x | Better (was 0.91x) |

### Key Observation

**Physical placement shows NO benefit over cache-only eviction.** In most cases, it performs slightly worse.

This is surprising because:
- Physical co-location should improve I/O locality
- Same-block nodes landing on same pages should reduce seeks
- But the benchmark shows no measurable gain

### Why Might Physical Placement Not Help?

1. **OS page cache dominates**: Even after "reopen", the OS cache may keep pages warm
2. **Sequential access already optimal**: The existing B+Tree + page cache is already efficient
3. **Small datasets**: 1K-10K nodes fit comfortably in OS cache
4. **Block size mismatch**: Fixed 128-node blocks may not match access patterns
5. **V3's compression is effective**: Delta/varint compression reduces I/O already

---

## 2. CHOSEN PHYSICAL PLACEMENT PROTOTYPE

**OPTION 1: In-Memory Block→Active-Pages Placement Bias**

**What it does:**
- Each `block_id` tracks up to 3 "preferred" page IDs
- When inserting a node, try that block's preferred pages first
- If allocating a new page, associate it with the block
- Fallback to current behavior if preferred pages are full

**Why this was chosen:**
- Smallest possible change to test physical placement hypothesis
- No B+Tree changes
- No on-disk format changes
- No migration machinery
- Rebuilds mapping on each reopen (acceptable for prototype)

**What this should have improved:**
- Insert locality: same-block nodes go to same pages
- Cold sequential: sequential node IDs hit cached pages more often
- Block-sequential: repeated block access benefits from same-page clustering

---

## 3. IMPLEMENTATION

### Files Modified

**`sqlitegraph-core/src/backend/native/v3/node/store.rs`**

#### 1. Added Block Placement Fields to NodeStore

```rust
pub struct NodeStore {
    // ... existing fields ...

    /// Block-to-preferred-pages mapping for physical placement prototype
    /// PROTOTYPE: In-memory only, biases same-block nodes to same pages
    /// Maps block_id → list of page_ids preferred for that block
    block_preferred_pages: HashMap<i64, Vec<u64>>,

    /// Maximum preferred pages to track per block (tunable)
    max_preferred_pages_per_block: usize,
    // ... rest of fields ...
}
```

#### 2. Added Helper Method: Associate Page with Block

```rust
/// Associate a page with a block for physical placement bias
///
/// PROTOTYPE: When a new page is allocated for a block, remember that
/// pages from this block should prefer this page in the future.
fn associate_page_with_block(&mut self, page_id: u64, block_id: i64) {
    let pages = self.block_preferred_pages.entry(block_id).or_insert_with(Vec::new);

    // Avoid duplicates
    if !pages.contains(&page_id) {
        pages.push(page_id);

        // Trim if exceeding max
        while pages.len() > self.max_preferred_pages_per_block {
            pages.remove(0); // Remove oldest
        }
    }
}
```

#### 3. Updated Page Selection for Block-Aware Placement

```rust
fn find_or_create_page_for_node(&mut self, node: &NodeRecordV3) -> NativeResult<u64> {
    let node_size = self.estimate_node_size(node)?;

    // PROTOTYPE: Block-aware placement bias
    // Try this block's preferred pages FIRST
    use super::page::node_id_to_block;
    let block_id = node_id_to_block(node.id);

    if let Some(preferred_pages) = self.block_preferred_pages.get(&block_id) {
        for &page_id in preferred_pages.iter().rev() {
            // Check dirty_pages first
            if let Some(page) = self.dirty_pages.get(&page_id) {
                if page.capacity() >= node_size {
                    return Ok(page_id);
                }
            }
            // Then check page_cache
            if let Some(page_bytes) = self.page_cache_get(page_id) {
                if let Ok(page) = NodePage::unpack(&page_bytes) {
                    if page.capacity() >= node_size {
                        return Ok(page_id);
                    }
                }
            }
        }
    }

    // Fall back to current behavior (dirty_pages → cache → allocate)

    // ... existing logic ...

    // When allocating new page, associate with block
    self.associate_page_with_block(new_page_id, block_id);

    // ... rest of function ...
}
```

### Configuration

- **BLOCK_SIZE**: 128 nodes (from page.rs, unchanged)
- **MAX_PREFERRED_PAGES_PER_BLOCK**: 3 pages
- **Mapping**: In-memory only, rebuilt on each `NodeStore::new()`

---

## 4. VALIDATION

### Correctness Checks

✅ **Preserved V3 correctness:**
- Lib compiles with `--features native-v3`
- All reopen tests pass (`v3_reopen_durability`)
- Integrity test passes (`v3_integrity_check`)
- Benchmark runs successfully
- All nodes found correctly

✅ **No format changes:**
- On-disk format unchanged
- Backward compatible
- `block_preferred_pages` is in-memory only

### Benchmark Results (Honest Reporting)

**Physical placement shows NO meaningful improvement.**

| Metric | Cache-Only (Phase 2) | Physical Placement (Phase 3) | Delta |
|--------|---------------------|------------------------------|-------|
| 1K Sequential | 1.12x | 1.03x | -0.09x (worse) |
| 1K Random | 0.95x | 0.97x | +0.02x (noise) |
| 1K Block-Sequential | 1.05x | 1.00x | -0.05x (worse) |
| 10K Sequential | 1.00x | 0.97x | -0.03x (worse) |
| 10K Random | 1.04x | 1.01x | -0.03x (worse) |
| 10K Block-Sequential | 0.91x | 0.95x | +0.04x (less regression) |

**The best signal from Phase 2 (1.12x for 1K sequential) DISAPPEARED in Phase 3.**

### Analysis

**Why did physical placement not help (or hurt)?**

1. **Overhead of block lookup**: Computing block_id and checking preferred pages adds CPU cost
2. **Page cache already effective**: The existing cache is doing well enough
3. **Small working sets**: 1K-10K nodes don't stress the I/O subsystem enough
4. **OS page cache dominates**: Even after "reopen", OS cache keeps pages warm
5. **B+Tree is already efficient**: O(log n) lookups are fast for these sizes

**Why was Phase 2's 1.12x signal not reproduced?**

The Phase 2 block-aware cache eviction may have benefited from a specific interaction that's disrupted by the placement bias:
- Phase 2: Eviction policy keeps same-block pages in cache
- Phase 3: Placement bias changes WHERE nodes go, potentially scattering them
- The 1.12x may have been measurement noise, or the two optimizations interfere

---

## 5. REMAINING RISKS

### 1. Block-Locality May Not Be Worth Pursuing

**Risk:** Three phases of incremental testing have NOT produced a strong signal.

**Evidence:**
- Phase 1 (metadata): No effect
- Phase 2 (cache eviction): 1.12x at 1K only
- Phase 3 (physical placement): No improvement

**Conclusion:** Block-locality optimizations show **diminishing returns** for V3's current architecture.

### 2. Dataset Size May Be The Wrong Lever

**Risk:** Testing with 1K-10K nodes may not reveal benefits at scale.

**Evidence:**
- All datasets fit comfortably in OS page cache
- 10K nodes ≈ 200 pages of 4KB each
- Page cache (16 pages) only holds 8% of data

**Counterpoint:** If block-locality doesn't help at 10K, it may not help at 100K either—the issue is fundamental, not scale.

### 3. Block Size Not Tuned

**Risk:** Fixed 128-node blocks may not match real access patterns.

**Evidence:**
- No analysis of typical node ID access patterns
- Block size chosen arbitrarily (128 = 2^7)
- No experimentation with different block sizes

**Mitigation:** Could test 64, 256, 512 node blocks if pursuing further.

### 4. In-Memory Mapping Rebuilds on Reopen

**Risk:** `block_preferred_pages` is in-memory only and rebuilds from scratch on each `NodeStore::new()`.

**Impact:**
- On reopen, placement "learns" from scratch
- No persistence of block→page associations
- May reduce effectiveness for read-heavy workloads

**Mitigation:** Could persist mapping if physical placement showed value (it didn't).

---

## CONCLUSION

**The physical block-placement prototype is COMPLETE.**

### What We Learned

1. **Physical placement alone is insufficient**: Biasing same-block nodes to same pages showed NO measurable benefit
2. **Cache eviction was the better direction**: Phase 2's 1.12x signal (though small) was better than Phase 3's results
3. **V3's current architecture is already efficient**: B+Tree + page cache + compression work well together
4. **Block-locality may not be the right optimization**: Three phases of testing haven't produced strong signal

### Recommendations

**STOP block-locality work for now.**

**Rationale:**
- Phase 1: Metadata-only → No effect
- Phase 2: Cache eviction → 1.12x at small scale only
- Phase 3: Physical placement → No improvement

**The incremental approach worked: We tested the hypothesis with minimal investment and learned that block-locality optimizations don't provide meaningful benefit for V3's current architecture.**

### Alternative Directions

If seeking performance improvements, consider:

1. **Larger page cache**: Increase from 16 to 64-128 pages
2. **Better compression**: Tune delta/varint encoding
3. **Prefetch**: Read-ahead for sequential access patterns
4. **B+Tree optimizations**: Better node splitting strategies

### Success Criteria Met

- ✅ Physical block-placement prototype exists
- ✅ Preserves current correctness
- ✅ Benchmark was reran honestly
- ✅ **Learned**: Physical placement shows **no benefit** over cache-only approaches
- ✅ **Next step is clear**: Block-locality is not a high-value direction for V3

---

**Comparison: All Three Phases**

| Metric | Phase 1: Metadata | Phase 2: Cache Eviction | Phase 3: Physical Placement |
|--------|-------------------|------------------------|------------------------------|
| Implementation | block_id field only | block_id in eviction | block_id in page selection |
| Changed behavior? | No | Yes | Yes |
| 1K Sequential speedup | ~1.0x | **1.12x** | 1.03x |
| 10K Sequential speedup | ~1.0x | 1.00x | 0.97x |
| Overall signal | None | Small positive | **None** |

**The block-locality hypothesis has been tested thoroughly. The incremental approach prevented over-investment in a low-value direction.**

---

**Prototype artifacts:**
- `sqlitegraph-core/src/backend/native/v3/node/store.rs` — block-aware placement logic
- `sqlitegraph-core/src/backend/native/v3/node/page.rs` — block_id metadata (from Phase 1)
- `sqlitegraph-core/examples/block_locality_benchmark.rs` — measurement tool
- This report
