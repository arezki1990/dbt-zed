# Page ID Collision - Root Cause of V3 Corruption

**Date:** 2026-03-11
**Status:** ROOT CAUSE IDENTIFIED

---

## The Bug

**Page ID collision between node storage and edge storage.**

### Node Storage Page IDs

NodeStore uses PageAllocator which allocates pages starting from page_id=2:
- First node page: page_id = 2
- Second node page: page_id = 3
- ...continues sequentially

### Edge Storage Page IDs

V3EdgeStore calculates page IDs using this formula (`edge_compat.rs:430`):
```rust
let page_id = (src as u64) * 2 + if dir == Direction::Outgoing { 100 } else { 200 };
```

For a 10K node database:
- Node 1 outgoing: page_id = 1*2 + 100 = 102
- Node 2 outgoing: page_id = 2*2 + 100 = **104** ← COLLISION!
- Node 3 outgoing: page_id = 3*2 + 100 = 106 ← COLLISION!
- ...and so on

### The Collision

With 10K nodes:
- Node storage allocates ~200 pages (10K nodes / ~50 nodes per page)
- Edge storage for node 2 calculates page_id=104
- **Page 104 is used by both node storage AND edge storage!**

When edge data is written to page 104, it overwrites whatever node data was there.
When node data is written to page 104, it overwrites whatever edge data was there.

**Result: Data corruption depending on which writes last.**

---

## Why Page 104+ Shows Corruption

The forensic analysis showed corruption starting at page 104 with "TestEdge" strings.

This is because:
1. Node pages fill up sequentially (pages 2-100+)
2. When edges are written for node 2+, they calculate page_id=104+
3. Edge data ("TestEdge" strings) gets written to page 104
4. This OVERWRITES the node page header that was at page 104

---

## The Fix

Edge storage needs to use page IDs that don't conflict with node storage.

**Option 1:** Allocate edge pages through PageAllocator instead of calculating them
**Option 2:** Use a different base offset for edge pages (e.g., start at 1,000,000)
**Option 3:** Reserve a separate page ID range for edges (e.g., 100,000-199,999)

The correct fix is **Option 1**: Edge pages should be allocated through PageAllocator,
just like node pages. The current formula-based approach is fundamentally broken.

---

## Files to Fix

1. `src/backend/native/v3/edge_compat.rs` - Line 430 (page_id calculation)
2. `src/backend/native/v3/edge_compat.rs` - Line 708 (write_page_to_disk offset calculation)
3. Any other place that calculates edge page IDs using the formula

---

## Validation

After fix:
- Node storage uses pages 2-N (allocated by PageAllocator)
- Edge storage uses pages N+1-M (also allocated by PageAllocator)
- No collisions possible because PageAllocator tracks all allocations
