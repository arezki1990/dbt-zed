# sqlitegraph Manual

This manual describes how to interact with the sqlitegraph backend from Rust
and the provided CLI tooling. It complements the examples and serves as an
operator guide for TDD, benchmarking, dual-read/write migration, and CLI
operations.

Current backend modes:

- `sqlite` — stable SQLite-backed mode
- `native-v3` — experimental native graph storage mode
- `combined` — explicit SQLite-authoritative mode exposed as
  `CombinedGraphBackend`; SQLite remains the only source of truth today while
  atomic SQLite + graph materialization is still planned
  - optional `CombinedReadMode::PreferMaterialized` enables live untyped
    `neighbors()` / `bfs()` / `k_hop()` / `node_degree()` / `shortest_path()` reads from
    `csr_shards`, with SQLite fallback when a materialized row is unavailable
  - this remains an explicit opt-in specialist mode; current local benches show
    modest cold-read gains plus reduced insert overhead, but writes are still
    slower overall, and the current read-heavy mixed workload is still slower
    end-to-end than SQLite-only
  - these reads are additionally freshness-gated by
    `materialized_version >= authoritative_version`
  - `publish_materialized_views()` is the current explicit rebuild/publish step
  - under `PreferMaterialized`, edge inserts/deletes incrementally refresh the
    affected CSR rows and node-only writes keep versions synchronized
    - current insert maintenance patches the two touched CSR rows directly from
      the latest materialized blobs rather than rebuilding those rows from
      SQLite adjacency queries
    - current delete maintenance removes the deleted node directly from the
      touched blobs and writes empty replacement rows for that node at the new
      version

---

## 1. Building & Testing

```bash
cargo test
cargo bench        # deterministic Criterion benches
```

### Targeted suites

- `cargo test --test subgraph_tests` – subgraph extraction cycles/self-loops/depth determinism.
- `cargo test --test pipeline_tests` and `--test dsl_tests` – reasoning pipeline sequences plus DSL ambiguity/error handling.
- `cargo test --test backend_trait_tests` / `--test migration_tests` – trait conformance, dual-write/shadow-read/cutover stress (including 100-node/300-edge loads).
- `cargo test --test cli_reasoning_tests` / `--test cli_safety_tests` – CLI coverage for subgraph/pipeline/explain/dsl-parse/safety-check (`--strict`).
- `cargo test --test perf_gate_tests` – ensures the committed `sqlitegraph_bench.json` baselines are honored via `bench_gates`.

### Safety Invariants

- Orphan edges are detected by verifying every edge endpoint references a stored entity before any reasoning or subgraph extraction runs.
- Duplicate edges (identical `(from,to,type)` tuples) are tallied so traversal/pipeline counts stay deterministic and regressions surface quickly.
- Invalid label/property references (metadata rows pointing at missing entities) are reported, and `safety-check --strict` fails builds whenever any of the above appear.
- Integrity sweeps (`safety-check --sweep`) perform a deep table walk (entities/edges/labels/properties), verifying sorted IDs, valid JSON payloads, and metadata references before committing to pipelines or migrations.

### DSL Constraints

- Supported clauses are limited to deterministic `pattern`, `k-hop`, `filter type=…`, and `score` steps; ordering matters and only one filter clause is allowed.
- Combination syntax (`CALLS*2`, `CALLS->USES`) must not introduce conflicting filters or unknown tokens—ambiguous or unsupported input causes parser errors surfaced to the CLI/tests.

Benchmarks produce HTML reports under `target/criterion`. Use `cargo bench
--bench bench_insert` (etc.) to isolate suites. The `bench_driver` binary runs
all benches sequentially and surfaces pass/fail summaries.

---

## 2. Core Rust APIs

### Graph primitives

- `SqliteGraph::open(path)` / `open_in_memory()`
- CRUD: `insert_entity`, `get_entity`, `update_entity`, `insert_edge`, etc.
- `GraphQuery` – neighbors, incoming/outgoing, edges_of_type, `has_path`,
  k-hop APIs (`k_hop_outgoing`, `k_hop_filtered`, `chain`), `pattern_matches`.
- `GraphReasoner` – `analyze(start, &PatternQuery, &ReasoningConfig)` returning
  ranked `ReasoningCandidate`s.

### Backend abstraction & ergonomic client

- `GraphBackend` trait standardizes inserts, traversal, pattern queries and
  multi-hop functions.
- `SqliteGraphBackend` is the sqlite-backed implementation (supports `in_memory`
  and `from_graph`).
- `CombinedGraphBackend` is the explicit SQLite-authoritative combined-mode
  wrapper. In the current slice it forwards to `SqliteGraphBackend` while
  preserving a separate public type for future graph materialization logic.
  It also supports `CombinedConfig` for explicit read policy selection.
