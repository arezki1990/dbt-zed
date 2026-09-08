//! Spec parse/serialize round-trips and the validation matrix.

use el_engine::spec::{self, Connection, Mode};

const PIPELINE: &str = r#"version: 1
pipeline: crm_to_raw
source: pg_prod
target:
  connection: warehouse
  database: RAW
  schema: CRM
  table: '{stream}'
streams:
- name: customers
  source:
    schema: public
    table: customers
  mode: incremental
  primary_key:
  - id
  update_key: updated_at
  select:
    include:
    - id
    - email
    - updated_at
  columns:
  - name: id
    cast: NUMBER(38,0)
    strict: true
  - name: updated_at
    cast: TIMESTAMP_NTZ
    parse: '%Y-%m-%d %H:%M:%S'
  - name: email
    rename: EMAIL_ADDRESS
- name: events
  source:
    path: exports/events.parquet
    format: parquet
canvas:
  nodes:
    stream:customers:
      x: 40.0
      y: 120.0
    cast:
      x: 340.0
      y: 160.0
"#;

const CONNECTIONS: &str = r#"version: 1
connections:
  pg_prod:
    type: postgres
    url: ${PG_PROD_URL}
  warehouse:
    type: snowflake
    account: ${SNOWFLAKE_ACCOUNT}
    user: loader
    auth:
      method: key_pair
      private_key_path: ${SNOWFLAKE_PK_PATH}
"#;

fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn pipeline_round_trip_is_byte_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "p.yml", PIPELINE);
    let pipeline = spec::load_pipeline(&path).unwrap();

    assert_eq!(pipeline.pipeline, "crm_to_raw");
    assert_eq!(pipeline.streams.len(), 2);
    assert_eq!(pipeline.streams[0].mode(None), Mode::Incremental);
    assert_eq!(
        pipeline.streams[0].target_table(&pipeline.target),
        "CUSTOMERS"
    );
    let canvas = pipeline.canvas.as_ref().unwrap();
    assert_eq!(canvas.nodes["stream:customers"].x, 40.0);

    // First write establishes canonical form; the second must be identical.
    let once = spec::to_canonical_yaml(&pipeline);
    let path2 = write(dir.path(), "p2.yml", &once);
    let reloaded = spec::load_pipeline(&path2).unwrap();
    let twice = spec::to_canonical_yaml(&reloaded);
    assert_eq!(once, twice, "canonical form must be a fixed point");
}

#[test]
fn unknown_keys_survive_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let with_extra = PIPELINE.replace("streams:", "my_custom_note: keep me\nstreams:");
    let path = write(dir.path(), "p.yml", &with_extra);
    let pipeline = spec::load_pipeline(&path).unwrap();
    let out = spec::to_canonical_yaml(&pipeline);
    assert!(out.contains("my_custom_note: keep me"), "{out}");
}

#[test]
fn write_warns_on_foreign_comments() {
    let dir = tempfile::tempdir().unwrap();
    let commented = format!("# my precious note\n{PIPELINE}");
    let path = write(dir.path(), "p.yml", &commented);
    let pipeline = spec::load_pipeline(&path).unwrap();
    let warnings = spec::write_pipeline(&pipeline, &path).unwrap();
    assert_eq!(warnings, vec![spec::WriteWarning::CommentsWillBeDropped]);
    // Second write of the now-managed file warns no more.
    let pipeline = spec::load_pipeline(&path).unwrap();
    let warnings = spec::write_pipeline(&pipeline, &path).unwrap();
    assert!(warnings.is_empty());
}

#[test]
fn validation_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let connections = spec::load_connections(&write(dir.path(), "c.yml", CONNECTIONS)).unwrap();

    // Broken pipeline: unknown source conn, non-snowflake target, missing
    // pk/cursor, include+exclude conflict, misplaced parse, file+incremental.
    let broken = r#"version: 1
pipeline: broken
source: ghost
target:
  connection: pg_prod
  schema: X
