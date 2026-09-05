//! Spec parse/serialize round-trips and the validation matrix.

use el_engine::spec::{self, Mode};

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
