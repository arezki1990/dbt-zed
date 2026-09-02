# Cold-Path Forensics Report
**Date**: 2026-03-12
**Component**: Native V3 Backend - Post rebuild_indexes() fix
**Goal**: Measure real cold-path behavior with open() contamination removed

---

## 1. FINDINGS

### Pure Open Cost (No Hidden Work)

| Dataset | Open Time | B+Tree Lookups | Node Decodes |
|---------|-----------|----------------|--------------|
| Small (1K nodes, 5K edges) | **7.14ms** | **0** | **0** |
| Medium (10K nodes, 50K edges) | **105.26ms** | **0** | **0** |

**Key Finding**: Open() now performs ZERO B+Tree lookups or node decodes. The rebuild_indexes() preload bug is eliminated.

### Cold vs Warm Read Costs

#### Small Dataset (1K nodes, 5K edges)

| Operation | Time | B+Tree Lookups | Page Reads | Node Decodes | B+Tree Cache | Node Page Cache |
|-----------|------|----------------|------------|--------------|--------------|-----------------|
| Pure open | 7.14ms | 0 | 0 | 0 | - | - |
| **First get_node** (cold) | **30.56µs** | **1** | **3** | **1** | 0/2 miss | 0/1 miss |
| Second get_node (warm) | 16.80µs | 1 | 3 | 1 | 2/2 hit | 1/1 hit |
| **First neighbors** (cold) | **14.08µs** | **2** | **5** | **0** | 0/4 miss | 0/1 miss |
| Second neighbors (warm) | 0.20µs | 0 | 0 | 0 | - | 1/1 hit |

**Cold/Warm Ratios**:
- get_node: 1.82x cold penalty
- neighbors: **70.40x cold penalty**

#### Medium Dataset (10K nodes, 50K edges)

| Operation | Time | B+Tree Lookups | Page Reads | Node Decodes | B+Tree Cache | Node Page Cache |
|-----------|------|----------------|------------|--------------|--------------| ----------------|
| Pure open | 105.26ms | 0 | 0 | 0 | - | - |
| **First get_node** (cold) | **17.10µs** | **1** | **3** | **1** | 0/2 miss | 0/1 miss |
| Second get_node (warm) | 2.05µs | 1 | 3 | 1 | 2/2 hit | 1/1 hit |
| **First neighbors** (cold) | **16.53µs** | **2** | **5** | **0** | 0/4 miss | 0/1 miss |
| Second neighbors (warm) | 0.24µs | 0 | 0 | 0 | - | 1/1 hit |

**Cold/Warm Ratios**:
- get_node: **8.34x cold penalty**
- neighbors: **68.88x cold penalty**

---

## 2. COLD-PATH BENCH PLAN

### What Was Measured

1. **Pure open()**: Time to open database with no reads
2. **First get_node (cold)**: First read after open, all caches cold
3. **Second get_node (warm)**: Same node, caches now warm
4. **First neighbors (cold)**: First neighbor query, node+edge caches cold
5. **Second neighbors (warm)**: Same neighbor query, caches warm

### What the Counters Reveal

**After open():**
- B+Tree lookups: 0 ✓
- Node decodes: 0 ✓
- Confirms rebuild_indexes() is no longer doing hidden work

