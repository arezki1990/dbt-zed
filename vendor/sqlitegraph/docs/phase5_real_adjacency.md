# Phase 5 — Real Adjacency Implementation

## Overview
This document specifies the exact implementation requirements for replacing the temporary mock adjacency logic with real edge-based neighbor traversal in the native backend.

## Exact Record Format (from implementation)

### NodeRecord Structure
```rust
pub struct NodeRecord {
    pub id: NativeNodeId,              // i64 - Unique node identifier
    pub flags: NodeFlags,              // u32 - Node state flags
    pub kind: String,                  // Node type/kind (e.g., "Function", "Variable")
    pub name: String,                  // Human-readable node name
    pub data: serde_json::Value,       // JSON metadata
    pub outgoing_offset: FileOffset,   // u64 - Offset to first outgoing edge in edge data section
    pub outgoing_count: u32,           // Number of outgoing edges
    pub incoming_offset: FileOffset,   // u64 - Offset to first incoming edge in edge data section
    pub incoming_count: u32,           // Number of incoming edges
}
```

### EdgeRecord Structure
```rust
pub struct EdgeRecord {
    pub id: NativeEdgeId,      // i64 - Unique edge identifier
    pub from_id: NativeNodeId,  // i64 - Source node identifier
    pub to_id: NativeNodeId,    // i64 - Target node identifier
    pub edge_type: String,     // Edge type (e.g., "calls", "imports", "references")
    pub flags: EdgeFlags,      // u16 - Edge state flags
    pub data: serde_json::Value, // JSON metadata
}
```

### File Layout Constants
- **Header Size**: 64 bytes (fixed)
- **Magic Bytes**: `b'S', b'Q', b'L', b'T', b'G', b'F', 0, 0`
- **Node Data Offset**: Starts at offset 64
- **Edge Data Offset**: Starts at offset 64 (nodes and edges share same area in current implementation)

## Exact Adjacency Algorithm

### Offset Computation Formula
```
For a given node_id:
1. Read NodeRecord via NodeStore::read_node(node_id)
2. Determine direction:
   - Outgoing: use node.outgoing_offset, node.outgoing_count
   - Incoming: use node.incoming_offset, node.incoming_count
3. Edge iteration: edges are stored contiguously starting at the specified offset
```

### Direction Rules
- **Outgoing edges**: `edge.from_id == node_id` (edges where this node is the source)
- **Incoming edges**: `edge.to_id == node_id` (edges where this node is the target)

### Edge Reading Process
1. **Read Node**: `node_store.read_node(node_id)` → NodeRecord
2. **Determine Edge Slice**:
   - `base_offset = node.outgoing_offset` OR `node.incoming_offset`
   - `edge_count = node.outgoing_count` OR `node.incoming_count`
3. **Iterate Edges**: For i in 0..edge_count:
   - Calculate edge position (current implementation uses estimate: `base_offset + i * 128`)
   - Read edge at position via EdgeStore (requires proper indexing in full implementation)
4. **Apply Direction Filter**:
   - Keep edge if `edge.from_id == node_id` (for outgoing)
   - Keep edge if `edge.to_id == node_id` (for incoming)
5. **Extract Neighbor IDs**:
   - For outgoing: return `edge.to_id`
   - For incoming: return `edge.from_id`

### Deterministic Ordering Rule
**Result order = Physical order in edge file**
- Edges must be returned in the exact order they appear in the edge data section
- No sorting, no reordering by ID or any other criteria

## Corruption Detection Rules

### File Bounds Validation
- `edge_offset + (edge_count * estimated_edge_size)` must not exceed file size
- Individual edge reads must not exceed `edge_data_offset + file_size`

### Node Reference Validation
- `edge.from_id` must be: `1 <= edge.from_id <= header.node_count`
- `edge.to_id` must be: `1 <= edge.to_id <= header.node_count`

### Structured Error Requirements
- **Invalid node reference**: Return `NativeBackendError::InvalidNodeId { id, max_id }`
- **File corruption**: Return `NativeBackendError::CorruptEdgeRecord { edge_id, reason }`
- **Buffer issues**: Return `NativeBackendError::BufferTooSmall { size, min_size }`
- **IO errors**: Allow to propagate as `NativeBackendError::Io`

### Edge Storage Limitations
Current implementation uses estimated offsets (`edge_id * 128` bytes from edge_data_offset).
Real adjacency must handle:
- Variable-length edge records
- Potential gaps between edges
- Corrupted edge records (invalid version, bad checksums, etc.)

## Implementation Requirements

### Functions to Modify
Only these functions may be changed in `src/backend/native/adjacency.rs`:

1. **`AdjacencyIterator::get_current_neighbor()`** - Replace mock logic with real edge reading
2. **`AdjacencyIterator::collect()`** - Works with updated get_current_neighbor
3. **`AdjacencyIterator::contains()`** - Works with updated get_current_neighbor
4. **`AdjacencyIterator::get_batch()`** - Works with updated get_current_neighbor

### Forbidden Changes
- **DO NOT** modify SQLite backend
- **DO NOT** integrate with GraphBackend yet
- **DO NOT** add new adjacency features beyond real edge reading
- **DO NOT** panic or unwrap - use structured Result<T, NativeBackendError>

### Current Edge Offset Limitation
The current EdgeStore uses estimated offsets: `base_offset + ((edge_id - 1) * 128)`
Real adjacency implementation must:
1. Work within this limitation for now
2. Add proper validation to detect when estimates are wrong
3. Return structured errors for corrupted/unreadable edges

## Test Preservation Requirements

### Existing Test Behavior
Tests currently expect:
- `test_single_node_neighbors_outgoing`: 3 outgoing neighbors for node with `outgoing_count=3`
- `test_single_node_neighbors_incoming`: 2 incoming neighbors for node with `incoming_count=2`
- `test_multi_node_adjacency`: Degree counts match declared counts

### Test Update Rules
1. **If test contradicts documented architecture** → Update the TEST
2. **If implementation contradicts documented behavior** → Fix the IMPLEMENTATION
3. **Prefer documented design over existing test behavior**

### Success Criteria
- **18/18 native_backend_storage_tests passing**
- **All library tests passing (no regressions)**
- **Real edge reading replaces all mock adjacency logic**
- **Strict corruption detection implemented**
- **Deterministic ordering preserved**