streams:
- name: s1
  source: { schema: public, table: t }
  mode: incremental
- name: s1
  source: { path: f.csv, format: csv }
  mode: incremental
  select:
    include: [a]
    exclude: [b]
  columns:
  - name: a
    cast: FLOAT
    parse: '%Y'
"#;
    let pipeline = spec::load_pipeline(&write(dir.path(), "b.yml", broken)).unwrap();
    let issues = spec::validate(&pipeline, &connections);
    let text = issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "source connection \"ghost\"",
        "must be snowflake",
        "requires primary_key",
        "requires update_key",
        "duplicate stream name",
        "mutually exclusive",
        "parse: but no temporal cast",
        "file sources do not support incremental",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn json_schemas_emit() {
    let pipeline_schema = spec::pipeline_json_schema();
    assert!(pipeline_schema["properties"]["streams"].is_object());
    let connections_schema = spec::connections_json_schema();
    assert!(connections_schema["properties"]["connections"].is_object());
    // Every connection kind is offered for completion, oracle included.
    let text = serde_json::to_string(&connections_schema).unwrap();
    for kind in ["postgres", "duckdb", "snowflake", "oracle"] {
        assert!(text.contains(&format!("\"{kind}\"")), "kind {kind} missing from schema");
    }
    assert!(text.contains("tns_admin"), "oracle fields missing from schema");
}

/// An oracle connection: discrete fields, ${VAR} credentials, optional
/// schema/wallet/tns_admin, unknown keys kept — and the file writes back
/// byte-stable through the canonical writer.
#[test]
fn oracle_connection_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "version: 1
connections:
  ora:
    type: oracle
    user: ${ORACLE_USER}
    password: ${ORACLE_PASSWORD}
    connect: db.example.com:1521/ORCLPDB1
    schema: ERP
    tns_admin: /opt/oracle/network/admin
    my_note: keep me
";
    let loaded = spec::load_connections(&write(dir.path(), "c.yml", yaml)).unwrap();
    let conn = &loaded.connections["ora"];
    assert_eq!(conn.kind(), "oracle");
    assert_eq!(conn.env_refs(), ["ORACLE_PASSWORD", "ORACLE_USER"]);
    let keys = conn.param_keys();
    for key in ["user", "password", "connect", "schema", "tns_admin", "my_note"] {
        assert!(keys.iter().any(|k| k == key), "missing key {key} in {keys:?}");
    }
    assert!(!keys.iter().any(|k| k == "wallet_dir"), "unset optional listed: {keys:?}");
    assert!(conn.shape_issues().is_empty(), "{:?}", conn.shape_issues());
    let Connection::Oracle(oracle) = conn else {
        panic!("not oracle");
    };
    assert_eq!(oracle.effective_schema(), "ERP");
    assert_eq!(oracle.wallet_dir, None);

    let rewritten = spec::to_canonical_connections_yaml(&loaded);
    assert!(rewritten.contains("type: oracle"), "{rewritten}");
    assert!(rewritten.contains("my_note: keep me"), "lost my_note:\n{rewritten}");
    assert!(!rewritten.contains("wallet_dir"), "unset optional written:\n{rewritten}");
    let reloaded = spec::load_connections(&write(dir.path(), "c2.yml", &rewritten)).unwrap();
    assert_eq!(spec::to_canonical_connections_yaml(&reloaded), rewritten);
    // Older kinds keep loading alongside it.
    let mixed = spec::load_connections(&write(dir.path(), "c3.yml", CONNECTIONS)).unwrap();
    assert_eq!(mixed.connections.len(), 2);
}

