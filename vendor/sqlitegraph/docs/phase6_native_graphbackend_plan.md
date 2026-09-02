# Phase 6 — Native GraphBackend Implementation Plan

## Overview
This document maps the exact GraphBackend trait methods to native backend API primitives, ensuring strict parity with SQLiteBackend behavior.

## Ground Truth Analysis

### GraphBackend Trait Contract (from src/backend.rs)

**Core Methods:**
1. `insert_node(node: NodeSpec) -> Result<i64, SqliteGraphError>`
2. `get_node(id: i64) -> Result<GraphEntity, SqliteGraphError>`
3. `insert_edge(edge: EdgeSpec) -> Result<i64, SqliteGraphError>`
4. `neighbors(node: i64, query: NeighborQuery) -> Result<Vec<i64>, SqliteGraphError>`
5. `bfs(start: i64, depth: u32) -> Result<Vec<i64>, SqliteGraphError>`
6. `shortest_path(start: i64, end: i64) -> Result<Option<Vec<i64>>, SqliteGraphError>`
7. `node_degree(node: i64) -> Result<(usize, usize), SqliteGraphError>`
8. `k_hop(start: i64, depth: u32, direction: BackendDirection) -> Result<Vec<i64>, SqliteGraphError>`
9. `k_hop_filtered(start: i64, depth: u32, direction: BackendDirection, allowed_edge_types: &[&str]) -> Result<Vec<i64>, SqliteGraphError>`
10. `chain_query(start: i64, chain: &[ChainStep]) -> Result<Vec<i64>, SqliteGraphError>`
11. `pattern_search(start: i64, pattern: &PatternQuery) -> Result<Vec<PatternMatch>, SqliteGraphError>`

### SQLiteBackend Reference Implementation (from src/backend/sqlite/impl_.rs)

**Key Behavioral Patterns:**
- Uses `SqliteGraph` as underlying storage
- Error handling via `SqliteGraphError`
- Deterministic ordering (SQLite ORDER BY clauses)
- Direction-aware queries with optional edge type filtering
- Integration with existing algorithm modules (bfs, multi_hop, pattern)

## Implementation Mapping

### 1. NativeGraphBackend Structure

```rust
pub struct NativeGraphBackend {
    graph_file: GraphFile,
}
```

**Wrapper Pattern:** Matches SqliteGraphBackend's `graph: SqliteGraph` field design.

### 2. Core CRUD Operations

#### `insert_node(node: NodeSpec) -> Result<i64, SqliteGraphError>`
**Mapping:**
- **SQLite**: `self.graph.insert_entity(&GraphEntity { id: 0, ... })`
- **Native**: `NodeStore::new(&mut self.graph_file).allocate_node_id()` + `write_node(&NodeRecord::new(...))`

**Key Requirements:**
- Convert `NodeSpec` → `NodeRecord`
- Auto-allocate sequential node IDs
- Return the assigned node ID
- Error mapping: `NativeBackendError` → `SqliteGraphError`

#### `get_node(id: i64) -> Result<GraphEntity, SqliteGraphError>`
**Mapping:**
- **SQLite**: `self.graph.get_entity(id)`
- **Native**: `NodeStore::new(&mut self.graph_file).read_node(id)` → convert to `GraphEntity`

**Key Requirements:**
- Convert `NodeRecord` → `GraphEntity`
- Handle missing nodes with appropriate error
- Maintain exact field mapping (id, kind, name, file_path, data)

#### `insert_edge(edge: EdgeSpec) -> Result<i64, SqliteGraphError>`
**Mapping:**
- **SQLite**: `self.graph.insert_edge(&GraphEdge { id: 0, ... })`
- **Native**: `EdgeStore::new(&mut self.graph_file).allocate_edge_id()` + `write_edge(&EdgeRecord::new(...))`

**Key Requirements:**
- Convert `EdgeSpec` → `EdgeRecord`
- Validate node references exist before creating edge
- Auto-allocate sequential edge IDs
- Update node adjacency metadata (outgoing_count, incoming_count)

### 3. Neighbor Operations

#### `neighbors(node: i64, query: NeighborQuery) -> Result<Vec<i64>, SqliteGraphError>`
**Mapping:**
- **SQLite**: Optimized SQL queries with direction and edge_type filtering
- **Native**: Real adjacency via `AdjacencyIterator` from Phase 5

**Direction Handling:**
- `BackendDirection::Outgoing` → `AdjacencyIterator::new_outgoing(graph_file, node)`
- `BackendDirection::Incoming` → `AdjacencyIterator::new_incoming(graph_file, node)`

**Edge Type Filtering:**
- **SQLite**: WHERE clause in SQL
- **Native**: `AdjacencyIterator::with_edge_filter(edge_types)` (Phase 5 supports this)

**Deterministic Ordering:**
- **SQLite**: ORDER BY to_id, id
- **Native**: Physical order in edge file (Phase 5 deterministic ordering rule)

### 4. Algorithm Integration

#### `bfs(start: i64, depth: u32) -> Result<Vec<i64>, SqliteGraphError>`
**Mapping:**
- **SQLite**: `bfs_neighbors(&self.graph, start, depth)`
- **Native**: Need native BFS implementation using adjacency API

**Challenge:** Algorithm modules currently expect SqliteGraph. Will need adapter layer.

#### `shortest_path(start: i64, end: i64) -> Result<Option<Vec<i64>>, SqliteGraphError>`
**Mapping:**
- **SQLite**: `shortest_path(&self.graph, start, end)`
- **Native**: Need native shortest path implementation

