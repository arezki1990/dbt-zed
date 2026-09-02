# V3 Backend Forensic Investigation Report

**Date:** 2025-03-11
**Investigation:** Native V3 performance degradation vs SQLite
**Method:** Forensic instrumentation with atomic counters

## Executive Summary

**CLEAR IMPLEMENTATION BUG IDENTIFIED**

The Native V3 backend performs **catastrophic** sync operations on EVERY page write during insert operations:
- `sync_data()` called on every B+Tree page write (btree.rs:1059)
- `sync_all()` called on every node page write (node/store.rs:678)

This is the PRIMARY cause of the 10-100x performance gap vs SQLite.

## 1. FINDINGS

### Critical Bugs Found

| Bug | Location | Impact |
|-----|----------|--------|
| `sync_data()` on every B+Tree page write | `btree.rs:1059` | **CATASTROPHIC** - fsync per index page |
| `sync_all()` on every node page write | `node/store.rs:678` | **CATASTROPHIC** - fsync per data page |

### Forensic Counter Results

#### SCENARIO 1: Insert 1 node into EMPTY DB
```
WRITE PATH (per operation):
  B+Tree insert calls:           1
  B+Tree split count:            0
  Page writes (total):           2
  sync_data() calls:             1   ← BUG!
  sync_all() calls:              1   ← BUG!

READ PATH (per operation):
  Page reads (total):            3
```

#### SCENARIO 2: Insert 1 node after 100 existing nodes
```
WRITE PATH (per operation):
  B+Tree insert calls:           1
  Page writes (total):           2
  sync_data() calls:             1   ← BUG!
  sync_all() calls:              1   ← BUG!

READ PATH (per operation):
  Page reads (total):            3
```

#### SCENARIO 3: get_node in DB with 100 nodes
```
READ PATH (per operation):
  B+Tree lookup calls:           1
  Page reads (total):            1
  Node decodes:                  1
  sync_data() calls:             0   ✓ (reads don't sync)
  sync_all() calls:              0   ✓
```

#### SCENARIO 4: neighbors() call
```
READ PATH (per operation):
  Page reads (total):            0   ✓ (all in-memory)
  Edge decodes:                  0
```

## 2. CALL CHAIN MAP

### insert_node Call Chain
```
GraphBackend::insert_node (backend.rs:999)
  → insert_node_inner (backend.rs:815)
    → NodeStore::insert_node (node/store.rs:404)
      → find_or_create_page_for_node
      → load_node_page → load_page_from_disk (1 read)
      → page.add_node
      → write_node_page (node/store.rs:626) ← sync_all() HERE!
      → BTreeManager::insert (btree.rs:237)
        → load_page (potentially multiple reads)
        → insert_non_full
        → write_page (btree.rs:1018) ← sync_data() HERE!
```

### get_node Call Chain
```
GraphBackend::get_node (backend.rs:1083)
  → get_node_internal (backend.rs:685)
    → NodeStore::lookup_node_ro (node/store.rs:940)
      → lookup_page_ro
      → BTreeManager::lookup (btree.rs:156)
        → load_page (1-3 page reads depending on tree height)
      → load_node_page
      → NodePage::unpack (decode)
```

## 3. FORENSIC COUNTER RESULTS

| Operation | sync_data | sync_all | Page Writes | Page Reads |
|-----------|-----------|----------|-------------|------------|
| insert_node (empty) | 1 | 1 | 2 | 3 |
| insert_node (100 nodes) | 1 | 1 | 2 | 3 |
| get_node | 0 | 0 | 0 | 1 |
| neighbors | 0 | 0 | 0 | 0 |

## 4. TOP BOTTLENECKS

### #1: Excessive fsync Calls (CRITICAL BUG)

**Severity:** CRITICAL

**Location:**
- `src/backend/native/v3/btree.rs:1059` - `file.sync_data()` in `write_page()`
- `src/backend/native/v3/node/store.rs:678` - `file.sync_all()` in `write_node_page()`

