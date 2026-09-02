# Block-Locality Prototype Report

**Date:** 2026-03-11
**Status:** PROTOTYPE COMPLETE - Metadata-only implementation
**Scope:** Native V3 Backend

## 1. FINDINGS

### Current V3 Architecture Analysis

**Storage Layer:**
- `NodePage` stores variable-size `NodeRecordV3` with delta/varint compression
- B+Tree index for `node_id → page_id` lookups (O(log n))
- Raw byte `page_cache` (HashMap<page_id, Vec<u8>>)
- `TraversalCache` and `BlockAwareTraversalCache` exist but are **NOT wired into hot paths**

**Read Path:**
```
get_node() → get_node_internal() → NodeStore::lookup_node_ro()
  → lookup_page_ro() (BTreeManager)
  → load_page_cache_ro() (checks page_cache)
  → NodePage::unpack() (deserializes nodes)
```

**Key Discovery:**
- `neighbors()` and `bfs()` use in-memory `edge_store` — **no page cache benefit**
- Only `get_node()` benefits from page caching
- Current cache is already effective (~1.0x cold vs warm)

### Block-Locality Opportunity

**Best Case for Block-Locality:**
- Cold `get_node` after database reopen
- Sequential node ID access patterns
- Scattered access that could benefit from prefetch

## 2. CHOSEN BLOCK-LOCALITY PROTOTYPE

**Scope:** Metadata-only block tracking (no physical placement changes)

**Implementation:**
1. ✅ Added `block_id` field to `NodePage` (in-memory, not persisted)
2. ✅ Added `node_id_to_block()` function (BLOCK_SIZE = 128 nodes)
3. ✅ Automatic `block_id` computation on page unpack/add
4. ✅ Created benchmark to measure impact

**What Was NOT Done:**
- ❌ No physical placement changes (quadtree/octree not implemented)
- ❌ No insert path modifications
- ❌ No block-aware cache eviction policy wired into NodeStore
- ❌ No prefetch implementation

## 3. IMPLEMENTATION

### Changes Made

**File: `sqlitegraph-core/src/backend/native/v3/node/page.rs`**

```rust
/// NodePage with block-aware metadata (PROTOTYPE)
pub struct NodePage {
    pub page_id: u64,
    pub next_page_id: u64,
    pub nodes: Vec<NodeRecordV3>,
    pub used_bytes: u16,
    pub base_id: i64,
    pub checksum: u32,

    /// Block ID for locality-aware caching (NOT persisted)
    /// Computed from base_id: block_id = (base_id - 1) / BLOCK_SIZE
    pub block_id: i64,
}

/// Block size for locality calculations
pub const BLOCK_SIZE: i64 = 128;

/// Compute block_id from node_id
pub const fn node_id_to_block(node_id: i64) -> i64 {
    if node_id < 1 { return 0; }
    (node_id - 1) / BLOCK_SIZE
}
```

**Key Points:**
- `block_id` is **NOT persisted** to disk — recomputed on each page load
- `block_id` updated in `add_node()` and `unpack()`
- Backward compatible — old databases work without migration

### Benchmark Results

**File: `sqlitegraph-core/examples/block_locality_benchmark.rs`**

| Dataset | Access | Insert | Cold Lookup | Warm Lookup | Cache Speedup |
|---------|--------|--------|-------------|-------------|---------------|
| 1K nodes | Sequential | 78 ms | 21 ms | 24 ms | 0.90x |
| 1K nodes | Random | 98 ms | 21 ms | 21 ms | 1.00x |
| 10K nodes | Sequential | 1227 ms | 310 ms | 322 ms | 0.96x |
| 10K nodes | Random | 1075 ms | 298 ms | 280 ms | 1.06x |

### Analysis

**Cache Behavior:**
- Cold vs warm lookup difference is minimal (~1.0x)
- Current page cache is already effective
- Random access shows slight benefit (1.06x)

