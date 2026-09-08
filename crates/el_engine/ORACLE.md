# Oracle connections (EL)

Oracle is a **source** (full and incremental extraction, table explorer,
ad-hoc query) and a **target** (full refresh and incremental MERGE). The
driver — Oracle's pure-Rust `oracledb`, pinned at a beta — is compiled
into `zdbt-el-worker` only; the IDE never links it. Every credential
travels as a `${VAR}` reference in `el/connections.yml`, resolved from
`.env` and handed to the worker through its environment.

## No Oracle client software is needed

The worker talks to Oracle through Oracle's own pure-Rust thin driver
(`oracledb`), which speaks the wire protocol itself. Nothing is installed
on the IDE host, on a `zdbt-el-serve` server or in the Docker image, and
it works the same on macOS (Apple Silicon included), Linux and Windows.
It connects to Oracle Database 12 and later.

Two things the driver reads from disk when a connection names them: a
`tnsnames.ora` (for a TNS alias in `connect`) and a wallet's `ewallet.pem`
(Autonomous Database with mutual TLS), both looked up in the directory
`tns_admin` / `wallet_dir` points at. A wallet that needs its own
password is not supported yet; TLS-only Autonomous connections (no
wallet) need nothing.

## Connection

```yaml
version: 1
connections:
  ora_erp:
    type: oracle
    user: "${ORACLE_USER}"
    password: "${ORACLE_PASSWORD}"          # always a ${VAR}, never a literal
    connect: db.example.com:1521/ORCLPDB1   # host:port/service_name, or a TNS alias
    schema: ERP                             # optional; defaults to the user's schema
    # wallet_dir: /etc/zdbt/wallet          # Autonomous Database wallet
    # tns_admin: /etc/zdbt/network/admin    # dir holding tnsnames.ora / sqlnet.ora
```

`connect` is an Easy Connect string or an alias from `tnsnames.ora`. When
`tns_admin` (or, failing that, `wallet_dir`) is set — relative to the
project when not absolute — the worker receives it as `TNS_ADMIN` and
hands it to the driver as its configuration and wallet directory.

The matching `.env` (next to the project, never committed):

```sh
ORACLE_USER=zdbt
ORACLE_PASSWORD=…
```

Only `${VAR}` names appear in the YAML; values live in `.env`, `.env.local`
or the real environment, and on a server in `/etc/zdbt-el-serve/env`.

A bare `NUMBER` column (no precision, no scale — the idiomatic Oracle
declaration) is read as an exact `Decimal(38,10)`, never a float, so a
19-digit id survives; `NUMBER(*,s)` keeps its scale. Put a `cast:` on the
stream column to choose another scale. `FLOAT`, `BINARY_FLOAT` and
`BINARY_DOUBLE` are floating point and read as such.

Identifiers follow Oracle's own folding: a name written unquoted in the
spec is upper-cased (`orders` addresses `ORDERS`, which is what
`CREATE TABLE orders` really made). Quote it in the spec — `"MixedCase"` —
to address a case-sensitive name.

### As a target

```yaml
target: { connection: ora_erp, schema: ANALYTICS, table: "{stream}" }
```

`full_refresh` publishes by rename (`ORDERS__ZDBT_STAGING` → `ORDERS`, the
previous table stepping aside as `ORDERS__ZDBT_OLD` before being dropped),
so a concurrent reader can see `ORA-00942` for a moment, and grants,
synonyms and indexes attached to the previous table do not survive the
swap. `incremental` never renames: it MERGEs staging into the live table
and reads `MAX(update_key)` back as the next watermark.

## Environment variables

| Variable | Set by | Purpose |
| --- | --- | --- |
| `ZDBT_EL_SRC_ORACLE_USER` / `_PASSWORD` / `_CONNECT` | the app, on the worker it spawns | source credentials |
| `ZDBT_EL_ORACLE_PASSWORD` | the app, on the loader sidecar | target password |
| `TNS_ADMIN` | the app, from `tns_admin`/`wallet_dir` (relative to the project) | wallet / `tnsnames.ora` directory |
| `EL_ORACLE_SMOKE_URL` / `_USER` / `_PASSWORD` | you | the live tests below |

