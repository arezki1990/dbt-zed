//! "Initialize EL workspace": writes a commented starter `el/` tree and
//! wires yaml-language-server completion for the spec files. Only ever run
//! from an explicit user action.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

const CONNECTIONS_EXAMPLE: &str = r#"# el/connections.yml — named connections for EL pipelines.
# Every value may reference environment variables as ${VAR}; put real
# credentials in your environment or a .env file, NEVER in this file.
version: 1
connections:
  files:
    type: local
  duck_demo:
    type: duckdb
    path: el/demo.duckdb           # created by Initialize when connector support is present
  pg_prod:
    type: postgres
    url: "${PG_PROD_URL}"          # postgres://user:pass@host:5432/db
  warehouse:
    type: snowflake
    account: "${SNOWFLAKE_ACCOUNT}"
    user: "${SNOWFLAKE_USER}"
    role: LOADER
    warehouse: LOAD_WH
    database: RAW
    auth:
      method: key_pair
      private_key_path: "${SNOWFLAKE_PK_PATH}"
"#;

const PIPELINE_EXAMPLE: &str = r#"# yaml-language-server: $schema=../.zdbt/el-pipeline.schema.json
# An example pipeline: edit it in YAML or on the canvas — both write this
# file. Preview streams before ever loading: right-click a node.
version: 1
pipeline: example
source: files
target:
  connection: warehouse
  schema: LANDING
  table: '{stream}'
defaults:
  mode: full_refresh
streams:
- name: sample_file
  source:
    path: el/sample.csv
    format: csv
  columns:
  - name: amount
    cast: NUMBER(18,2)
  - name: created_at
    cast: TIMESTAMP_NTZ
    parse: '%Y-%m-%d %H:%M:%S'
"#;

const SAMPLE_CSV: &str = "id,amount,created_at\n1,10.50,2026-01-01 09:00:00\n2,badvalue,2026-01-02 10:30:00\n";

const PIPELINE_DUCKDB_EXAMPLE: &str = r#"# yaml-language-server: $schema=../.zdbt/el-pipeline.schema.json
# A real database source, zero credentials: the demo DuckDB file created
# by Initialize. Preview it from the canvas mapping editor.
version: 1
pipeline: example_duckdb
source: duck_demo
target:
  connection: warehouse
  schema: LANDING
  table: '{stream}'
streams:
- name: demo_orders
  source:
    schema: main
    table: demo_orders
  columns:
  - name: amount
    cast: NUMBER(18,2)
"#;

/// Creates the `el/` tree. Returns the created files; refuses to overwrite
/// anything that exists.
pub fn initialize_el_workspace(project_root: &Path) -> Result<Vec<PathBuf>> {
    let el = super::el_dir(project_root);
    let mut created = Vec::new();
    std::fs::create_dir_all(el.join("pipelines")).context("creating el/pipelines")?;
    std::fs::create_dir_all(el.join(".zdbt")).context("creating el/.zdbt")?;

    let mut write_new = |path: PathBuf, contents: &str| -> Result<()> {
        if !path.exists() {
            std::fs::write(&path, contents)
                .with_context(|| format!("writing {}", path.display()))?;
            created.push(path);
        }
        Ok(())
    };

    write_new(el.join("connections.yml"), CONNECTIONS_EXAMPLE)?;
    write_new(el.join("pipelines").join("example.yml"), PIPELINE_EXAMPLE)?;
    write_new(el.join("sample.csv"), SAMPLE_CSV)?;
    write_new(
        el.join(".zdbt").join("el-pipeline.schema.json"),
        &serde_json::to_string_pretty(&el_engine::spec::pipeline_json_schema())?,
    )?;
    write_new(
        el.join(".zdbt").join("el-connections.schema.json"),
        &serde_json::to_string_pretty(&el_engine::spec::connections_json_schema())?,
    )?;

    // When connector support is installed, seed a demo DuckDB database and
    // a pipeline that reads from it — a real database stream, zero setup.
    if let Some(worker) = super::find_worker() {
        let demo_db = el.join("demo.duckdb");
        let seeded = std::process::Command::new(&worker)
            .arg("seed-demo")
            .arg(&demo_db)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if seeded {
            write_new(
                el.join("pipelines").join("example_duckdb.yml"),
                PIPELINE_DUCKDB_EXAMPLE,
            )?;
        }
    }

    merge_yaml_schema_settings(project_root)?;
    Ok(created)
}