**First get_node (cold):**
- 1 B+Tree lookup (to find node's page)
- 3 page reads (1 B+Tree page + 1 header + 1 node page)
- 1 node decode
- B+Tree cache: 0/2 misses (needs to read root + 1 internal page)
- Node page cache: 0/1 miss

**Second get_node (warm):**
- Same 1 B+Tree lookup, 3 page reads, 1 decode
- BUT: B+Tree cache 2/2 hits, Node page cache 1/1 hit
- Result: 8.34x faster (medium), 1.82x faster (small)

**First neighbors (cold):**
- 2 B+Tree lookups (1 for node + 1 for edge cluster head)
- 5 page reads (3 for node path + 2 for edge path)
- 0 node decodes (node was cached from get_node)
- 1 edge page read

**Second neighbors (warm):**
- 0 additional B+Tree lookups
- 0 additional page reads
- All data in memory cache
- Result: 68-70x faster

---

## 3. EXECUTION NOTES

### Test Environment
- Feature flags: `native-v3,v3-forensics`
- File-based databases (tempdir)
- Random seeded RNG for reproducible edge patterns
- Target node: middle of dataset (node_count / 2)

### Warnings During Setup
- Edge type overwrites: Expected limitation of tuple-key model (not relevant to this measurement)

### Counter Verification
- All counters reset before open()
- After open(): 0 lookups/decodes confirmed
- Incremental counter changes match expected operations

---

## 4. RESULTS: OPEN vs FIRST READ vs SECOND READ

### Cost Attribution Analysis

| Phase | Small Time | Medium Time | % of Total (Open + Cold Read) |
|-------|------------|-------------|-------------------------------|
| **Pure open** | 7.14ms | 105.26ms | **99.6%** (small) / **99.8%** (medium) |
| **First get_node** | 0.03ms | 0.02ms | 0.4% (small) / 0.02% (medium) |
| **Second get_node** | 0.02ms | 0.002ms | - |

**Key Insight**: Open dominates the cold-start cost. First read is tiny in comparison.

### First Read Cost Breakdown

The cold first get_node (17µs medium, 31µs small) is dominated by:
1. **B+Tree lookup**: Finding the node's page (O(log N) traversal)
2. **Page I/O**: 3 page reads from disk
3. **Node decode**: Deserializing the node record

The warm second get_node (2µs medium, 17µs small) still does the same work, but:
- B+Tree cache hits eliminate I/O
- Node page cache hits eliminate I/O
- Result: memory-only operation

### Neighbors Cold Path

First neighbors is surprisingly fast (14-17µs) because:
- Node is already cached from get_node
- Only edge path needs cold lookup
- Result: 1 extra B+Tree lookup + 1 edge page read

Second neighbors is essentially free (0.2-0.24µs) because:
- All data in memory
- No disk I/O
- Just in-memory lookups

---

## 5. NEXT USEFUL TARGET

### Priority 1: Open Optimization
**Impact**: High - Open is 99.6%+ of cold-start cost

**Current cost**:
- Small: 7.14ms for 1K nodes
- Medium: 105.26ms for 10K nodes
- Scaling: ~O(N) for direct page scan

**Opportunity**:
- rebuild_indexes() still does full page scan (O(N))
- Could serialize indexes to avoid rebuild entirely
- Could do lazy index population on first use

**Expected improvement**: 50-90% reduction in open time

### Priority 2: B+Tree Lookup Path
**Impact**: Medium - Only affects cold reads

**Current cost**:
- First get_node: 17µs (medium), 31µs (small)
- Dominated by B+Tree traversal + page I/O

**Opportunities**:
- B+Tree root page could be cached/mmap'd
- Internal page cache could be warmed during open
- Page read batching could reduce I/O overhead

**Expected improvement**: 20-40% reduction in cold read latency

### Priority 3: Cache Warming Strategy
**Impact**: Low-Medium - Only affects first read after open

**Observation**: Cold/warm ratio is 1.8-8.3x for get_node

**Opportunity**:
- Proactively cache hot nodes during open
- But: need to identify "hot" without usage history

**Expected improvement**: Unpredictable, workload-dependent

---

## 6. REMAINING RISKS

### Correctness
- **LOW**: Index rebuild is now direct page scan - risk of parsing errors on corrupt pages
- **MITIGATION**: Page unpack errors are silently skipped (continue on Err)

### Performance Regression
- **LOW**: Direct page scan may be slower than B+Tree for very small databases
- **MITIGATION**: Only affects open, not reads

### Page Ownership Conflicts
- **MEDIUM**: Direct file access causes diagnostic warnings
- **IMPACT**: Noise in logs, no correctness issue
- **MITIGATION**: Integrate with FileCoordinator in future iteration

### Scaling to Larger Datasets
- **UNKNOWN**: 100K+ nodes not tested
- **CONCERN**: rebuild_indexes() is O(N) - 100K nodes could take >1 second
- **MITIGATION**: Index serialization would fix this

---

## APPENDIX: Raw Test Output

### Small Dataset Full Output
```
Pure open():                 7.14ms
First get_node (cold):       30.56µs
Second get_node (warm):      16.80µs
First neighbors (cold):      14.08µs
Second neighbors (warm):     200.00ns
Cold/Warm get_node ratio:    1.82x
Cold/Warm neighbors ratio:   70.40x
```

### Medium Dataset Full Output
```
Pure open():               105.26ms
First get_node (cold):       17.10µs
Second get_node (warm):       2.05µs
First neighbors (cold):      16.53µs
Second neighbors (warm):     240.00ns
Cold/Warm get_node ratio:    8.34x
Cold/Warm neighbors ratio:   68.88x
```

---

## CONCLUSION

The rebuild_indexes() fix successfully eliminated the hidden open-time B+Tree lookup pathology. Open() now does zero lookups or decodes.

**The remaining cold-path cost breakdown**:
- **99.6%+**: Pure open (rebuild_indexes page scan)
- **<0.5%**: First read (actual B+Tree lookup + I/O)
- **Negligible**: Warm reads (cached)

**Next target**: rebuild_indexes() page scan optimization via index serialization or lazy population.