/// Oracle sources validate like the other database kinds; the shape
/// checks name fields and variables, never the values.
#[test]
fn oracle_validation_messages() {
    let dir = tempfile::tempdir().unwrap();
    let connections_yaml = "version: 1
connections:
  ora:
    type: oracle
    user: scott
    password: tiger
    connect: ''
  warehouse:
    type: snowflake
    account: ${SNOWFLAKE_ACCOUNT}
    user: loader
    auth:
      method: key_pair
      private_key_path: ${SNOWFLAKE_PK_PATH}
";
    let pipeline_yaml = "version: 1
pipeline: p
source: ora
target:
  connection: warehouse
  schema: RAW
streams:
  - name: emp
    source:
      schema: SCOTT
      table: EMP
";
    let connections =
        spec::load_connections(&write(dir.path(), "c.yml", connections_yaml)).unwrap();
    let pipeline = spec::load_pipeline(&write(dir.path(), "p.yml", pipeline_yaml)).unwrap();
    let issues = spec::validate(&pipeline, &connections);
    let text = issues
        .iter()
        .map(|issue| issue.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("table sources need a database connection"), "{text}");
    assert!(text.contains("need a connect string"), "{text}");
    assert!(text.contains("password is stored literally"), "{text}");
    assert!(!text.contains("tiger"), "password leaked: {text}");
    // Oracle loads as well as it reads: as a target it is accepted, and a
    // kind with no loader still is not.
    let reversed = pipeline_yaml
        .replace("source: ora", "source: warehouse")
        .replace("connection: warehouse", "connection: ora");
    let pipeline = spec::load_pipeline(&write(dir.path(), "p2.yml", &reversed)).unwrap();
    let issues = spec::validate(&pipeline, &connections);
    assert!(
        !issues.iter().any(|issue| issue.message.contains("targets must be")),
        "{issues:?}"
    );
    let postgres_target = "version: 1
connections:
  ora:
    type: oracle
    user: scott
    password: ${ORACLE_PASSWORD}
    connect: db.example.com:1521/ORCLPDB1
  warehouse:
    type: postgres
    url: ${PG_URL}
";
    let connections =
        spec::load_connections(&write(dir.path(), "c2.yml", postgres_target)).unwrap();
    let pipeline = spec::load_pipeline(&write(dir.path(), "p3.yml", pipeline_yaml)).unwrap();
    let issues = spec::validate(&pipeline, &connections);
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("targets must be snowflake, duckdb or oracle")),
        "{issues:?}"
    );
}

#[test]
fn env_refs_are_names_only() {
    let dir = tempfile::tempdir().unwrap();
    let connections = spec::load_connections(&write(dir.path(), "c.yml", CONNECTIONS)).unwrap();
    let refs = connections.connections["warehouse"].env_refs();
    assert_eq!(refs, ["SNOWFLAKE_ACCOUNT", "SNOWFLAKE_PK_PATH"]);
}

/// Hand-added unknown keys on connections must survive a canonical
/// rewrite — the connection editor re-serializes the whole file, and a
/// key the UI doesn't know about is the user's, not garbage.
#[test]
fn connection_unknown_keys_survive_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "version: 1
connections:
  pg:
    type: postgres
    url: ${PG_URL}
    my_note: keep me
  duck:
    type: duckdb
    path: el/w.duckdb
    pool_size: 4
";
    let loaded = spec::load_connections(&write(dir.path(), "c.yml", yaml)).unwrap();
    let rewritten = spec::to_canonical_connections_yaml(&loaded);
    assert!(rewritten.contains("my_note: keep me"), "lost my_note:\n{rewritten}");
    assert!(rewritten.contains("pool_size: 4"), "lost pool_size:\n{rewritten}");
    // And a second load of the rewrite still parses to the same map.
    let reloaded = spec::load_connections(&write(dir.path(), "c2.yml", &rewritten)).unwrap();
    assert_eq!(reloaded.connections.len(), 2);
}

