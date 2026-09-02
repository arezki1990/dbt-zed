# sqlitegraph Backend Modes Spec

Status: Draft
Audience: maintainers
Scope: backend architecture, mode contract, transactional guarantees

## 1. Goal

Define a backend architecture that supports:

- SQLite-only operation
- native-v3-only operation
- combined SQLite + graph operation
- optional HNSW/vector features

without forcing users into a multi-service stack such as PostgreSQL + Neo4j + FAISS/Chroma.

The core product goal is one embedded stack with explicit operating modes, not one blurred backend with accidental behavior.

## 2. Problem Statement

The current codebase mixes three concerns:

- standalone SQLite graph storage
- standalone native-v3 graph storage
- derived acceleration structures such as CSR and HNSW

These concerns exist in the same crate, but the authority boundaries are not explicit.

### 2.1 Evidence from current architecture

- [src/backend.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend.rs:116) defines a large `GraphBackend` trait that already abstracts most consumer-facing behavior.
- [src/config/kinds.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/config/kinds.rs:8) defines `BackendKind::{SQLite, Native}`.
- [src/backend_selector.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend_selector.rs:1) is stale and still exposes `BackendKind::{Sqlite, Neo4j}` plus `GraphBackendFactory::new_sqlite()`. This conflicts with the config-layer backend model.
- [src/graph/core.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/graph/core.rs:44) defines `SqliteGraph` as an embedded SQLite-backed engine with:
  - connection pooling
  - adjacency caches
  - query cache
  - `SnapshotManager`
  - loaded `hnsw_indexes`
- [src/backend/sqlite/impl_.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/sqlite/impl_.rs:207) wraps `SqliteGraph` in `SqliteGraphBackend`.
- [src/backend/sqlite/impl_.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/sqlite/impl_.rs:283) shows SQLite traversal still uses SQL tables directly in `query_neighbors(...)`.
- [src/backend/native/mod.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/native/mod.rs:1) describes native as “storage layer only”, but the crate also exports [src/backend/native/v3/backend.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/native/v3/backend.rs:70) `V3Backend`, which is a full `GraphBackend` implementation.
- [src/schema.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/schema.rs:24) and nearby sections persist HNSW metadata inside the main schema, while [src/graph/core.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/graph/core.rs:130) loads HNSW indexes during `SqliteGraph::open_*`.
- [src/schema.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/schema.rs:366) stores `csr_shards` in the relational schema, and recent native-v3 work already uses CSR runtime views layered over edge-store.

### 2.2 Architectural friction

- Backend mode selection is duplicated and inconsistent.
- SQLite truth, native-v3 truth, and derived graph materializations are not clearly separated.
- HNSW is currently entangled with the open path and schema rather than being treated as an optional feature.
- Combined-mode semantics do not exist as a first-class contract even though the codebase already contains the building blocks.

## 3. Target Architecture

### 3.1 First-class operating modes

sqlitegraph SHALL support the following explicit modes:

1. `sqlite`
   - SQLite is the only storage engine.
   - Graph operations execute against SQLite-backed tables and caches.

2. `native-v3`
   - native-v3 is the only storage engine.
   - WAL, MVCC, graph traversal, and graph algorithms operate on native-v3 data.

3. `combined`
   - SQLite is authoritative truth.
   - graph/native-v3 state is a materialized execution layer derived from SQLite truth.
   - SQLite + graph materialization share one atomic visibility boundary.

4. `hnsw` feature
   - optional accelerator
   - not part of the strict atomic contract in the first redesign

### 3.2 Authority model

The authority model SHALL be explicit:

- `sqlite` mode: SQLite tables are authoritative.
- `native-v3` mode: native-v3 files/tables are authoritative.
- `combined` mode: SQLite is authoritative; graph/native-v3 structures are derived and maintained transactionally with SQLite.
- HNSW is optional and outside the strict atomic boundary for the first combined-mode architecture.

### 3.3 Transaction model

For `combined` mode only:

- one WAL / commit journal boundary
- one MVCC visibility boundary
- one snapshot version clock

Atomicity SHALL cover:

- canonical SQL entities/edges/properties
- graph materializations required for traversal and graph algorithms

Atomicity SHALL NOT initially cover:

- HNSW graph topology maintenance
- vector acceleration rebuilds

### 3.4 Derived graph layer

In `combined` mode, graph materialization SHALL be treated as a derived execution layer, not a second source of truth.

Examples:

- forward adjacency
- reverse adjacency
- typed adjacency
- CSR shards / subgraphs
- traversal-local execution views

These structures MUST be rebuildable from authoritative SQL truth.

## 4. Non-Goals

This redesign does not aim to:

- remove standalone native-v3 support
- remove standalone SQLite support
- make HNSW fully transactional in the first phase
- force a networked deployment model
- preserve every internal abstraction if it blocks explicit mode semantics

## 5. Required API Contract Changes

The public contract SHALL become explicit about mode semantics.

