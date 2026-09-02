# Reopen Corruption Bug - Forensic Analysis

**Date:** 2026-03-11
**Status:** BUG CONFIRMED AND REPRODUCED
**Severity:** CRITICAL - V3 cannot reliably reopen databases

---

## Reproduction

**Error:** `Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 21605 > 4096`

**Steps to reproduce:**
1. Create V3 database
2. Insert 10K nodes, 50K edges
3. Flush to disk
4. Close database
5. Attempt to reopen → **CRASH**

---

## Forensic Data

### Corruption Scope
- **File size:** 82,735,326 bytes (20,199 pages)
- **Corrupted pages:** 31 out of 20,199 (~0.15%)
- **Corruption pattern:** String/JSON data leaking into page header

### Corrupted Page Examples

| Page ID | used_bytes value | Hex | ASCII Interpretation |
|---------|------------------|-----|----------------------|
| Page 2 | 31,522 | 0x7b22 | `{"` (JSON start) |
| Page 19 | 25,701 | 0x6465 | `de` (JSON delimiter) |
| Pages 21,22,24,26,28,29,31,34... | 28,271 | 0x6e6f | `no` (ASCII string) |

### Pattern Analysis

**The value 0x6e6f ("no") appearing in 9+ pages is highly suspicious:**

1. **This is string data**, not a valid used_bytes count (max should be 4064)
2. **The repetition suggests:** Data being written to wrong memory location
3. **Likely scenario:** Page data is being written at wrong offset during pack/write

### First Page Header (Valid)

```
Offset 0-7:   [83, 81, 76, 84, 71, 70, 0, 3]  ("SQLTf\x03" - magic)
Offset 8-15:  [0, 0, 0, 4, 0, 0, 0, 7]          (page_id, next_page_id)
Offset 16-19: [0, 0, 0, 0]                        (node_count=0, used_bytes=0)
Offset 20-23: [0, 0, 39, 16]                     (base_id=10000)
```

First page header is valid. Corruption starts at page 2+.

---

## Root Cause Hypothesis

### Most Likely: Offset Calculation Error in Page Write

**Hypothesis:** When writing node data to the page buffer, the code is writing to the wrong offset, causing data to spill into the page header region.

**Evidence:**
- The ASCII-readable values (0x6e6f = "no", 0x7b22 = "{") suggest string data from node names/JSON
- These values appear in the used_bytes field (offset 18-19)
- The corruption affects only some pages (31 out of 20,199)

**Code paths to investigate:**

1. **`NodePage::pack()` in `node/page.rs`** (line 637-687)
   ```rust
   let data_offset = PAGE_HEADER_SIZE;  // 32
   bytes[data_offset..data_offset + node_data.len()].copy_from_slice(&node_data);
   ```
   If `node_data.len()` is too large or `data_offset` is wrong, this could overwrite the header.

2. **File write operations** - If write offset is miscalculated, data could be written to wrong location

3. **WAL replay** - If WAL replay uses wrong offset, it could corrupt pages

### Secondary Hypothesis: Byte Order Issue

**Hypothesis:** used_bytes is being stored as little-endian but read as big-endian (or vice versa).

**Evidence against:**
- First page shows used_bytes = 0, which is same in both endianns
- The value 28271 (0x6e6f) doesn't make sense as a byte-swapped version of a reasonable value

---

## Investigation Plan

### Step 1: Validate Page Pack Logic

Add forensics to `NodePage::pack()`:
```rust
// Before writing, validate all header fields
assert!(actual_used_bytes <= USABLE_SIZE);
assert!(PAGE_HEADER_SIZE + actual_used_bytes <= MAX_PAGE_SIZE);
```

### Step 2: Trace File Writes

Add forensics to all file write operations:
- Log write offset and length
- Validate offset + length doesn't cross page boundaries
- Check for overlapping writes

### Step 3: Check WAL Replay

If WAL is used for reopen:
- Verify WAL records contain correct page data
- Check WAL replay uses correct offsets
- Validate checksums before and after WAL replay

### Step 4: Add Page Header Validation on Write

Add checksum validation immediately after pack():
```rust
let packed = self.pack()?;
// Re-unpack to validate
let validated = NodePage::unpack(&packed)?;
```

---

## Immediate Workaround

**For users affected by this bug:**
- Use smaller datasets (<5K nodes) - corruption appears less frequent
- Avoid reopen cycles - keep connection open
- Don't use V3 for data that must persist

**For developers:**
- Do not recommend V3 for general workloads until this is fixed
- DO NOT use V3 for any critical data
- Add reopen stress tests to CI

---

## Next Steps

1. **Add forensics** to track page pack/write operations
2. **Create minimal reproduction** with fewer pages to debug faster
3. **Fix root cause** once identified
4. **Add validation** to prevent future corruption

---

**Test artifact:**
- `examples/reopen_corruption_repro.rs` - Reproduction script
- This report