/// Self-healing for the JSON schemas the canonical YAML header points at:
/// (re)writes `el/.zdbt/*.schema.json` when missing or empty, so hand-made
/// EL projects (no Initialize run) still get completion and validation.
/// Derived artifacts, not user YAML — plain fs writes are fine.
pub fn ensure_schemas(project_root: &Path) -> Result<bool> {
    let el = super::el_dir(project_root);
    if !el.is_dir() {
        return Ok(false);
    }
    let dir = el.join(".zdbt");
    let mut wrote = false;
    for (name, schema) in [
        ("el-pipeline.schema.json", el_engine::spec::pipeline_json_schema()),
        (
            "el-connections.schema.json",
            el_engine::spec::connections_json_schema(),
        ),
    ] {
        let path = dir.join(name);
        let missing = std::fs::metadata(&path).map(|meta| meta.len() == 0).unwrap_or(true);
        if missing {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
            std::fs::write(&path, serde_json::to_string_pretty(&schema)?)
                .with_context(|| format!("writing {}", path.display()))?;
            wrote = true;
        }
    }
    Ok(wrote)
}

/// Adds the two schema associations to the project's `.zed/settings.json`,
/// preserving everything already there.
fn merge_yaml_schema_settings(project_root: &Path) -> Result<()> {
    let dir = project_root.join(".zed");
    std::fs::create_dir_all(&dir).context("creating .zed")?;
    let path = dir.join("settings.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };

    let schemas = root
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("lsp")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .context("lsp is not an object")?
        .entry("yaml-language-server")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .context("yaml-language-server is not an object")?
        .entry("settings")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .context("settings is not an object")?
        .entry("yaml")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .context("yaml is not an object")?
        .entry("schemas")
        .or_insert(serde_json::json!({}));

    if let Some(map) = schemas.as_object_mut() {
        map.entry("./el/.zdbt/el-pipeline.schema.json".to_owned())
            .or_insert(serde_json::json!(["el/pipelines/*.yml"]));
        map.entry("./el/.zdbt/el-connections.schema.json".to_owned())
            .or_insert(serde_json::json!(["el/connections.yml"]));
    }

    std::fs::write(&path, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_once_and_preserves_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".zed")).unwrap();
        std::fs::write(
            dir.path().join(".zed/settings.json"),
            r#"{ "dbt": { "target": "dev" } }"#,
        )
        .unwrap();

        let created = initialize_el_workspace(dir.path()).unwrap();
        assert_eq!(created.len(), 5);

        // Idempotent: nothing new on the second run.
        let again = initialize_el_workspace(dir.path()).unwrap();
        assert!(again.is_empty());

        // The scaffolded pipeline parses and validates against connections.
        let pipeline = el_engine::spec::load_pipeline(
            &dir.path().join("el/pipelines/example.yml"),
        )
        .unwrap();
        let connections =
            el_engine::spec::load_connections(&dir.path().join("el/connections.yml")).unwrap();
        let issues: Vec<_> = el_engine::spec::validate(&pipeline, &connections)
            .into_iter()
            // Env vars are legitimately unset in tests.
            .filter(|issue| !issue.message.contains("references ${"))
            .collect();
        assert!(issues.is_empty(), "{issues:?}");

        // Settings merged, existing keys preserved.
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(".zed/settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings["dbt"]["target"], "dev");
        assert!(
            settings["lsp"]["yaml-language-server"]["settings"]["yaml"]["schemas"]
                ["./el/.zdbt/el-pipeline.schema.json"]
                .is_array()
        );

        // And the preview actually works on the scaffolded sample.
        let result = el_engine::preview_stream(
            dir.path(),
            &pipeline,
            "sample_file",
            50,
            None,
            &el_engine::CancelFlag::default(),
        )
        .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.failures.len(), 1, "badvalue must be reported");
    }
}
