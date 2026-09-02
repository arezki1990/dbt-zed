# V3 REGRESSION SWEEP REPORT

**Date:** 2025-03-11
**Test File:** `tests/v3_regression_sweep.rs`
**Purpose:** Health check / regression sweep after multiple corruption fixes

---

## 1. FINDINGS

### CRITICAL: ACTIVE CORRUPTION BUGS DETECTED

The regression sweep revealed **3 active corruption bugs** in Native V3:

1. **Node Page Overflow During Insert** (Scenario C)
   - Error: `"Invalid header field 'node_page': page overflow: header 32 + data 4065 > 4096"`
   - Trigger: Inserting nodes with larger payloads (name patterns + JSON data)
   - Location: During insert phase, before flush

2. **Node Page Corruption On Reopen** (Scenarios B, D)
   - Error: `"Invalid header field 'node_page': used_bytes exceeds page boundary: 32 + 25448 > 4096"`
   - Trigger: After inserting 50K edges, then reopening
   - Location: During V3Backend::open()

3. **Page Boundary Validation Enforcement Bug**
   - The error messages indicate page boundary validation is catching corruption
   - But the corruption is being CREATED by the backend, not external factors

---

## 2. REGRESSION MATRIX

| Scenario | Dataset | Result | Error Location |
|----------|---------|--------|----------------|
| A | 10K nodes, 10 kinds | **PASS** | N/A |
| B | 10K nodes + 50K edges | **FAIL** | V3Backend::open() |
| C | 10K nodes + 20 kinds + 15 name patterns | **FAIL** | Insert phase |
| D | 10K nodes + 50K edges + 10 kinds | **FAIL** | V3Backend::open() |
| Repeat | 10K nodes × 3 runs | **PASS** | N/A |

**Pass Rate:** 40% (2/5 scenarios)

---

## 3. REOPEN/INTEGRITY RESULTS

### Scenario A (PASSED)
- **Before close:** File: 55562352 bytes, root_page=258, height=2, nodes=10000, zero_page_errors=0
- **After reopen:** File: 55562352 bytes, root_page=258, height=2, nodes=10000, zero_page_errors=0
- **All 10 kind queries worked:** 1000 nodes each
- **Sample nodes verified:** Correct

### Scenario B (FAILED)
- **Before close:** File: 104960112 bytes, root_page=25507, height=2, nodes=10000, zero_page_errors=0
- **On reopen:** `"Invalid header field 'node_page': used_bytes exceeds page boundary"`
- **Issue:** Node page corruption manifesting after edge insertion

### Scenario D (FAILED)
- **Before close:** File: 114909296 bytes, root_page=27930, height=2, nodes=10000, zero_page_errors=0
- **On reopen:** `"Invalid header field 'node_page': used_bytes exceeds page boundary"`
- **Issue:** Same as Scenario B - edge-related node page corruption

---

## 4. REPEATED-RUN STABILITY RESULTS

**Test:** 3 runs of 10K nodes with simple payloads

| Run | File Size | Root Page | Node Count | Zero Page Errors |
|-----|-----------|-----------|------------|------------------|
| 1 | 28397680 bytes | 7 | 10000 | 0 |
| 2 | 30560368 bytes | 7 | 10000 | 0 |
| 3 | 28397680 bytes | 7 | 10000 | 0 |

**Status:** STABLE - All runs completed successfully

**Note:** File size variance between runs (28MB vs 30MB) suggests some nondeterminism in page allocation, but no data corruption.

---

## 5. REMAINING RISKS

### HIGH PRIORITY

1. **Page Overflow Bug (CRITICAL)**
   - **Symptom:** Nodes with larger payloads cause page overflow
   - **Impact:** Cannot use V3 for real workloads with realistic data sizes
   - **Likely cause:** Node serialization exceeding PAGE_SIZE (4096) without proper splitting

2. **Edge-Induced Node Corruption (CRITICAL)**
   - **Symptom:** After inserting 50K edges, node pages become corrupt on reopen
   - **Impact:** Any graph with significant edge relationships is vulnerable
   - **Likely cause:** Edge metadata writing into node pages, or node page ID corruption

3. **Header Field Validation**
   - **Symptom:** Error message mentions "Invalid header field 'node_page'"
   - **Likely cause:** Either (a) node page header is being corrupted, or (b) validation logic is too strict for valid edge cases

### MEDIUM PRIORITY

4. **File Size Nondeterminism**
   - 28MB vs 30MB for identical 10K-node inserts suggests page allocation variance
   - May indicate fragmentation or allocation policy issues

### LOW PRIORITY

5. **Edge Type Overwrite Warnings**
   - "WARNING: V3EdgeStore inserting edge_type 'random' for (1668, 1670, Outgoing), overwriting existing type 'branch'"
   - This is documented behavior, but may cause data inconsistency

---

## 6. RECOMMENDED NEXT STEPS

### IMMEDIATE (Before any optimization work)

1. **Fix Page Overflow Bug**
   - Add node splitting when serialized size exceeds PAGE_SIZE
   - Or add proper error handling when node is too large for single page

2. **Fix Edge-Induced Corruption**
   - Investigate why inserting edges corrupts node pages
   - Check if edge IDs are being written into node page space
   - Verify node page read/write boundaries

3. **Add Smaller Regression Tests**
   - Create minimal reproducers for each bug
   - Binary search to find exact failure threshold

### BEFORE MARKING "VALIDATED"

4. **Fix all HIGH PRIORITY bugs above**
5. **Re-run this regression sweep - must pass 100%**
6. **Add 100K node tests** (10x larger than current)
7. **Test concurrent operations**
8. **Test crash recovery**

---

## CONCLUSION

**Native V3 is NOT validated for the documented paths.**

The regression sweep successfully detected critical page boundary and corruption bugs that prevent:
- Nodes with realistic payload sizes
- Graphs with significant edge relationships
- Reliable reopen after edge insertion

**These bugs must be fixed before proceeding to optimization work.**
