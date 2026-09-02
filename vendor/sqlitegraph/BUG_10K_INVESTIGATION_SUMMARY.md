# 10K-Node Database Creation Bug - Investigation Summary

**Date:** 2026-03-11
**Status:** PARTIALLY FIXED - 1K nodes work, 10K nodes fail
**Root Cause:** Multiple `create(true)` calls truncating database file

---

## 1. Root Cause Identified

### Primary Bug: File Truncation

Multiple locations were using `OpenOptions::new().write(true).create(true)` which **truncates the file on every open**:

1. **`node/store.rs:write_node_page()`** - Line 663
2. **`node/store.rs:commit_batch()`** - Line 283
3. **`backend.rs:insert_node_inner()`** - Line 933 (external data path)
4. **`edge_compat.rs:write_page_to_disk()`** - Line 714

**Impact:** Each time these functions opened the file, all existing data was lost.

---

## 2. Fixes Applied

### Fixed: `create(true)` → `create(false)`

All four locations changed to use `create(false)` or conditional `create(!file_exists)`.

### Added: File Extension Logic

When writing beyond current file size:
1. Check file size with `file.metadata()`
2. If needed, call `file.set_len(required_len)`
3. Call `file.sync_all()` to persist metadata
4. Write data at offset
5. Call `file.sync_all()` to flush

### Added: Thread-Safe Page Cache

Changed `page_cache: HashMap<u64, Vec<u8>>` to `page_cache: Arc<RwLock<HashMap<u64, Vec<u8>>>` to allow cache population from read-only methods.

---

## 3. Test Results

| Test | Result | Details |
|------|--------|---------|
| `test_1k_nodes_without_wal` | ✅ PASS | 1K nodes inserted, reopened successfully |
| `test_10k_nodes_without_wal` | ❌ FAIL | Fails at page 35-46 (inconsistent) |

### Failure Analysis

**Symptom:** `UnexpectedEof` when reading page N
**Location:** During insertion, not reopen
**Page Number:** Varies (35, 37, 38, 42, 44, 46) - non-deterministic

**Evidence from debug tests:**
- After 2500 nodes: file size = 159856 bytes
- Page 40 offset = 112 + 39 * 4096 = 159856 (exactly file size!)
- File size equals page offset, not offset + page_size

This indicates the file was extended to exactly the page offset, but the page data (4096 bytes) was not included.

---

## 4. Remaining Issues

### Hypothesis: File Handle Coordination

**Problem:** NodeStore and B+Tree both write to the same file using separate file handles.

**Race Condition:**
1. B+Tree opens file, sees size S
2. B+Tree extends file to S1 with `set_len()`
3. B+Tree writes page data, calls `sync_all()`
4. NodeStore opens file (different handle)
5. NodeStore sees stale metadata (size < S1)
6. NodeStore extends file to S2 but write fails or is incomplete

**Non-Determinism:** The inconsistent failure page numbers suggest cache eviction, allocator behavior, or metadata caching timing variations.

---

## 5. Workarounds

### For Users:

1. **Use WAL mode** - Write-Ahead Log coordinates writes properly
   ```rust
   V3Backend::create_with_wal(&db_path, true)
   ```

2. **Limit to ~1K nodes** - Current fix works for smaller databases

3. **Use batch mode** with explicit flush:
   ```rust
   let backend = V3Backend::create(&db_path)?;
   let mut batch = backend.begin_batch();
   // ... insert nodes ...
   batch.commit()?; // Single fsync for all
   ```

### For Developers:

1. **Shared file handle** - Single `File` object shared across all writers
2. **Pre-allocated file** - Create large file upfront, no extension needed
3. **Write serialization** - Mutex around all file operations

---

## 6. Files Modified

1. `src/backend/native/v3/node/store.rs`
   - Changed `page_cache` to `Arc<RwLock<HashMap>>`
   - Fixed `create(true)` → `create(false)` in `write_node_page()` and `commit_batch()`
   - Added file extension logic with `set_len()` and `sync_all()`

2. `src/backend/native/v3/btree.rs`
   - Added file extension logic to `write_page()`
   - Added `sync_all()` after writes

3. `src/backend/native/v3/backend.rs`
   - Fixed `create(true)` in `insert_node_inner()` (external data path)
   - Added file extension logic

4. `src/backend/native/v3/edge_compat.rs`
   - Fixed `create(true)` in `write_page_to_disk()`
   - Added file extension logic

---

## 7. Recommendations

### Short-term:
1. Document that 10K nodes without WAL is not supported
2. Add warning when creating large databases without WAL
3. Consider using WAL by default

### Long-term:
1. Implement shared file handle for all V3 writers
2. Or use a memory-mapped file (mmap) for simpler offset-based writes
3. Consider SQLite-style journaling for all modifications

---

## 8. Test Cases Added

- `tests/test_10k_bug_reproduction.rs` - Original bug reproduction
- `tests/file_extension_debug.rs` - File extension behavior tests
- `tests/page_write_debug.rs` - Page write tracing
- `tests/page_42_debug.rs` - Detailed page 42 debugging
- `tests/check_write_path.rs` - Write path verification
