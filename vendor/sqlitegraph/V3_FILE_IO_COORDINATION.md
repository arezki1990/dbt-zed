# V3 Backend File I/O Coordination Analysis

**Date:** 2026-03-11
**Phase:** OBSERVE
**Goal:** Identify all file I/O paths causing 10K-node bug

---

## A. All Code Paths That Open Main DB File

### 1. NodeStore (`node/store.rs`)
- **Line 283** - `commit_batch()`: Opens with `OpenOptions::new().write(true).create(!file_exists)`
- **Line 697** - `write_node_page()`: Opens with `OpenOptions::new().write(true).create(!file_exists)`
- **Line 952** - `load_node_page()`: Opens with `File::open()` (READ)
- **Line 1104** - `load_page_batch()`: Opens with `File::open()` (READ)
- **Line 1155** - `get_all_node_ids()`: Opens with `File::open()` (READ)
- **Ownership:** `db_path: PathBuf` stored in NodeStore struct

### 2. BTreeManager (`btree.rs`)
- **Line 1124** - `load_page()`: Opens with `File::open()` (READ)
- **Line 1173** - `write_page()`: Opens with `OpenOptions::new().read(true).write(true)`
- **Ownership:** `db_path: Option<PathBuf>` stored in BTreeManager struct

### 3. V3Backend (`backend.rs`)
- **Line 294** - `write_header()`: Opens for header write
- **Line 400** - `open()`: Opens with `File::open()` for header read (READ)
- **Line 813** - External data write (query result caching)
- **Line 831** - External data write (another path)
- **Line 933** - `insert_node_inner()`: Opens for external node data
- **Lines 1169, 1623, 1686** - Additional write paths
- **Ownership:** `db_path: PathBuf` stored in V3Backend struct

### 4. V3EdgeStore (`edge_compat.rs`)
- **Line 434** - `read_page_from_disk()`: Opens with `File::open()` (READ)
- **Line 713** - `write_page_to_disk()`: Opens with `OpenOptions::new().write(true).create(!file_exists)`
- **Ownership:** Receives `db_path: &PathBuf` as function parameter

---

## B. Components Holding Independent Handles

| Component | Handle Type | Lifetime | Coordination |
|-----------|-------------|----------|--------------|
| NodeStore | Opens new `File` per operation | Per-call | NONE |
| BTreeManager | Opens new `File` per operation | Per-call | NONE |
| V3Backend | Opens new `File` per operation | Per-call | NONE |
| V3EdgeStore | Opens new `File` per operation | Per-call | NONE |

**Result:** 4 components, 0 coordination, separate handles every operation

---

## C. Components That Extend/Truncate/Seek/Write Main DB File

### File Extension (`set_len`)
- **NodeStore::commit_batch()** - Line 315
- **NodeStore::write_node_page()** - Line 716
- **BTreeManager::write_page()** - Line 1187
- **V3Backend::insert_node_inner()** - Line 947
- **V3EdgeStore::write_page_to_disk()** - Line 728

### File Seek (`seek(SeekFrom::Start(offset))`)
- All write operations above
- All read operations (load_page, etc.)

### File Write (`write_all`)
- All write operations listed in Section A

### File Sync (`sync_all()` / `sync_data()`)
- **NodeStore:** `sync_all()` after writes
- **BTreeManager:** `sync_all()` after writes and after `set_len()`
- **V3EdgeStore:** `sync_data()` after writes

---

## D. File Length / Page Offset Caching

**No centralized file length caching found.**

Each component independently:
1. Opens file
2. Calls `file.metadata().map(|m| m.len()).unwrap_or(0)`
3. Compares to `required_len`
4. Calls `file.set_len(required_len)` if needed

**Race condition:**
- Thread A: Opens file, sees size S
- Thread B: Opens file, sees size S
- Thread A: Calls `set_len(S1)`, writes page, syncs
- Thread B: Calls `set_len(S1)` (redundant but safe), writes page
- Thread B's write may corrupt Thread A's data or vice versa

---

## E. Coordination Centralization Point (Minimal Disruption)

### Current Architecture
```
V3Backend
├── db_path: PathBuf
├── btree: RwLock<BTreeManager>      (has db_path: Option<PathBuf>)
├── node_store: RwLock<NodeStore>    (has db_path: PathBuf)
├── edge_store: RwLock<V3EdgeStore>  (receives &PathBuf)
└── allocator: Arc<RwLock<PageAllocator>>  (shared - WORKS)
```

### Proposed Change - Shared File Handle
```
V3Backend
├── db_path: PathBuf
├── file_handle: Arc<Mutex<RawFile>>  // NEW: Coordinated access
├── btree: RwLock<BTreeManager>       (gets Arc clone)
├── node_store: RwLock<NodeStore>     (gets Arc clone)
├── edge_store: RwLock<V3EdgeStore>   (gets Arc clone via backend)
└── allocator: Arc<RwLock<PageAllocator>>  (existing pattern)
```

**Why Arc<Mutex<RawFile>>:**
- `Arc`: Shared ownership across components (like allocator)
- `Mutex`: Exclusive access for writes (prevents race conditions)
- `RawFile`: File handle with seek/write/sync capability

### Implementation Strategy

**Option 1: Minimal Coordination Layer**
Create a `FileCoordinator` struct that:
- Wraps a single `File` handle
- Provides `write_page(page_id, data)` method
- Internally handles: seek, extend if needed, write, sync
- Pass `Arc<Mutex<FileCoordinator>>` to all components

**Option 2: Extend FileHandle wrapper**
Create a newtype wrapper with coordinated write methods that all components use.

**Recommended:** Option 1 - FileCoordinator
- Minimal code changes
- Clear coordination point
- Easy to verify correctness
- Follows existing `Arc<RwLock<T>>` pattern

---

## F. Root Cause Summary

**The Bug:**
10K-node insertion fails with `UnexpectedEof` at nondeterministic page numbers.

**The Root Cause:**
Multiple components (NodeStore, BTreeManager, V3Backend, V3EdgeStore) open independent file handles to the same physical file. When extending the file with `set_len()` and writing pages, the OS file cache and metadata can become inconsistent between handles, causing reads to see incomplete data.

**Why 1K Works, 10K Fails:**
- 1K nodes: ~100 pages, fits in OS cache, less contention
- 10K nodes: ~1000+ pages, more file extension events, higher race probability

**Why WAL Works:**
WAL mode coordinates all writes through the WAL file first, then checkpoint. The main DB file is only written during checkpoint (single-threaded).

---

## Next Phase: CONSTRAIN

Define the smallest serious fix that:
1. Coordinates all main DB file writes through a single handle
2. Doesn't require storage format changes
3. Doesn't break existing functionality
4. Is verifiable with existing tests
