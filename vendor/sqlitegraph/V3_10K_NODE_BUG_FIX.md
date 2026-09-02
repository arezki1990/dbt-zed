# 10K-Node Bug Fix - Native V3 Backend

## Problem

The native V3 backend was failing when inserting 10K nodes without WAL mode, with the error:
```
UnexpectedEof: Failed to read page X from disk
```

The failure occurred at non-deterministic page numbers (35-52 range).

## Root Cause

The bug was in `NodeStore::allocate_page_if_needed()`:

```rust
// BEFORE (BUGGY CODE):
let new_page_id = allocator.allocate()?;
let new_page = NodePage::new(new_page_id);
let page_bytes = new_page.pack()?;
self.page_cache_insert(new_page_id, page_bytes.to_vec()); // Only added to cache!
Ok(new_page_id)  // Page ID returned, but page NOT on disk!
```

The problem flow:
1. **Page allocated** in memory, added to cache only
2. **B+Tree updated** to reference the new page (via `btree.insert(node_id, page_id)`)
3. **Page evicted from cache** due to cache size limits (16 pages)
4. **Read attempted** via B+Tree traversal
5. **UnexpectedEof** because page doesn't exist on disk!

## Solution

Write newly allocated pages to disk **before** updating the B+Tree index:

```rust
// AFTER (FIXED CODE):
let new_page_id = allocator.allocate()?;
let new_page = NodePage::new(new_page_id);
let page_bytes = new_page.pack()?;

// CRITICAL: Write to disk BEFORE B+Tree update
if let Some(coordinator) = &self.file_coordinator {
    coordinator.write_page(new_page_id, &page_bytes)?;
} else {
    // Fallback path...
}

// Now add to cache
self.page_cache_insert(new_page_id, page_bytes.to_vec());
Ok(new_page_id)
```

## Changes Made

### 1. `src/backend/native/v3/node/store.rs`
- Modified `allocate_page_if_needed()` to write new pages to disk immediately after allocation
- Uses `FileCoordinator` when available for coordinated I/O

### 2. `src/backend/native/v3/file_coordinator.rs`
- Added validation in `read_page()` to check file size before reading
- Updated `write_page()` to use actual file size from `metadata()` instead of cached size

## Why This Fix Is Correct

The fix ensures the invariant:
**Any page ID in the B+Tree index must have a corresponding page on disk.**

By writing the page to disk before the B+Tree is updated:
1. The page exists on disk before any reference to it is published
2. Reads via the B+Tree will always find the page
3. Cache eviction doesn't cause UnexpectedEof

## Testing

The fix was verified by:
1. Running `test_10k_nodes_without_wal` 5 consecutive times - all passed
2. Running all tests in `test_10k_bug_reproduction.rs` - all passed
3. Previously this test would fail consistently at varying page numbers

## Performance Impact

Minimal - the fix adds one disk write per page allocation:
- Before: Page write happened later (batch commit or when page full)
- After: Page write happens immediately at allocation
- Net effect: Same number of writes, just earlier in the lifecycle

This actually improves correctness with negligible performance impact since:
- Empty pages are small (4KB)
- Writes are amortized across many node insertions
- The `FileCoordinator` ensures coordinated I/O across all components
