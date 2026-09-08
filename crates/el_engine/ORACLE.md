# Oracle connections (EL)

Oracle is a **source** (full and incremental extraction, table explorer,
ad-hoc query) and a **target** (full refresh and incremental MERGE). The
driver — Oracle's pure-Rust `oracledb`, pinned at a beta — is compiled
into `zdbt-el-worker` only; the IDE never links it. Every credential
travels as a `${VAR}` reference in `el/connections.yml`, resolved from
`.env` and handed to the worker through its environment.

## Two drivers: thin by default, thick for Oracle 10g and 11g

The worker talks to Oracle through Oracle's own pure-Rust **thin driver**
(`oracledb`), which speaks the wire protocol itself. Nothing is installed
on the IDE host, on a `zdbt-el-serve` server or in the Docker image, and
it works the same on macOS (Apple Silicon included), Linux and Windows.
It connects to **Oracle Database 12.1 and later**.

Servers before 12.1 — **10g and 11g** — speak an older protocol the thin
driver refuses at the handshake. For those the worker carries a second,
**thick driver** (ODPI-C over Oracle Instant Client), used when a
connection says `driver: thick`, or automatically under the default
`driver: auto` when the thin driver is refused for the server's version.
A modern database never touches the thick driver. The thick driver has
two constraints the thin one does not:

- **It needs Oracle Instant Client on the machine running the worker.**
  The freely downloadable **19c** client reaches both 11g and 10g:
  officially 11.2.0.4 and later, and 10g as well once its `sqlnet.ora`
  allows the old logon protocol — put
  `SQLNET.ALLOWED_LOGON_VERSION_CLIENT=8` (and `_SERVER=8`) in a file the
  worker sees through `TNS_ADMIN`. Verified against 10.2.0.1 and 11.2.0.2.
  Builds: `instantclient-basiclite-linux.x64-19.x.0.0.0dbru.zip` and
  `…linux.arm64-19.x…` under
  `download.oracle.com/otn_software/linux/instantclient/<version>/`, no
  login needed. Name its directory in `ZDBT_EL_ORACLE_CLIENT_DIR` **and**,
  on Linux, put it on the loader path too (`LD_LIBRARY_PATH`, or a file in
  `/etc/ld.so.conf.d` plus `ldconfig`): ODPI-C loads `libclntsh` from the
  named directory, but that library's own siblings (`libnnz`,
  `libclntshcore`) load through the system loader. On a server,
  `deploy/el-serve/install.sh --oracle-client-url <zip>` installs the
  client and `libaio` and writes both variables into
  `/etc/zdbt-el-serve/env`.
- **It cannot run on an Apple Silicon Mac**: Oracle's only ARM64 macOS
  client (23.3) crashes inside its own crypto library at connect (Oracle
  bug 36790189), and no older client exists for that platform. So a 10g
  or 11g pipeline runs on a **Linux remote** (deploy it there); on a Mac
  the worker refuses with a message saying so rather than dying.

Both drivers share everything else: the extractor, the explorer, the
loader, the type tables and the SQL they send.

**Browsing a 10g / 11g database from a Mac** is done through a remote's
worker: pick the remote in the EL panel's **worker** dropdown (next to the
profile) and every table listing, ad-hoc query and stream preview is sent
to that daemon, which runs it with its own worker, connections.yml,
profile and Instant Client. The daemon answers on `/explore/tables`,
`/explore/query` and `/explore/preview`, token-guarded like the rest of
its API. Pipeline runs on a remote still need an explicit deploy.

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
    # driver: thick                         # auto (default) | thin | thick — 10g/11g
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
| `ZDBT_EL_ORACLE_DRIVER` | the app, from `driver` | which driver the worker connects with |
| `ZDBT_EL_ORACLE_CLIENT_DIR` | you, in `.env` or the server env | where Instant Client lives (thick driver only); the app forwards it |
| `ZDBT_EL_ORACLE_TRY_MACOS_CLIENT` | you | lets the thick driver try on an Apple Silicon Mac anyway |
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
and CI skip them. Against a 12.1+ database they run on any machine, a Mac
included. To exercise the thick driver, point them at a 10g / 11g
database from a Linux box (or a container on the database's Docker
network) with `ZDBT_EL_ORACLE_CLIENT_DIR` exported and the
`oracle-thick` feature on; `driver: auto` then falls back by itself.

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

- **"this Oracle server is older than 12.1"** — a 10g / 11g server on a
  worker without the thick driver, or with `driver: thin`. Set
  `driver: auto` (or `thick`), install the matching Instant Client on a
  Linux worker and deploy there.
- **`DPI-1047`** — the thick driver was chosen but Instant Client is
  missing or invisible to the *worker* process: name its directory in
  `ZDBT_EL_ORACLE_CLIENT_DIR` and, on Linux, in `LD_LIBRARY_PATH` as well
  (in `.env` for the IDE, in `/etc/zdbt-el-serve/env` for a server). A
  message naming `libnnz` or `libclntshcore` means the second half is
  missing.
- **"the thick Oracle driver cannot run on this Mac"** — a 10g / 11g
  server reached from Apple Silicon. Deploy to a Linux remote.
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
  for the thin driver. For a 10g / 11g server the container needs the
  Instant Client mounted in and named (the image carries `libaio`):
  `-v /opt/oracle/instantclient_19_26:/opt/oracle/instantclient:ro
  -e ZDBT_EL_ORACLE_CLIENT_DIR=/opt/oracle/instantclient
  -e LD_LIBRARY_PATH=/opt/oracle/instantclient`, plus a wallet or
  `tnsnames.ora` directory when a connection names one. With
  `--network host` the daemon reaches databases on the host's loopback
  and its port 7431 is reachable through an SSH tunnel without opening a
  firewall.
