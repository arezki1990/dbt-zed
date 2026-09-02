# PHASE 29 STEP 7 MMAP PRE-AUDIT NOTES

## Current GraphFile Buffering Analysis

### Current Buffering Fields in GraphFile (graph_file.rs:98-104)
- **file**: `std::fs::File` - underlying file handle
- **read_buffer**: `ReadBuffer` - adaptive read-ahead buffer (lines 19-67)
  - Fields: `data: Vec<u8>`, `offset: u64`, `size: usize`, `capacity: usize`
  - Adaptive capacity: 256B default, up to 16KB max
- **write_buffer**: `WriteBuffer` - write-behind buffer (lines 69-95)
  - Fields: `operations: Vec<(u64, Vec<u8>)>`, `capacity: usize` (32 pending writes)

### Current Read/Write APIs
- **`read_bytes(offset, buffer)`** (line 282): Uses adaptive read-ahead buffering
- **`write_bytes(offset, data)`** (line 301): Uses write-behind buffering for ≤256B writes
- **`read_bytes_direct(offset, buffer)`** (line 412): Bypasses all buffering (used by V2)
- **`flush_write_buffer()`** (line 383): Flushes pending write operations
- **`invalidate_read_buffer()`** (line 406): Forces fresh reads from disk
- **`read_with_ahead()`** (line 316): Adaptive read-ahead implementation

### Layout Enforcement Locations
- **NODE_SLOT_SIZE**: 4096 bytes (from constants.rs, used in node_store.rs:744)
- **EDGE_SLOT_SIZE**: 256 bytes (from edge_store.rs:33, but check constants.rs)
- Node slot calculation: `node_data_offset + (id-1) * 4096`
- Edge slot calculation: `edge_data_offset + (id-1) * 256`

### Current mmap Usage
- **NO existing mmap usage** - GraphFile uses only `std::fs::File` with manual buffering
- No memmap2 or similar crate in dependencies (only bytemuck, binrw for serialization)

### Current V2 Path Analysis
- V2 uses `read_bytes_direct()` for reads (bypasses read buffer)
- V2 still calls `flush_write_buffer()` and `invalidate_read_buffer()` for write coherence
- V2 writes still use `write_bytes()` which goes through write buffer

### Dependencies Status
- **memmap2**: NOT present in Cargo.toml
- Need to add: `memmap2 = "0.9"` for cross-platform mmap support

### Critical Observations
1. V2 path already partially avoids buffering with `read_bytes_direct()`
2. Write path still uses buffering even in V2 mode
3. No mmap infrastructure currently exists
4. Layout invariants are enforced but use manual File I/O

## STEP 1: MMAP INTEGRATION DESIGN

### Decision: Use memmap2 for both reads and writes in V2 mode
- **Rationale**: Provides zero-copy access, eliminates internal buffering entirely
- **Alternative considered**: Direct File I/O (pread/pwrite-style) - rejected because it would require manual buffer management
- **Dependency change**: Add `memmap2 = "0.9"` to Cargo.toml

### MMAP-Backed GraphFile Invariants
1. **All offsets are absolute** within the mapped region
2. **Node slots**: `[node_data_offset + (id-1)*4096, +4096)` - enforced by GraphFile methods
3. **Edge slots**: `[edge_data_offset + (id-1)*256, +256)` - enforced by GraphFile methods
4. **No internal shadow buffer** modifies bytes after write - mmap ensures direct memory mapping
5. **V2 mode constraint**: GraphFile read/write MUST NOT use any internal buffering other than mmap

### Implementation Strategy
- Add `mmap: Option<MmapMut>` field behind `cfg(feature = "v2_experimental")`
- Helper methods:
  - `mmap_ensure_size(&mut self, len: u64) -> NativeResult<()>`
  - `mmap_read_bytes(&self, offset: u64, buf: &mut [u8]) -> NativeResult<()>`
  - `mmap_write_bytes(&mut self, offset: u64, buf: &[u8]) -> NativeResult<()>`
- Bounds checking against mapped region
- Preserve layout invariants (no writes before node_data_offset)
- Return proper NativeError on out-of-bounds access

### V2 Path Replacement Strategy
- **NodeStore**: Modify `read_node_v2` and `write_node_v2` to use mmap helpers
- **EdgeStore**: Modify `write_clustered_edges` and `read_clustered_edges` to use mmap helpers
- **V1 Preservation**: Keep existing buffered paths for non-V2 (no feature flag)
- **Dead code removal**: Remove unused flush/invalidate helpers in STEP 5