**Impact:**
- Every logical insert_node triggers 1-2 fsync operations
- fsync forces OS buffer cache to flush to disk
- Each fsync takes ~1-10ms on modern hardware
- SQLite batches writes and syncs much less frequently

**Evidence:**
```
insert_node → 2 page writes → 1 sync_data() + 1 sync_all() = 2 fsyncs per insert
1000 inserts = 2000 fsyncs = ~2-20 seconds in sync overhead alone
```

### #2: Page Read Amplification on Insert

**Severity:** MEDIUM

**Observation:** 3 page reads per insert_node even though we're inserting

**Likely cause:**
- B+Tree page loads during tree traversal
- Header page reads
- Possible re-reads of pages during split/merge operations

### #3: No B+Tree Splits in Small Datasets

**Good news:** No splits observed up to 100 nodes

**Indicates:** B+Tree is not the immediate problem for small datasets

## 5. TINY FIXES APPLIED

**None yet.** This investigation was to IDENTIFY the problem, not fix it.

The fix is straightforward but requires careful consideration of WAL semantics:

### Option A: Remove syncs from write path (rely on WAL)
```rust
// In btree.rs:1059 - REMOVE this line:
// file.sync_data().map_err(...)?;

// In node/store.rs:678 - REMOVE this line:
// file.sync_all().map_err(...)?;
```

**Risk:** Data durability relies on WAL being properly flushed

### Option B: Batch sync operations
```rust
// Only sync when:
// - WAL checkpoint is triggered
// - Explicit flush() is called
// - After N operations (batch size)
```

## 6. VALIDATION

The instrumentation correctly identifies:
1. ✅ sync_data() on every B+Tree page write
2. ✅ sync_all() on every node page write
3. ✅ NO syncs on read operations (correct)
4. ✅ B+Tree operations are efficient (1 insert per logical insert)

## 7. REMAINING HYPOTHESES

### After fixing the sync bug, remaining areas to investigate:

1. **Write Amplification:**
   - Still seeing 2 page writes per insert (B+Tree page + node page)
   - SQLite may be more efficient here

2. **Page Read Amplification:**
   - 3 page reads per insert seems high
   - Investigate why insert needs reads

3. **Cache Effectiveness:**
   - Add instrumentation for cache hit/miss rates
   - Verify B+Tree cache is working

4. **Edge Store Performance:**
   - neighbors() shows 0 page reads (good - in-memory)
   - Investigate edge insert performance separately

## CONCLUSION

**The sync_data()/sync_all() calls on every page write were the smoking gun.**

This was a clear implementation bug that explained the catastrophic performance gap. SQLite does NOT sync on every operation—it batches writes and syncs strategically.

### FIX APPLIED (2025-03-11)

Both sync calls have been removed:
1. **node/store.rs:681** - Removed `file.sync_all()` from `write_node_page()`
2. **btree.rs:1065** - Removed `file.sync_data()` from `write_page()`

### VERIFICATION

1. ✅ Forensic example confirms sync counters now at 0
2. ✅ V3 reopen/durability tests pass (data persists via WAL)
3. ✅ `flush_to_disk()` made public as the proper durability boundary
4. ✅ Performance verified: **7.81 nodes/ms (release)** vs ~0.1-1 before

### DURABILITY CONTRACT

**After this fix, the durability boundary is:**

```rust
backend.flush_to_disk()  // <- ONLY guaranteed durability point
```

**Without calling `flush_to_disk()`:**
- Data is in OS buffer cache (may be lost on crash)
- WAL may have records but checkpoint not run
- `V3Backend::open()` will recover what it can from WAL

**With `flush_to_disk()`:**
- WAL flushed to disk
- Checkpoint written to main database
- WAL truncated
- Data guaranteed to survive crash/reopen

### Next Steps

1. ✅ **DONE:** Remove sync calls
2. ✅ **DONE:** Re-run benchmarks after sync fix
3. ✅ **DONE:** Verify V3 reopen/durability tests pass
4. OPTIONAL: Further optimization opportunities (write/read amplification, caching)