/// Profiles: same logical names, per-environment connections. Base is the
/// fallback; the active profile overrides and extends; unknown profiles
/// are hard errors (never a silent fall-through to another environment).
#[test]
fn profiles_resolve_and_guard() {
    let yaml = "version: 1
default_profile: dev
connections:
  wh: { type: duckdb, path: el/base.duckdb }
  files: { type: local }
profiles:
  dev:
    connections:
      pg: { type: postgres, url: '${PG_DEV}' }
  prod:
    connections:
      wh: { type: snowflake, account: '${SF_ACCOUNT}', user: '${SF_USER}',
            auth: { method: key_pair, private_key_path: '${SF_KEY}' } }
      pg: { type: postgres, url: '${PG_PROD}' }
";
    let dir = tempfile::tempdir().unwrap();
    let raw = spec::load_connections(&write(dir.path(), "c.yml", yaml)).unwrap();

    // Base only.
    let base = raw.resolved(None).unwrap();
    assert_eq!(base["wh"].kind(), "duckdb");
    assert!(!base.contains_key("pg"));

    // dev: base wh survives, pg added.
    let dev = raw.resolved(Some("dev")).unwrap();
    assert_eq!(dev["wh"].kind(), "duckdb");
    assert_eq!(dev["pg"].kind(), "postgres");
    assert_eq!(dev["files"].kind(), "local");

    // prod: wh overridden to snowflake.
    let prod = raw.resolved(Some("prod")).unwrap();
    assert_eq!(prod["wh"].kind(), "snowflake");

    // Unknown profile = error naming the real ones.
    let error = raw.resolved(Some("staging")).unwrap_err();
    assert!(error.contains("staging") && error.contains("dev"), "{error}");

    // default_profile drives selection when nothing else picks.
    assert_eq!(
        spec::active_profile(dir.path(), &raw).as_deref(),
        Some("dev")
    );
    // The local selection file outranks the default.
    std::fs::create_dir_all(dir.path().join("el/.zdbt")).unwrap();
    std::fs::write(dir.path().join("el/.zdbt/profile"), "prod\n").unwrap();
    assert_eq!(
        spec::active_profile(dir.path(), &raw).as_deref(),
        Some("prod")
    );

    // Round-trip keeps the profiles block.
    let rewritten = spec::to_canonical_connections_yaml(&raw);
    assert!(rewritten.contains("profiles:"), "{rewritten}");
    assert!(rewritten.contains("default_profile: dev"), "{rewritten}");
}

/// Renaming re-keys the stream's canvas entries in place (order and
/// unrelated keys kept), refuses empty and duplicate names, and the
/// canonical form stays a fixed point.
#[test]
fn rename_stream_rekeys_canvas_and_rejects_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let mut pipeline = spec::load_pipeline(&write(dir.path(), "p.yml", PIPELINE)).unwrap();
    // Pin the other two keys of the row so every prefix is exercised.
    let canvas = pipeline.canvas.as_mut().unwrap();
    canvas.nodes.insert("map:customers".into(), spec::NodePos { x: 1., y: 2. });
    canvas.nodes.insert("target:customers".into(), spec::NodePos { x: 3., y: 4. });

    assert_eq!(pipeline.rename_stream("customers", "  "), Err("stream name can't be empty".into()));
    assert_eq!(
        pipeline.rename_stream("customers", "events"),
        Err("events already exists in this pipeline".into())
    );
    assert!(pipeline.rename_stream("ghost", "x").unwrap_err().contains("gone"));

    pipeline.rename_stream("customers", " clients ").unwrap();
    assert_eq!(pipeline.streams[0].name, "clients");
    // target_table is untouched: the derived name follows the stream.
    assert_eq!(pipeline.streams[0].target_table(&pipeline.target), "CLIENTS");
    let keys: Vec<&str> = pipeline.canvas.as_ref().unwrap().nodes.keys().map(String::as_str).collect();
    assert_eq!(keys, ["stream:clients", "cast", "map:clients", "target:clients"]);
    assert_eq!(pipeline.canvas.as_ref().unwrap().nodes["stream:clients"].x, 40.0);
    assert_eq!(pipeline.canvas.as_ref().unwrap().nodes["target:clients"].y, 4.0);

    let once = spec::to_canonical_yaml(&pipeline);
    let reloaded = spec::load_pipeline(&write(dir.path(), "p2.yml", &once)).unwrap();
    assert_eq!(once, spec::to_canonical_yaml(&reloaded));
}

