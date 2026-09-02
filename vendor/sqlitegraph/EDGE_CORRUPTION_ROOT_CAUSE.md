# EDGE CORRUPTION BUG - ROOT CAUSE ANALYSIS

**Date:** 2025-03-11
**Issue:** Edge-heavy corruption in scenarios B and D
**Status:** ROOT CAUSE IDENTIFIED

---

## OBSERVE - Findings

### Error Messages
1. **Minimal test (10 nodes + 20 edges)**:
   ```
   "Invalid header field 'node.id_delta': invalid varint encoding for ID delta"
   ```

2. **Scenario B/D (10K nodes + 50K edges)**:
   ```
   "Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 25448 > 4096"
   ```

### Error Location
Both errors occur in `NodePage::unpack()` during `V3Backend::open()` when rebuilding indexes:
- `rebuild_indexes()` iterates through all node IDs
- Calls `get_node_internal()` → `lookup_node_ro()` → B+Tree lookup
- `load_page_cache_ro()` loads the page
- `NodePage::unpack()` tries to deserialize as node page
- **FAILS** because page contains edge data, not node data

---

## ROOT CAUSE

### NodeStore and EdgeStore Share the Same B+Tree

In `V3Backend::open()` (lines 530-572):

```rust
// NodeStore's BTreeManager
let mut btree = BTreeManager::with_root_and_cache(
    Arc::clone(&allocator),
    None,
    header.root_index_page,  // <-- SAME root!
    header.btree_height,
    ...
);

// EdgeStore's BTreeManager
let edge_store = V3EdgeStore::with_path_and_allocator(
    BTreeManager::with_root_and_cache(
        Arc::clone(&allocator),
        None,
        header.root_index_page,  // <-- SAME root!
        header.btree_height,
        ...
    ),
    ...
);
```

### Why This Causes Corruption

1. **NodeStore** inserts: `node_id → node_page_id` mappings into B+Tree
2. **EdgeStore** inserts: `edge_key(src, dir) → edge_page_id` mappings into SAME B+Tree
3. B+Tree now contains MIXED entries from both stores
4. During reopen, NodeStore's B+Tree lookup can return edge_page_ids
5. `NodePage::unpack()` tries to read edge data as node page → CORRUPTION

### Edge Key Function
```rust
// edge_compat.rs line 352
fn edge_key(src: i64, dir: Direction) -> u64 {
    ((src as u64) << 1) | (if dir == Direction::Outgoing { 0 } else { 1 })
}
```

Examples:
- edge_key(1, Outgoing) = 2
- edge_key(2, Outgoing) = 4
- edge_key(10, Outgoing) = 20

Node IDs: 1, 2, 3, 4, 5, ...

The B+Tree key ranges can overlap!

### Page Format Mismatch

**Edge page format** (edge_compat.rs line 214):
```
[version: 1 byte][edge_count: 4 bytes][edge records...]
```

**Node page format** (page.rs line 690):
```
[PAGE_HEADER: 32 bytes][delta/varint encoded nodes...]
```

When edge data is read as node page:
- Bytes at offset 18-19 (USED_BYTES_OFFSET) contain edge data
- Interpreted as `used_bytes` → bogus value like 25448
- OR varint decoder fails on edge data → "invalid varint encoding"

---

## FIX APPROACH

### Give Each Store Its Own B+Tree

1. **Add separate root fields to PersistentHeaderV3**:
   ```rust
   pub struct PersistentHeaderV3 {
       ...
       pub node_index_page: u64,   // B+Tree root for node_id → page_id
       pub edge_index_page: u64,   // B+Tree root for edge_key → page_id
       pub node_btree_height: u32,
       pub edge_btree_height: u32,
       ...
   }
   ```

2. **Initialize NodeStore with node_index_page**:
   ```rust
   BTreeManager::with_root_and_cache(
       ...,
       header.node_index_page,
       header.node_btree_height,
       ...
   )
   ```

3. **Initialize EdgeStore with edge_index_page**:
   ```rust
   BTreeManager::with_root_and_cache(
       ...,
       header.edge_index_page,
       header.edge_btree_height,
       ...
   )
   ```

4. **Update flush logic to track both roots separately**

---

## VALIDATION

### Reproduction Test
Created `tests/edge_corruption_minimal.rs`:
- 10 nodes + 20 edges
- Flushes and reopens
- Fails during `V3Backend::open()`

Expected after fix:
- Test passes
- NodeStore and EdgeStore have separate B+Trees
- No page type confusion

---

## IMPLEMENTATION PLAN

1. Modify `PersistentHeaderV3` to have separate index roots
2. Update `V3Backend::create()` to allocate separate roots
3. Update `V3Backend::open()` to use separate roots
4. Update flush logic to track both roots
5. Add tests to verify B+Tree separation

---

## FILES TO MODIFY

1. `src/backend/native/v3/header.rs` - Add node_index_page, edge_index_page fields
2. `src/backend/native/v3/backend.rs` - Use separate roots during create/open/flush
3. `src/backend/native/v3/edge_compat.rs` - Track edge B+Tree root separately
4. `src/backend/native/v3/node/store.rs` - Track node B+Tree root separately

---

## RELATED ISSUES

- Node overflow bug (Scenario C): FIXED
- Edge corruption bug (Scenarios B/D): ROOT CAUSE IDENTIFIED, FIX IN PROGRESS
