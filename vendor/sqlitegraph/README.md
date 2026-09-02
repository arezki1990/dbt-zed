# SQLiteGraph

[![crates.io](https://img.shields.io/crates/v/sqlitegraph.svg)](https://crates.io/crates/sqlitegraph)
[![Documentation](https://docs.rs/sqlitegraph/badge.svg)](https://docs.rs/sqlitegraph)

Embedded graph database with explicit backend modes, graph algorithms,
Cypher-inspired queries, and HNSW vector search.

**Positioning:** Single-binary embedded database (no server). Persistent storage with
atomic batch commits. Graph algorithms + HNSW vector search in one engine.
SQLite: stable, mature, and easy to inspect with standard tooling. Native V3:
experimental graph-oriented storage with cache, KV, pub/sub, and traversal
features. Native-v3 runtime traversal now layers CSR runtime views on top of
the V3 edge-store/B+Tree source of truth for supported neighbor/traversal
reads, but CSR/sharding is still not a standalone backend traversal engine.
See the benchmarks below for workload-specific behavior.

## Technical Architecture

**Backend modes**
- SQLite backend — stable, mature, inspectable with standard tooling
- Native V3 backend — experimental graph-oriented storage with bounded adjacency cache, KV, pub/sub, and traversal primitives
- Combined mode — explicit SQLite-authoritative contract that currently opens
  as `CombinedGraphBackend`, using SQLite as the only source of truth while
  the atomic graph-materialization layer is still being built
  - optional `CombinedReadMode::PreferMaterialized` currently applies only to
    live untyped `neighbors()`/`bfs()`/`k_hop()`/`node_degree()`/`shortest_path()` via
    `csr_shards`, with SQLite fallback
  - `PreferMaterialized` remains an explicit opt-in specialist mode: current
    local benches show modest cold-read gains, improved but still slower write
    maintenance, and no end-to-end win yet on the current mixed-workload
    benchmarks
  - materialized reads are version-gated: combined mode only trusts CSR when
    `materialized_version >= authoritative_version`
  - `publish_materialized_views()` rebuilds and publishes CSR from SQLite truth
  - under `PreferMaterialized`, edge inserts/deletes incrementally refresh the
    affected CSR rows and node-only writes keep authoritative/materialized
    versions aligned
    - the current insert fast path patches the two affected CSR rows directly
      from the latest materialized blobs
    - the current delete fast path removes the deleted node directly from the
      touched blobs and writes empty replacement rows for the deleted node
    - mixed workloads improved further, but still do not beat SQLite-only
      overall

**Vector + graph queries**
- HNSW vector similarity search (`HnswIndex`, batch insert, cosine guard)
- Native-v3 HNSW routing with optional turbovec acceleration for large vector sets
- Cypher-inspired query layer — `pattern_match`, structural graph queries
- Graph algorithms — BFS, k-hop, shortest path, louvain community detection, SCC, A*

**Native V3 current state**
- Query truth, Cypher parity, graph algorithm, HNSW, turbovec, and native-v3 comprehensive suites are green in local verification
- Edge-store remains the authoritative adjacency store; CSR runtime views are rebuilt from it and consulted first for supported current-snapshot neighbor/traversal reads
- CSR runtime coverage now includes outgoing, incoming, typed, and weighted neighbor-backed traversal paths, but CSR/sharding is still not the backend-default standalone engine

**Concurrency & durability**
- MVCC snapshot reads during traversal (`src/mvcc.rs`)
- Write-ahead log, atomic batch commits
- Bounded LRU adjacency cache (degree-1000 hit: ~131 ns → ~13 ns)

**Temporal topology**
- Version chains via `checkpoint()` / `snapshot_as_of(n)`
- Persistent-homology analysis: H₀ component lifetimes (`scc_lineage_barcode`), β₁ cyclomatic trajectory (`cycle_rank_snapshot`), circular-dependency lifecycle (`cycle_scc_barcode`)

**Bindings**
- Python bindings via PyO3 / maturin (`sqlitegraph-py`)

**Keyword index:** embedded graph database · HNSW vector search · Cypher · MVCC · write-ahead log · persistent homology · temporal versioning · louvain · SQLite · Native V3 · Python bindings · Rust

## What's New in the 3.9 Line

The current 3.9.x line on `main` tightens the combined-backend contract,
continues the native-v3 cleanup wave, and makes the capability story explicit
instead of aspirational:

- **Combined backend remains SQLite-authoritative** — combined mode is now
  documented as an explicit SQLite-truth path with optional
  `CombinedReadMode::PreferMaterialized` for specialist live-read workloads.
- **Native-v3 internals were cleaned up aggressively without widening the public API** —
  large mixed-responsibility modules were split into focused child modules while
  keeping trait-facing behavior and regression coverage intact.
- **Status docs now match verified behavior** — native-v3 and combined-mode
  docs describe what is green, what is experimental, and where the materialized
  path is still opt-in.

Known native-v3 gaps still remain:

- multi-hop Cypher still fails on V3
- `query_nodes_by_name_pattern(...)` still uses substring semantics instead of SQLite `GLOB`
- `snapshot_import(...)` is still unsupported

See [CHANGELOG](https://github.com/oldnordic/sqlitegraph/blob/main/CHANGELOG.md) for full history.

## Backends

| Feature | SQLite | Native V3 | Combined |
|---------|--------|-----------|----------|
| Status | Stable | Experimental (Bugs should be reported) | Phase 2 authority seam |
| Authority | SQLite | Native V3 | SQLite |
| Storage | `.db` file | `.graph` file | `.db` file |
| Graph algorithms | 35+ | 35+ | Delegates to SQLite today |
| HNSW vectors | Yes | Yes | Delegates to SQLite today |
| Turbovec acceleration | No | Yes (feature-gated, large-vector routing) | No |
| MVCC snapshots | Yes (v8) | Yes (v8) | Delegates to SQLite today |
| Time-travel queries | Yes (optimized v9) | Yes (optimized v9) | Delegates to SQLite today |
| Default traversal runtime | SQLite table/index path | V3 edge-store / B+Tree path | SQLite table/index path today via `CombinedGraphBackend` |
| Optional materialized live reads | N/A | CSR runtime views | Untyped `neighbors()` / `bfs()` / `k_hop()` / `node_degree()` / `shortest_path()` via opt-in CSR fallback; specialist mode, not default |
| Atomic SQLite + graph materialization | N/A | N/A | Not yet implemented |

## Benchmarks

See [Architecture](https://github.com/oldnordic/sqlitegraph/blob/main/docs/ARCHITECTURE.md) for system design details and [Benchmarking](https://github.com/oldnordic/sqlitegraph/blob/main/docs/BENCHMARKING.md) for methodology.

**Representative clean samples from 2026-06-07 (AMD Ryzen 7 7800X3D, tmpfs, Rust 1.95.0):**

| Benchmark | SQLite | V3 |
|-----------|--------|----|
| Criterion `bfs_traversal/small_random_1k_5k` | `2.3680 ms` | `3.3191 ms` |
| Criterion `bfs_traversal/medium_random_10k_50k` | `26.510 ms` | `56.240 ms` |
| Release microbenchmark point lookup | `3965 ns` | `146 ns` |

Use the release microbenchmark for fast sanity checks and the Criterion suites
for workload comparisons.

**Run benchmarks yourself:**

```bash
git clone https://github.com/oldnordic/sqlitegraph.git
cd sqlitegraph
./scripts/run-curated-benchmarks.sh

cd sqlitegraph-core
cargo run --release --example test_performance_comparison --features native-v3
cargo bench --features native-v3 --bench backend_comparison
cargo bench --features native-v3 --bench sqlite_v3_curated
```

See [examples/](https://github.com/oldnordic/sqlitegraph/tree/main/sqlitegraph-core/examples) for quick performance checks and [docs/BENCHMARKING.md](https://github.com/oldnordic/sqlitegraph/blob/main/docs/BENCHMARKING.md) for the full workflow.

## Quick Start

```toml
[dependencies]
# SQLite backend (default)
sqlitegraph = "3.9.0"

# OR Native V3 backend (graph-oriented storage)
sqlitegraph = { version = "3.9.0", features = ["native-v3"] }
```

```rust
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use sqlitegraph::backend::sqlite::SqliteGraphBackend;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = SqliteGraphBackend::in_memory()?;

    let node_id = backend.insert_node(NodeSpec {
        kind: "User".to_string(),
        name: "Alice".to_string(),
        file_path: None,
        data: serde_json::json!({"age": 30}),
    })?;

    println!("Created node: {}", node_id);
    Ok(())
}
```

## TypedDiGraph (In-Memory)

A lightweight in-memory directed graph with typed node and edge weights,
independent of the `GraphBackend` persistence layer. Useful for build-system
DAGs, dependency graphs, and analysis passes that don't need disk storage.

```rust
use sqlitegraph::typed_digraph::{TypedDiGraph, NodeIndex, Direction};
use sqlitegraph::typed_digraph::algo::{toposort, tarjan_scc, Dfs};

let mut g = TypedDiGraph::<&str, i32>::new();
let a = g.add_node("compile");
let b = g.add_node("link");
let c = g.add_node("run");
g.add_edge(a, b, 1);
g.add_edge(b, c, 2);

// Topological order
let order = toposort(&g).expect("acyclic");
assert_eq!(order, vec![a, b, c]);

// DFS traversal
let mut dfs = Dfs::new(&g, a);
assert_eq!(dfs.by_ref().collect::<Vec<_>>(), vec![a, b, c]);
```

Available in the current 3.x line.

## CLI

```bash
cargo install sqlitegraph-cli

# Query
sqlitegraph --db graph.db query "MATCH (n:User) RETURN n.name"

# Algorithms
sqlitegraph --db graph.db bfs --start 1 --depth 3
sqlitegraph --db graph.db algo pagerank --iterations 100
```

## Copy-Paste CLI Demo

```bash
rm -f /tmp/sqlitegraph-demo.db

sqlitegraph --db /tmp/sqlitegraph-demo.db --write insert --kind User --name Alice --data '{"age":30}'
sqlitegraph --db /tmp/sqlitegraph-demo.db --write insert --kind User --name Bob --data '{"age":31}'
sqlitegraph --db /tmp/sqlitegraph-demo.db --write query 'CREATE (1)-[:KNOWS]->(2)'

sqlitegraph --db /tmp/sqlitegraph-demo.db query 'MATCH (a:User)-[:KNOWS]->(b:User) RETURN a.name, b.name'
sqlitegraph --db /tmp/sqlitegraph-demo.db algo scc
```

## Hybrid Runtime Demo

This crate includes a runnable demo that combines ordinary SQLite rows, Native
V3 graph metadata, SQLite-backed HNSW vectors, and V3 pub/sub:

```bash
cargo run -p sqlitegraph --example hybrid_sqlite_v3_hnsw_pubsub --features native-v3
```

## Safety Invariants

- Orphan edges are detected by verifying every edge endpoint references a stored entity before any reasoning or subgraph extraction runs.
- Duplicate edges (identical `(from,to,type)` tuples) are tallied so traversal/pipeline counts stay deterministic and regressions surface quickly.
- Invalid label/property references (metadata rows pointing at missing entities) are reported by the safety-check helpers.
- Integrity sweeps perform a deep table walk (entities/edges/labels/properties), verifying sorted IDs, valid JSON payloads, and metadata references before committing to pipelines or migrations.

## DSL Constraints

- Supported clauses are limited to deterministic `pattern`, `k-hop`, `filter type=…`, and `score` steps; ordering matters and only one filter clause is allowed.
- Combination syntax (`CALLS*2`, `CALLS->USES`) must not introduce conflicting filters or unknown tokens—ambiguous or unsupported input causes parser errors surfaced to the CLI/tests.

## Performance & Instrumentation

Performance thresholds in `sqlitegraph_bench.json` gate releases. Benchmarks
produce HTML reports under `target/criterion`. Use
`cargo bench --bench bench_insert` (etc.) to isolate suites. For comparison
work, prefer release-mode examples for quick sanity checks and Criterion suites
for workload data.

Runtime instrumentation is exposed through the core APIs used by benchmarks and
integration tests: prepare/execute counts, transaction begins/commits/rollbacks,
and cache hits/misses can be captured while reproducing workloads.

## Schema Compatibility Matrix

| Version | Description |
|---------|-------------|
| 1 | Base tables (`graph_entities`, `graph_edges`, `graph_labels`, `graph_properties`) plus indexes and `graph_meta`. |
| 2 | Adds `graph_meta_history` rows so each migration application is recorded; exposed via `run_pending_migrations` / CLI `migrate`. |
| Future | The CLI refuses to open DBs whose version exceeds the compiled `SCHEMA_VERSION`. |

Upgrade workflow:
1. Inspect the database with `sqlitegraph --db <path> status`.
2. Review pending migrations through the library migration helpers.
3. Apply migrations atomically through the library helper; history entries are appended automatically.

## Ecosystem

Tools built on SQLiteGraph:

| Tool | Purpose | Repository | crates.io |
|------|---------|------------|-----------|
| **Magellan** | Code graph indexing, symbol navigation | [github.com/oldnordic/magellan](https://github.com/oldnordic/magellan) | [crates.io/crates/magellan](https://crates.io/crates/magellan) |
| **llmgrep** | Semantic code search | [github.com/oldnordic/llmgrep](https://github.com/oldnordic/llmgrep) | [crates.io/crates/llmgrep](https://crates.io/crates/llmgrep) |
| **Mirage** | CFG analysis, path enumeration | [github.com/oldnordic/mirage](https://github.com/oldnordic/mirage) | [crates.io/crates/mirage-analyzer](https://crates.io/crates/mirage-analyzer) |
| **splice** | Precision code editing | [github.com/oldnordic/splice](https://github.com/oldnordic/splice) | [crates.io/crates/splice](https://crates.io/crates/splice) |

## Documentation

- [Architecture](https://github.com/oldnordic/sqlitegraph/blob/main/docs/ARCHITECTURE.md) - System design
- [Manual](https://github.com/oldnordic/sqlitegraph/blob/main/MANUAL.md) - API guide
- [Query Language](https://github.com/oldnordic/sqlitegraph/blob/main/docs/QUERY_LANGUAGE.md) - Cypher-inspired query reference
- [Changelog](https://github.com/oldnordic/sqlitegraph/blob/main/CHANGELOG.md) - Version history
- [SnapshotId Migration Guide](https://github.com/oldnordic/sqlitegraph/blob/main/docs/SNAPSHOTID_MIGRATION.md) - v2.1.2 API changes

## License

GPL-3.0-only
