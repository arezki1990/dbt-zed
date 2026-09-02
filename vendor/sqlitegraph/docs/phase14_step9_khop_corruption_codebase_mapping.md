# Phase 14 Step 9: K-Hop V1 Corruption Codebase Mapping

## Error Message Analysis

**Primary Error**: `ConnectionError("failed to fill whole buffer")`

**Root Cause**: File I/O layer attempting to read beyond available file data, causing `std::io::Error::UnexpectedEof` which gets wrapped and propagated as `ConnectionError`.

## Complete Call Chain Analysis

### 1. K-Hop Benchmark Entry Point
**File**: `sqlitegraph/benches/k_hop.rs`
**Function**: `main()` → Benchmark execution
**Key Line**: Line 102: `graph.k_hop(node_ids[0], 1, BackendDirection::Outgoing).expect("Failed to perform 1-hop traversal")`

### 2. High-Level K-Hop API
**File**: `sqlitegraph/src/multi_hop.rs`
**Function**: `k_hop(graph: &SqliteGraph, start: i64, depth: u32, direction: BackendDirection) → Result<Vec<i64>, SqliteGraphError>`
**Key Line**: Line 23: `k_hop_internal(graph, start, depth, direction, None)`

### 3. K-Hop Internal Implementation
**File**: `sqlitegraph/src/multi_hop.rs`
**Function**: `k_hop_internal()`
**Key Line**: Line 125: `let neighbors = adjacency_for(graph, node, direction, allowed_lookup.as_ref())?;`

### 4. Adjacency Resolution
**File**: `sqlitegraph/src/multi_hop.rs`
**Function**: `adjacency_for(graph: &SqliteGraph, node: i64, direction: BackendDirection, allowed_types: Option<&AHashSet<&str>>) → Result<Vec<i64>, SqliteGraphError>`
**Key Lines**:
- Line 146: `BackendDirection::Outgoing => graph.fetch_outgoing(node),`

### 5. SQLite Backend Dispatch
**File**: `sqlitegraph/src/graph/adjacency.rs`
**Function**: `fetch_outgoing(&self, id: i64) → Result<Vec<i64>, SqliteGraphError>`
**Note**: This dispatches to the native backend for SQLiteGraph instances

### 6. Native Backend K-Hop Implementation
**File**: `sqlitegraph/src/backend/native/graph_backend.rs`
**Function**: `k_hop(&self, start: i64, depth: u32, direction: BackendDirection) → Result<Vec<i64>, SqliteGraphError>`
**Key Lines**:
- Line 163: `let result = native_k_hop(graph_file, start as NativeNodeId, depth, direction.into())?;`

### 7. Native K-Hop Core Algorithm
**File**: `sqlitegraph/src/backend/native/graph_ops.rs`
**Function**: `native_k_hop(graph_file: &mut GraphFile, start: NativeNodeId, depth: u32, direction: Direction) → Result<Vec<NativeNodeId>, NativeBackendError>`
**Key Lines**:
- Line 308: `Direction::Outgoing => AdjacencyHelpers::get_outgoing_neighbors(graph_file, node)?,`

### 8. Adjacency Helpers
**File**: `sqlitegraph/src/backend/native/adjacency.rs`
**Function**: `get_outgoing_neighbors(graph_file: &mut GraphFile, node_id: NativeNodeId) → NativeResult<Vec<NativeNodeId>>`
**Key Lines**:
- Line 419: `let iterator = AdjacencyIterator::new_outgoing(graph_file, node_id)?;`
- Line 420: `iterator.collect()`

### 9. Adjacency Iterator Creation
**File**: `sqlitegraph/src/backend/native/adjacency.rs`
**Function**: `AdjacencyIterator::new_outgoing(graph_file: &'a mut GraphFile, node_id: NativeNodeId) → NativeResult<Self>`
**Key Lines**:
- Line 91: `let mut node_store = NodeStore::new(graph_file);`
- Line 92: `let node = node_store.read_node(node_id)?;`

### 10. Node Store Read Operation
**File**: `sqlitegraph/src/backend/native/node_store.rs`
**Function**: `read_node(&mut self, node_id: NativeNodeId) → NativeResult<NodeRecord>`
**Key Lines**:
- Line 128: `self.rebuild_index_for_node(node_id)?`
- Line 131: `let node = self.read_node_internal(node_id, offset)?;`

### 11. Node Index Rebuilding
**File**: `sqlitegraph/src/backend/native/node_store.rs`
**Function**: `rebuild_index_for_node(&mut self, target_id: NativeNodeId) → NativeResult<FileOffset>`
**Key Lines**:
- Line 323: `let offset = node_data_offset + ((id - 1) as u64 * 4096);`
- Line 336: `if remaining_bytes < 32 { /* validation */ }`

### 12. Internal Node Read with Corruption Point
**File**: `sqlitegraph/src/backend/native/node_store.rs`
**Function**: `read_node_internal(&mut self, node_id: NativeNodeId, offset: FileOffset) → NativeResult<NodeRecord>`
**Key Lines**:
- Line 182: `let total_size = 1 + 4 + 8 + 2 + 2 + 4 + kind_len + name_len + data_len + 8 + 4 + 8 + 4;`
- Line 186-196: **PHASE 14 STEP 9 FIX ADDED HERE**: Boundary validation before read
- Line 199: `let mut buffer = vec![0u8; total_size];`
- Line 200: `if let Err(e) = self.graph_file.read_bytes(offset, &mut buffer) {`