#### `k_hop` and `k_hop_filtered`
**Mapping:**
- **SQLite**: `multi_hop::k_hop(&self.graph, ...)`
- **Native**: Need native k-hop implementation using adjacency iterator

### 5. Complex Queries

#### `node_degree(node: i64) -> Result<(usize, usize), SqliteGraphError>`
**Mapping:**
- **SQLite**: Fetch outgoing/incoming and count lengths
- **Native**: `AdjacencyHelpers::outgoing_degree()` + `incoming_degree()`

**Return Format:** `(outgoing_count, incoming_count)` - must match exactly.

#### `chain_query(start: i64, chain: &[ChainStep]) -> Result<Vec<i64>, SqliteGraphError>`
**Mapping:**
- **SQLite**: `multi_hop::chain_query(&self.graph, start, chain)`
- **Native**: Need native implementation

#### `pattern_search(start: i64, pattern: &PatternQuery) -> Result<Vec<PatternMatch>, SqliteGraphError>`
**Mapping:**
- **SQLite**: `pattern::execute_pattern(&self.graph, start, pattern)`
- **Native**: Need native implementation

## Error Mapping Strategy

### NativeBackendError → SqliteGraphError Mapping

```rust
fn map_native_error(err: NativeBackendError) -> SqliteGraphError {
    match err {
        NativeBackendError::Io(e) => SqliteGraphError::BackendError(e.to_string()),
        NativeBackendError::InvalidNodeId { id, max_id } => {
            SqliteGraphError::BackendError(format!("Invalid node ID: {} (max: {})", id, max_id))
        }
        NativeBackendError::InvalidEdgeId { id, max_id } => {
            SqliteGraphError::BackendError(format!("Invalid edge ID: {} (max: {})", id, max_id))
        }
        NativeBackendError::CorruptNodeRecord { node_id, reason } => {
            SqliteGraphError::BackendError(format!("Corrupt node record {}: {}", node_id, reason))
        }
        NativeBackendError::CorruptEdgeRecord { edge_id, reason } => {
            SqliteGraphError::BackendError(format!("Corrupt edge record {}: {}", edge_id, reason))
        }
        NativeBackendError::BufferTooSmall { size, min_size } => {
            SqliteGraphError::BackendError(format!("Buffer too small: {} < {}", size, min_size))
        }
        NativeBackendError::RecordTooLarge { size, max_size } => {
            SqliteGraphError::BackendError(format!("Record too large: {} > {}", size, max_size))
        }
        NativeBackendError::InconsistentAdjacency { node_id, count, direction, file_count } => {
            SqliteGraphError::BackendError(format!(
                "Inconsistent adjacency for node {}: {} {} != {} in file",
                node_id, direction, count, file_count
            ))
        }
        // Map other error types...
    }
}
```

## Implementation Strategy

### Phase 6A: Core CRUD + Neighbors
1. Implement `NativeGraphBackend` struct with basic operations
2. Implement `insert_node`, `get_node`, `insert_edge` using native stores
3. Implement `neighbors` using Phase 5 real adjacency
4. Implement `node_degree` using `AdjacencyHelpers`

### Phase 6B: Algorithm Adapter Layer
1. Create adapter functions to make native backend work with existing algorithm modules
2. Implement `bfs`, `shortest_path` using adjacency API
3. Implement `k_hop`, `k_hop_filtered` using adjacency API

### Phase 6C: Advanced Queries
1. Implement `chain_query` using adjacency API
2. Implement `pattern_search` using adjacency API
3. Ensure full parity with SQLite backend behavior

## Testing Strategy

### Backend Trait Tests
- **Target**: `tests/backend_trait_tests.rs` must pass for both backends
- **Approach**: Run tests with `SqliteGraphBackend` and `NativeGraphBackend`
- **Validation**: Identical results ordering, error types, edge cases

### Regression Testing
- **Target**: All existing tests must pass (43/43)
- **Approach**: Full test suite comparison before/after implementation
- **Validation**: Zero semantic changes, identical behavior

## Success Criteria

1. **GraphBackend Implementation**: All 11 trait methods implemented
2. **Test Parity**: `backend_trait_tests` pass for both backends with identical results
3. **No Regressions**: Full test suite passes (43/43 tests)
4. **Error Compatibility**: Native errors map to appropriate SqliteGraphError types
5. **Deterministic Behavior**: Identical ordering and empty semantics to SQLite backend

## Files to Create/Modify

### New Files
- `src/backend/native/graph_impl.rs` - Main GraphBackend implementation

### Files to Modify
- `src/backend/native/mod.rs` - Export NativeGraphBackend
- `src/backend.rs` - Re-export NativeGraphBackend alongside SqliteGraphBackend

### Files to Reference
- `src/backend/sqlite/impl_.rs` - Reference implementation for parity
- `src/backend/native/adjacency.rs` - Phase 5 real adjacency logic
- `src/backend/native/node_store.rs` - Node storage primitives
- `src/backend/native/edge_store.rs` - Edge storage primitives
- `src/backend/native/graph_file.rs` - File management primitives

## Limitations and Considerations

### Algorithm Module Dependencies
- Current algorithm modules (bfs, multi_hop, pattern) are SqliteGraph-specific
- Will need adapter functions or temporary re-implementations for native backend
- This is acceptable as long as behavioral parity is maintained

### Performance Characteristics
- Native backend may have different performance profile than SQLite
- This is acceptable as long as semantics and results are identical
- Focus on correctness over performance in Phase 6

### File-Based Storage Semantics
- Native backend uses file offsets and estimated edge positions
- Must handle corruption detection gracefully
- Must maintain deterministic neighbor ordering matching SQLite behavior