- `BackendClient` wraps any `GraphBackend` and provides ergonomic helpers using
  wrapper types:
  - `NodeId` / `EdgeId` newtypes (deterministic ordering guarantees).
  - `Label`, `PropertyKey`, `PropertyValue` helpers for the label/property
    indexes (`labeled`, `with_property`).
  - `neighbors_of`, `get_node`, `subgraph`, `run_pattern`, `run_pipeline`,
    `explain_pipeline`.
  - `explain_pipeline` returns `PipelineExplanation` with step summaries, per
    step counts, filters and scoring notes.

### Multi-hop & pattern querying

- `PatternQuery` + `NodeConstraint` describe chained edge traversals with node
  filters.
- Multi-hop utilities guarantee deterministic traversal order (sorted adjacency).

### Reasoning & analog layer

- `GraphReasoner` pairs pattern matches with deterministic multi-hop expansions
  and structural scores (`ReasoningCandidate`).
- `ReasoningPipeline` (pattern / KHops / Filter / Score steps) powers the CLI
  reasoning commands and programmatic inference.

---

## 3. Migration & Dual-run Tooling

### Dual-runtime harness

- `DualRuntime` compares two `GraphBackend`s (primary vs mirror) across a set of
  nodes (`DualRuntimeJob`) reporting matches/diffs with logs.

### Dual-write helper

- `DualWriter` mirrors nodes/edges across two backends, tracking `MirrorStats`.

### Migration manager

- `MigrationManager` manages a base + shadow sqlite backend pair:
  - `insert_node` / `insert_edge` dual-writes.
  - `shadow_read(job)` runs the dual runtime harness.
  - `cutover()` flips the active backend; `is_cutover()` surfaces status.
  - `active_backend()` returns the current read backend.

---

## 4. CLI

Binary: `sqlitegraph` (see `src/bin/sqlitegraph.rs`).

Usage:

```
sqlitegraph [--backend sqlite] [--db memory|PATH] --command <subcommand> [args]
```

Deterministic subcommands:

- `status` (default) – backend + entity count.
- `list` – entity IDs + names (ascending id).
- `subgraph --root N --depth D [--types edge=CALLS --types node=Fn]` – emits a
  JSON neighborhood (`nodes`, `edges`, `signature`) plus the applied node/edge
  filters using the same deterministic traversal as `subgraph::extract_subgraph`.
- `pipeline --dsl "<dsl>"` or `--file pipeline.json` – runs a reasoning pipeline
  and returns `nodes` + detailed `scores`, echoing the DSL string for auditing.
- `explain-pipeline --dsl "<dsl>"` – returns `steps_summary`, `node_counts`,
  `filters`, `scoring` arrays describing the executed pipeline and the DSL.
- `dsl-parse --input "<expr>"` – parses DSL input and classifies it as
  pattern/pipeline/subgraph with metadata (legs, depth, filter counts).
- `safety-check [--strict]` – runs all validators (orphans, duplicates, invalid
  labels/properties) and emits a JSON report (used for CI/operations). With
  `--strict`, the command exits with a failure code when violations exist.
- Legacy `dsl:<expr>` form redirects to `dsl-parse` but the structured command
  is preferred.

The CLI metrics command (`sqlitegraph --db <path> --command metrics [--reset-metrics]`)
reports the live instrumentation snapshot—prepare/execute counts, transaction
begins/commits/rollbacks, plus cache hits/misses—and optionally clears the
counters so operators can capture deltas while reproducing workloads.

Environment variable `CARGO_BIN_EXE_sqlitegraph` is used in tests; the CLI
operates purely locally (in-memory or file-backed sqlite). Use `cargo run
--bin sqlitegraph -- --help` for details.

---

## 5. Examples

### `basic_usage`

Demonstrates:

- Graph construction (`Function`/`Struct` nodes, CALLS/USES edges).
- Neighbors via `GraphQuery`.
- Pattern matching and reasoning scoring (`reasoning score=...` log).

Invoke: `cargo run --example basic_usage`.

### `migration_flow`

Demonstrates:

- `MigrationManager` dual writes, shadow reads, and cutover activation.
- Schema version matrix:
  - Version 1: Base tables (`graph_entities`, `graph_edges`, `graph_labels`,
    `graph_properties`) plus indexes and `graph_meta`.
  - Version 2: Adds `graph_meta_history` rows so each migration application is
    recorded; exposed via `run_pending_migrations` / CLI `migrate`.
  - Future migrations extend the matrix; the CLI refuses to open DBs whose
    version exceeds the compiled `SCHEMA_VERSION`.
  - Upgrade workflow:
    1. Inspect version with `sqlitegraph --command status`
       (shows `schema_version=N`).
    2. Run `sqlitegraph --command migrate --dry-run` to view pending steps.
    3. Execute `sqlitegraph --command migrate` (or call the library helper) to
       apply migrations atomically; history entries are appended automatically.
- Output includes `shadow_read matches=…` and `cutover active=…`.

Invoke: `cargo run --example migration_flow`.

---

## 6. Benchmark Gating & Performance Logging