### 13. Graph File Read Operation with Primary Corruption Point
**File**: `sqlitegraph/src/backend/native/graph_file.rs`
**Function**: `read_bytes(&mut self, offset: u64, buffer: &mut [u8]) → NativeResult<()>`
**Key Lines**:
- Line 244: `if !self.read_buffer.read(offset, buffer) {`
- Line 246: `self.read_with_ahead(offset, buffer)?;`

### 14. Read-Ahead Logic (ACTUAL "failed to fill whole buffer" SOURCE)
**File**: `sqlitegraph/src/backend/native/graph_file.rs`
**Function**: `read_with_ahead(&mut self, offset: u64, buffer: &mut [u8]) → NativeResult<()>`
**Key Lines**:
- Line 268: `let read_size = std::cmp::max(buffer.len(), self.read_buffer.capacity);`
- Line 269: `let read_ahead_size = std::cmp::min(read_size, self.read_buffer.capacity);`
- Line 273-288: **PHASE 14 STEP 9 FIX ADDED HERE**: Boundary validation and adjusted size
- Line 288: `self.file.read_exact(&mut self.read_buffer.data[..adjusted_read_size])?;`
- Line 295-300: **PHASE 14 STEP 9 FIX ADDED HERE**: Buffer size validation

## Data Structures and Fields

### NativeNodeId
**Type**: `u64`
**Usage**: Node identifier used throughout the native backend

### FileOffset
**Type**: `u64`
**Usage**: Byte offset within the graph file for data locations

### NodeRecord Structure (V1 Format)
```rust
struct NodeRecord {
    id: NativeNodeId,
    kind: String,
    name: String,
    data: serde_json::Value,
    outgoing_count: u32,
    outgoing_offset: u32,
    incoming_count: u32,
    incoming_offset: u32,
}
```

### V1 File Layout
- **Fixed 4KB slots per node**: `offset = node_data_offset + ((node_id - 1) * 4096)`
- **Node serialization**: Version + flags + ID + kind_len + name_len + data_len + strings + adjacency
- **Total node size calculation**: `1 + 4 + 8 + 2 + 2 + 4 + kind_len + name_len + data_len + 8 + 4 + 8 + 4`

## Key Variables and Constants

### Buffer Sizes
- `self.read_buffer.capacity`: Read-ahead buffer capacity (from ReadBuffer struct)
- `total_size`: Calculated node record size for V1 format
- `read_ahead_size`: Amount of data to read in advance for performance

### Offsets
- `node_data_offset`: Starting offset of node data section in file
- `offset`: Calculated file offset for specific node
- `remaining_bytes`: `file_size - offset` (bytes available from current position)

### File Metadata
- `file_size`: Total size of the graph file
- `header.node_count`: Number of nodes in the graph

## Error Types and Propagation

### NativeBackendError Variants
- `CorruptNodeRecord { node_id, reason }`: Used when node validation fails
- `BufferTooSmall { size, min_size }`: Used when trying to read more data than available
- `EndOfFile { offset, file_size }`: Used when attempting to read beyond file end
- `InvalidNodeId { id, max_id }`: Used when node ID is out of valid range

### Error Wrapping Chain
1. `std::io::Error::UnexpectedEof` (from `read_exact()`)
2. → `NativeBackendError` (in graph_file.rs)
3. → `SqliteGraphError::Connection` (in error conversion)
4. → `panic!()` (in benchmark with `.expect()`)

## Phase 14 Step 9 Fix Locations

### Fix 1: Node Store Boundary Validation
**File**: `sqlitegraph/src/backend/native/node_store.rs`
**Function**: `read_node_internal()`
**Lines**: 184-196
**Purpose**: Validate enough remaining data before attempting node read

### Fix 2: Graph File Read-Ahead Protection
**File**: `sqlitegraph/src/backend/native/graph_file.rs`
**Function**: `read_with_ahead()`
**Lines**: 271-300
**Purpose**: Prevent reading beyond file end in read-ahead optimization

## Critical Success Metrics

### Before Fix
- **Error**: `ConnectionError("failed to fill whole buffer")`
- **Location**: `read_exact()` in `read_with_ahead()`
- **Trigger**: K-hop benchmark with 100 nodes in star topology

### After Fix Expected
- **No "failed to fill whole buffer" errors**
- **Proper error messages for boundary conditions**
- **K-hop benchmark completion**
- **No regression in existing functionality**

## Test Strategy

### Regression Test Target
**Test**: `v1_k_hop_native_should_not_corrupt_100_nodes`
**Pattern**: Star graph with center node 0, 99 leaf nodes
**Operation**: `graph.k_hop(0, 1, BackendDirection::Outgoing)`
**Expected**: Success with 99 neighbors returned
**Before Fix**: Panic with "failed to fill whole buffer"
**After Fix**: Successful completion

---

**Status**: Complete codebase mapping with surgical V1 fixes implemented at exact corruption points.
**Next**: Test fixes with k-hop benchmark and create regression test.