### 5.1 Mode selection

The API SHALL expose one canonical mode enum and configuration surface.

The stale split between:

- [src/config/kinds.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/config/kinds.rs:8)
- [src/backend_selector.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend_selector.rs:4)

MUST be removed.

Proposed contract direction:

- `BackendMode::SQLite`
- `BackendMode::NativeV3`
- `BackendMode::Combined`

Optional vector flags belong in config, not in the mode enum.

### 5.2 Capability contract

The manual and API docs MUST state for each mode:

- authoritative store
- WAL behavior
- MVCC behavior
- atomicity boundary
- traversal source
- graph algorithm source
- HNSW support level

## 6. Internal Design Constraints

### 6.1 Preserve consumer surface where practical

The existing `GraphBackend` trait in [src/backend.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend.rs:116) is broad enough that most consumer APIs do not need immediate redesign.

The main changes belong under the backend implementations and open/config path.

### 6.2 Combined mode is composition, not replacement

`combined` mode SHALL be a first-class composition layer.

It SHALL NOT erase:

- standalone SQLite backend
- standalone native-v3 backend

### 6.3 HNSW decoupling

Because [src/graph/core.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/graph/core.rs:130) currently loads HNSW indexes during `SqliteGraph` construction, HNSW is entangled with the SQLite open path.

The redesign SHOULD move HNSW toward:

- explicit optional initialization
- explicit rebuild/fallback semantics
- non-atomic feature status in combined mode

## 7. Expected Change Surface

High-change areas:

- [src/backend.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend.rs:1)
- [src/backend_selector.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend_selector.rs:1)
- [src/config/graph_config.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/config/graph_config.rs:1)
- [src/config/kinds.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/config/kinds.rs:1)
- [src/graph/core.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/graph/core.rs:44)
- [src/backend/sqlite/impl_.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/sqlite/impl_.rs:207)
- [src/backend/native/v3/backend.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/backend/native/v3/backend.rs:70)
- [src/schema.rs](/home/feanor/Projects/sqlitegraph/sqlitegraph-core/src/schema.rs:1)
- `src/sharding/*`
- `src/hnsw/*`

Lower-change areas:

- graph algorithms
- traversal consumers
- most pattern/Cypher callers

## 8. Acceptance Criteria

This spec is satisfied when:

- backend modes are explicit and singular in the public contract
- standalone SQLite and standalone native-v3 both remain supported
- combined mode is documented and testable
- SQLite + graph materialization share one atomic/MVCC/WAL contract in combined mode
- HNSW is documented as optional and outside the strict atomic contract unless explicitly upgraded later
- README, manual, architecture docs, API docs, and changelog all describe the same semantics

## 9. Current Implementation Status

Implemented so far:

- Phase 0 contract cleanup:
  - canonical `BackendKind`
  - canonical env parsing
  - stale selector contract removed
- Phase 1 open-path separation:
  - explicit `GraphConfig::combined()`
  - explicit `open_graph(..., combined)` branch
- Early Phase 2 authority seam:
  - public `CombinedGraphBackend`
  - SQLite remains authoritative in combined mode by construction
  - current combined behavior delegates to SQLite execution paths by default
- opt-in `CombinedReadMode::PreferMaterialized` exists for live untyped
    `neighbors()` / `bfs()` / `k_hop()` / `node_degree()` / `shortest_path()` reads via
    `csr_shards`, with SQLite fallback
  - `PreferMaterialized` SHALL remain explicit opt-in specialist behavior until
    broader benchmarks justify changing the default tradeoff
  - materialized combined reads are freshness-gated by
    `materialized_version >= authoritative_version`
  - `publish_materialized_views()` now provides an explicit rebuild/publish
    step from SQLite truth into combined CSR runtime rows
  - under `PreferMaterialized`, combined mode now incrementally refreshes the
    affected CSR rows for edge insert/delete and keeps versions synchronized on
    node-only writes
    - current optimization slice narrows insert maintenance further by patching
      only the two touched CSR rows directly from the latest materialized blobs
    - current optimization slice narrows delete maintenance by removing the
      deleted node directly from the touched blobs and writing empty
      replacement rows for the deleted node itself
  - current local benchmark finding: modest cold-read gains do not yet justify
    default-on write amplification
  - broader mixed-workload and reopen benchmarks currently do not justify a
    default-on policy either; current mixed workloads improved materially, but
    both read-heavy and balanced cases are still slower end-to-end than
    SQLite-only

Not implemented yet:

- distinct graph materialization behind combined mode
- atomic SQLite + graph materialization commit boundary
- rebuild/fallback semantics for derived graph views
## 10. Implementation Readiness Decision

This spec recommends proceeding with a phased implementation because the current codebase already contains:

- a broad backend trait
- a SQLite engine with MVCC-related concepts
- a native-v3 engine with WAL/MVCC-related concepts
- CSR materialization infrastructure

The main missing piece is not capability; it is explicit architectural layering and contract cleanup.