The first three are the contract between the app and the worker: they are
never passed on the command line and never logged. You do not set them.

## A local Oracle for testing

`docker/oracle-test/` runs Oracle Database 23ai Free (multi-arch, so it
works on Apple Silicon) with a seeded demo schema:

```sh
ORACLE_PASSWORD=…sys-password… APP_USER_PASSWORD=…zdbt-password… \
  docker compose -f docker/oracle-test/compose.yml up -d --wait
```

First start takes a couple of minutes even on the `faststart` image;
`--wait` blocks until the healthcheck passes. It listens on
`127.0.0.1:1521`, service `FREEPDB1`, and the seed creates user `ZDBT`
with `DEMO_CUSTOMERS`, `DEMO_ORDERS` (NUMBER / VARCHAR2 / BINARY_DOUBLE /
BOOLEAN / DATE / TIMESTAMP / TIMESTAMP WITH TIME ZONE / CLOB / RAW) and a
view. `docker compose -f docker/oracle-test/compose.yml down -v` removes
it, database files included.

### The gated tests

Both need a reachable database; both are `#[ignore]` so `cargo test`
and CI skip them. They run on any machine, a Mac included.

```sh
export EL_ORACLE_SMOKE_URL=127.0.0.1:1521/FREEPDB1
export EL_ORACLE_SMOKE_USER=zdbt
export EL_ORACLE_SMOKE_PASSWORD=…zdbt-password…

# Source: type mapping, chunking, incremental cursor.
cargo test -p el_engine --features oracle -- --ignored oracle_smoke --nocapture

# Target: DuckDB → Oracle, rename swap then MERGE, watermark read-back.
cargo test -p el_worker -- --ignored oracle_target --nocapture
```

Each test creates and drops its own tables (`ZDBT_EL_SMOKE`,
`ZDBT_EL_ORDERS`) in the user's own schema.

### The IDE by hand

1. Build the worker and let the IDE find it:
   `cargo build -p el_worker`, then either put the binary beside the IDE's
   own or export `ZDBT_EL_WORKER=target/debug/zdbt-el-worker` before
   launching.
2. In the EL panel, **Connections +** → **oracle**: user
   `${ORACLE_USER}`, password `${ORACLE_PASSWORD}`, connect
   `127.0.0.1:1521/FREEPDB1`, schema `ZDBT`. Save.
3. Put the two values in the project's `.env`.
4. Expand the connection in the sidebar: the table list (every owner
   outside the data dictionary) should include `ZDBT.DEMO_CUSTOMERS`,
   `ZDBT.DEMO_ORDERS` and `ZDBT.DEMO_ACTIVE_CUSTOMERS`.
5. **Query** tab: `SELECT * FROM ZDBT.DEMO_ORDERS` returns rows (the worker
   caps it with `FETCH FIRST n ROWS ONLY`).
6. New pipeline, source `ZDBT.DEMO_ORDERS`, target a DuckDB connection, run
   it; then flip the stream to `incremental` with `update_key: UPDATED_AT`,
   insert a row in the container and run again — only the new row moves.
7. For the target direction, point a pipeline's target at the Oracle
   connection with `schema: ZDBT` and run it twice.

## When something fails

- **"does not fit VARCHAR2(4000 CHAR)"** — a text value is wider than
  Oracle's 4000-byte VARCHAR2 ceiling. Add `cast: VARCHAR(4001)` (or any
  length above 4000) to the stream column so it is created as CLOB. A CLOB
  cannot be a primary key or update key.
- **`ORA-12154` / `ORA-12541`** — `connect` did not resolve. Easy Connect
  needs `host:port/service_name`; an alias needs `tns_admin` pointing at
  the directory holding `tnsnames.ora`.
- **`ORA-00942` on a table you can see in SQL*Plus** — a folding mismatch:
  the spec's unquoted name was upper-cased. Quote it to keep the case.
- **Docker images** — `docker/el-serve/Dockerfile*` need nothing extra
  for Oracle; only a wallet or `tnsnames.ora` directory has to be mounted
  in when a connection names one.