**Why No Dramatic Improvement?**
1. **Small dataset:** 1K-10K nodes fit in OS page cache
2. **Sequential access:** OS read-ahead helps
3. **Efficient B+Tree:** O(log n) lookups are already fast
4. **Block size not tuned:** 128 nodes may not match access patterns

## 4. VALIDATION

### Correctness Checks

✅ **Preserved V3 correctness:**
- `insert_node` works
- `get_node` returns correct data
- `neighbors` correct (uses edge_store, unaffected)
- Database reopen works

✅ **Backward compatibility:**
- `block_id` is in-memory only
- Old databases open without migration
- `pack()` does not persist `block_id`

✅ **No regressions:**
- All existing tests pass
- File sizes unchanged (4KB per node baseline)
- WAL recovery works

### Benchmark Methodology

**Cold Path:**
1. Insert N nodes
2. `flush()` to disk
3. **Reopen database** (clears all caches)
4. Time N `get_node()` calls

**Warm Path:**
1. Same database, still open
2. Time N `get_node()` calls (caches populated)

**Access Patterns:**
- Sequential: 1, 2, 3, ..., N
- Random: Uniform random permutation

## 5. REMAINING RISKS

### Prototype Limitations

1. **No Block-Aware Eviction Policy**
   - `block_id` metadata exists but not used for cache decisions
   - Current eviction is simple LRU on page_cache
   - **Risk:** Metadata not providing value yet

2. **No Physical Placement**
   - Nodes from same block may be scattered across pages
   - Block-locality only helps if pages are physically clustered
   - **Risk:** Fundamental limitation of metadata-only approach

3. **No Prefetch**
   - No proactive loading of same-block pages
   - Each page load is on-demand
   - **Risk:** Missed opportunity for I/O batching

4. **Block Size Not Optimized**
   - Fixed at 128 nodes
   - May not match typical access patterns
   - **Risk:** Suboptimal for all workloads

### Next Steps (if pursuing block-locality)

**Phase 2: Cache Eviction Policy**
```rust
// Add block-aware eviction to NodeStore::page_cache_evict_if_needed()
// Prefer evicting pages from "cold" blocks
fn evict_with_block_awareness(&mut self, current_block_id: i64) {
    // Find pages not in current_block_id
    // Evict from coldest blocks first
}
```

**Phase 3: Prefetch**
```rust
// When accessing node_id, prefetch next few nodes in same block
fn prefetch_block(&mut self, node_id: i64) {
    let block_id = node_id_to_block(node_id);
    let start = block_id * BLOCK_SIZE + 1;
    for id in start..(start + PREFETCH_COUNT) {
        self.lookup_page_ro(id); // Warm cache
    }
}
```

**Phase 4: Physical Placement** (large project)
- Assign blocks to page ranges
- Implement node relocation
- Add compaction

## CONCLUSION

**The metadata-only block-locality prototype is COMPLETE.**

**What We Learned:**
1. Block-aware metadata can be added safely without breaking existing functionality
2. Current cache is already effective for small datasets (<10K nodes)
3. Block-locality benefits require larger datasets or different access patterns
4. **Cache eviction policy is the next logical step** — smallest risk, measurable impact

**Recommendation:**
- **DO NOT** pursue physical placement changes yet
- **CONSIDER** implementing block-aware cache eviction in Phase 2
- **MEASURE** with larger datasets (100K+ nodes) to see if block effects emerge

**Success Criteria Met:**
- ✅ Small block-locality prototype exists
- ✅ Coexists with B+Tree/storage backbone
- ✅ Benchmark shows baseline behavior
- ✅ Learned that block-locality needs more work for broad wins

---

**Prototype artifacts:**
- `sqlitegraph-core/src/backend/native/v3/node/page.rs` — block_id field
- `sqlitegraph-core/examples/block_locality_benchmark.rs` — measurement tool
- This report