- `bench_gates::record_bench_run(name, BenchMetric)` records deterministic
  metrics (ops/sec, bytes/sec, free-form notes) into a JSON file
  (`sqlitegraph_bench.json` by default, path configurable via
  `set_bench_file_path`).
- `bench_gates::check_thresholds(name, BenchThreshold)` enforces minimum throughput
  / maximum latency before CI passes.
- `bench_gates::compare_to_baseline` compares against stored metrics to detect
  regressions or improvements.
- Criterion benches call `record_bench_run` exactly once per benchmark; the
  values are deterministic mock metrics so tests remain repeatable while the
  real benches still write actual measurements when run manually.
- Use gating in CI to enforce max latency/regression tolerance (see
  `tests/bench_gates_tests.rs` for API coverage).
Performance thresholds in sqlitegraph_bench.json enforce stability across versions.

---

## 7. Deterministic Dataset Generators

`bench_utils` includes data generators (line, star, grid, Erdos–Renyi, scale
free). All take `(node_count, seed)`; outputs are sorted deterministically.
`tests/bench_data_tests.rs` ensures invariants.

---

## 8. Dual-read/dual-write Workflows

1. Use `DualWriter` or `MigrationManager` to mirror nodes/edges into a shadow
   backend.
2. Execute `shadow_read` jobs across key nodes to compare neighbors + BFS.
3. Inspect `DualRuntimeReport`—the `log` field records match/mismatch entries.
4. Once confident, `cutover()` to shadow backend; `active_backend()` will now
   point to the mirror instance.

---

## 9. Coding Guidelines (Enforced in Repo)

- Max 300 LOC per file (per spec).
- Deterministic ordering (sorted adjacency, seeded RNG) everywhere.
- No async/runtime dependencies; pure Rust + SQLite.
- Tests first (TDD) before implementations.

---

## Code Usage and Warning Behavior

SQLiteGraph includes several components:
- CLI tooling  
- Library API  
- Tests  
- Benchmarks  
- Migration utilities  
- Dual-runtime validation  
- DSL parser  

Many symbols are invoked only through one of these subsystems. This leads clippy to incorrectly classify them as "dead code."

A manual audit verified that:
- All flagged items are used.
- No suppressions were added.
- Removing these items would break tests, CLI or reasoning flows.

Developers should treat these warnings as **informational**.

---

With the manual + README in place, sqlitegraph is fully documented and ready to
serve as the embedded graph backend needed to replace Neo4j. Contributions
should follow the existing TDD workflow and keep new files under 300 LOC.

---

## 10. DSL parser

- `parse_dsl("CALLS->USES")` → `PatternQuery` with appropriate legs.
- `parse_dsl("3-hop type=Fn")` → `SubgraphRequest { depth: 3, allowed_node_types: ["Fn"] }`.
- `parse_dsl("pattern CALLS*3 filter type=Module")` → `ReasoningPipeline` with a
  repeated CALL chain and a node filter.
- Invalid inputs return `DslResult::Error(message)`.

Use the DSL for quick CLI interactions (`--command dsl:CALLS->USES`) or to
bootstrap pipelines/subgraph requests from config files.

---

## 11. CLI reasoning & DSL workflows

- Build DSL expressions (`CALLS->USES`, `pattern CALLS*3 filter type=Module`,
  `3-hop type=Fn`) and feed them to:
  - `sqlitegraph --command pipeline --dsl "<expr>"` to execute reasoning.
  - `sqlitegraph --command explain-pipeline --dsl "<expr>"` to inspect per-step
    counts (useful for tuning TDD tests).
  - `sqlitegraph --command dsl-parse --input "<expr>"` for dry-run validation.
- Pipelines can also be provided via files (JSON containing `{ "dsl": "..." }`).
- CLI commands mirror library functions, so you can reproduce failing CI runs
  locally by copying the DSL snippet.

## 12. Safety checks

- Library: `run_safety_checks(&SqliteGraph)` aggregates:
  - `validate_referential_integrity` (edges referencing missing nodes).
  - `validate_no_duplicate_edges` (duplicate rows per from/to/type triple).
  - `validate_labels_properties` (label/property rows referencing missing nodes).
- Strict mode: `run_strict_safety_checks` returns `SafetyError` if any counters
  are non-zero.
- CLI: `sqlitegraph --command safety-check [--db path]` emits the structured
  JSON report, enabling automation/CI. Adding `--deep` runs `PRAGMA integrity_check`, and `--sweep` performs the extended table sweep described above.
- Tests: `tests/safety_tests.rs` cover each validator, `tests/cli_safety_tests.rs`
  ensures CLI parity.

---

## 12. Label & property indexes

- `index::add_label`, `index::get_entities_by_label`
- `index::add_property`, `index::get_entities_by_property`

All functions are deterministic and sort by `entity_id`. These APIs are used by
the extended `BackendClient` helpers and CLI tooling.