/// Two streams loading one table overwrite each other on every run: the
/// rename refuses up front and validate flags the state however it arose.
#[test]
fn colliding_target_tables_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let connections = spec::load_connections(&write(dir.path(), "c.yml", CONNECTIONS)).unwrap();
    let mut pipeline = spec::load_pipeline(&write(dir.path(), "p.yml", PIPELINE)).unwrap();
    pipeline.streams[1].target_table = Some("CLIENTS".into());
    assert!(
        !spec::validate(&pipeline, &connections)
            .iter()
            .any(|issue| issue.message.contains("both load table"))
    );

    // "clients" derives CLIENTS, which the events stream already pins.
    let refused = pipeline.rename_stream("customers", "clients").unwrap_err();
    assert!(refused.contains("same table as events"), "unexpected: {refused}");
    assert_eq!(pipeline.streams[0].name, "customers");

    // Hand-edited YAML reaches the state anyway — validate says so.
    pipeline.streams[0].target_table = Some("clients".into());
    let text = spec::validate(&pipeline, &connections)
        .iter()
        .map(|issue| issue.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("both load table"), "missing collision in:\n{text}");
}

#[test]
fn remove_stream_drops_canvas_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut pipeline = spec::load_pipeline(&write(dir.path(), "p.yml", PIPELINE)).unwrap();
    assert!(!pipeline.remove_stream("ghost"));
    assert!(pipeline.remove_stream("customers"));
    assert_eq!(pipeline.streams.len(), 1);
    assert_eq!(pipeline.streams[0].name, "events");
    let keys: Vec<&str> = pipeline.canvas.as_ref().unwrap().nodes.keys().map(String::as_str).collect();
    assert_eq!(keys, ["cast"]);

    // A canvas holding only that stream's keys goes away entirely.
    let mut pipeline = spec::load_pipeline(&write(dir.path(), "p3.yml", PIPELINE)).unwrap();
    pipeline.canvas.as_mut().unwrap().nodes.shift_remove("cast");
    assert!(pipeline.remove_stream("customers"));
    assert!(pipeline.canvas.is_none());
    assert!(!spec::to_canonical_yaml(&pipeline).contains("canvas:"));
}

#[test]
fn move_stream_swaps_and_clamps() {
    let dir = tempfile::tempdir().unwrap();
    let mut pipeline = spec::load_pipeline(&write(dir.path(), "p.yml", PIPELINE)).unwrap();
    let names = |pipeline: &spec::Pipeline| -> Vec<String> {
        pipeline.streams.iter().map(|stream| stream.name.clone()).collect()
    };
    assert!(!pipeline.move_stream("customers", true));
    assert!(!pipeline.move_stream("ghost", false));
    assert_eq!(names(&pipeline), ["customers", "events"]);

    // Only customers is pinned: the pin stays put.
    assert!(pipeline.move_stream("customers", false));
    assert_eq!(names(&pipeline), ["events", "customers"]);
    assert_eq!(pipeline.canvas.as_ref().unwrap().nodes["stream:customers"].y, 120.0);
    assert!(!pipeline.move_stream("customers", false));

    // Both pinned: the pins follow the swap.
    pipeline
        .canvas
        .as_mut()
        .unwrap()
        .nodes
        .insert("stream:events".into(), spec::NodePos { x: 40., y: 40. });
    assert!(pipeline.move_stream("customers", true));
    assert_eq!(names(&pipeline), ["customers", "events"]);
    let nodes = &pipeline.canvas.as_ref().unwrap().nodes;
    assert_eq!(nodes["stream:customers"].y, 40.0);
    assert_eq!(nodes["stream:events"].y, 120.0);
}
