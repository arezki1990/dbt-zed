# Oracle connections (EL)

Oracle is a **source** (full and incremental extraction, table explorer,
ad-hoc query) and a **target** (full refresh and incremental MERGE). The
driver is compiled into `zdbt-el-worker` only — the IDE never links it —
and every credential travels as a `${VAR}` reference in
`el/connections.yml`, resolved from `.env` and handed to the worker
through its environment.

## Oracle Instant Client is required at runtime

The driver (`oracle` → ODPI-C) builds without any Oracle software, but it
loads `libclntsh` when a connection is opened. **Whichever machine runs
`zdbt-el-worker` needs Oracle Instant Client** — the IDE host when you
work locally, or the `zdbt-el-serve` host when the pipeline runs on a
remote server. A missing client surfaces as `DPI-1047` and the worker
turns it into install instructions.

Tell the worker where the client is with **`ZDBT_EL_ORACLE_CLIENT_DIR`**:
in the project's `.env` next to the credentials, or in the process
environment (on a server, `/etc/zdbt-el-serve/env`). The worker hands that
directory to the driver explicitly, which is the only placement that
works everywhere: macOS 12+ no longer searches `~/lib`, and a signed app
strips `DYLD_*` / `LD_*` variables from the processes it starts.

Instant Client **Basic Light** (~35 MB) is enough for everything the
connector does; Basic adds the character sets Basic Light omits — take it
if your data is not US7ASCII/WE8*/UTF-8. Downloads are under Oracle's OTN
licence; using them means accepting it.

**Linux (x86-64 or ARM64)** — the production path, and what
`deploy/el-serve/install.sh --oracle-client` automates on a server:

```sh
sudo mkdir -p /opt/oracle && cd /opt/oracle
sudo curl -fLO https://download.oracle.com/otn_software/linux/instantclient/instantclient-basiclite-linuxx64.zip
sudo unzip -q instantclient-basiclite-linuxx64.zip      # → /opt/oracle/instantclient_23_x
sudo apt-get install -y libaio1 || sudo apt-get install -y libaio1t64
echo ZDBT_EL_ORACLE_CLIENT_DIR=/opt/oracle/instantclient_23_x >> ~/my-project/.env
```

(ARM64: `instantclient-basiclite-linux-arm64.zip`. Ubuntu 24.04 ships
`libaio1t64` instead of `libaio1`, hence the fallback.) Registering the
directory with `ldconfig` or `LD_LIBRARY_PATH` works as well; the
installer writes both variables into `/etc/zdbt-el-serve/env`.

**macOS (Apple Silicon) — not usable.** Instant Client 23.3 is the only
build Oracle publishes for ARM64 Macs (the "latest" link serves the same
2024 file) and it crashes inside its own crypto library the moment a
connection opens (Oracle bug 36790189, reported against
[node-oracledb](https://github.com/oracle/node-oracledb/issues/1723) and
[python-oracledb](https://github.com/oracle/python-oracledb/issues/351);
no published fix). The worker therefore refuses to connect on an Apple
Silicon Mac and says so, instead of dying without a message. Run Oracle
pipelines from a Linux remote (**Remotes → deploy**), and run the table
explorer and ad-hoc queries there too. When Oracle ships a fixed client,
`ZDBT_EL_ORACLE_TRY_MACOS_CLIENT=1` in `.env` lifts the refusal.

**Windows** — unzip `instantclient-basiclite-windows.x64.zip` and name the
directory in `ZDBT_EL_ORACLE_CLIENT_DIR` (adding it to `PATH` works too).

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
`tns_admin` (or, failing that, `wallet_dir`) is set, the worker receives it
as `TNS_ADMIN`.

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
| `ZDBT_EL_ORACLE_CLIENT_DIR` | you, in `.env` or the server env | where Instant Client lives; the app forwards it to the worker |
| `ZDBT_EL_ORACLE_TRY_MACOS_CLIENT` | you | lifts the Apple Silicon refusal once Oracle ships a working client |
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

Both need Instant Client on this machine and a reachable database; both
are `#[ignore]` so `cargo test` and CI skip them. On a Mac they cannot
run (see above): use a Linux box or a container on the same Docker
network as the database, with `ZDBT_EL_ORACLE_CLIENT_DIR` exported.

```sh
export EL_ORACLE_SMOKE_URL=127.0.0.1:1521/FREEPDB1
export EL_ORACLE_SMOKE_USER=zdbt
export EL_ORACLE_SMOKE_PASSWORD=…zdbt-password…
export ZDBT_EL_ORACLE_CLIENT_DIR=/opt/oracle/instantclient_23_x

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
3. Put the two values and `ZDBT_EL_ORACLE_CLIENT_DIR` in the project's
   `.env`. (On a Mac, steps 4–7 happen on a Linux remote: deploy there.)
4. Expand the connection in the sidebar: the table list (every owner
   outside the data dictionary) should include `ZDBT.DEMO_CUSTOMERS`,
   `ZDBT.DEMO_ORDERS` and `ZDBT.DEMO_ACTIVE_CUSTOMERS`. Without Instant
   Client this is where the install message appears.
5. **Query** tab: `SELECT * FROM ZDBT.DEMO_ORDERS` returns rows (the worker
   caps it with `FETCH FIRST n ROWS ONLY`).
6. New pipeline, source `ZDBT.DEMO_ORDERS`, target a DuckDB connection, run
   it; then flip the stream to `incremental` with `update_key: UPDATED_AT`,
   insert a row in the container and run again — only the new row moves.
7. For the target direction, point a pipeline's target at the Oracle
   connection with `schema: ZDBT` and run it twice.

## When something fails

- **`DPI-1047`** — Instant Client is missing or invisible to the *worker*
  process. Name its directory in `ZDBT_EL_ORACLE_CLIENT_DIR`: in `.env`
  for the IDE, in `/etc/zdbt-el-serve/env` for a server (the launcher and
  the systemd unit both source it), not just in your shell.
- **"Oracle pipelines cannot run on this Mac"** — that is the Apple
  Silicon client crash described above, caught before it happens. Deploy
  to a Linux remote.
- **"does not fit VARCHAR2(4000 CHAR)"** — a text value is wider than
  Oracle's 4000-byte VARCHAR2 ceiling. Add `cast: VARCHAR(4001)` (or any
  length above 4000) to the stream column so it is created as CLOB. A CLOB
  cannot be a primary key or update key.
- **`ORA-12154` / `ORA-12541`** — `connect` did not resolve. Easy Connect
  needs `host:port/service_name`; an alias needs `tns_admin` pointing at
  the directory holding `tnsnames.ora`.
- **`ORA-00942` on a table you can see in SQL*Plus** — a folding mismatch:
  the spec's unquoted name was upper-cased. Quote it to keep the case.
- **Docker images** — `docker/el-serve/Dockerfile*` build a slim Debian
  runtime with no Oracle client and no `libaio`. Mount one in and name it:
  `-v /opt/oracle/instantclient_23_x:/opt/oracle/instantclient
  -e ZDBT_EL_ORACLE_CLIENT_DIR=/opt/oracle/instantclient`, with `libaio`
  installed in the image. The client is not redistributable, so it is
  deliberately not baked into the image.
