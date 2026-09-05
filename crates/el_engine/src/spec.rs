//! The EL-as-code spec: `el/connections.yml` and `el/pipelines/<name>.yml`.
//!
//! Field declaration order here IS the canonical YAML order — the writer
//! re-serializes whole files, and one-field edits must produce one-line
//! diffs. Unknown keys survive round-trips via `flatten`ed catch-alls, so
//! a hand-added key is never silently deleted; hand-written comments are
//! not preserved (documented v1 limitation, warned on write).

use std::path::Path;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::SnowflakeType;

pub const MANAGED_HEADER: &str =
    "# Managed by zdbt — comments outside this header are not preserved.";

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {message}")]
    Parse { path: String, message: String },
    #[error("{path} uses YAML anchors/aliases, which zdbt does not round-trip")]
    UnsupportedYamlFeature { path: String },
}

/// A problem `validate` found; `stream` is None for pipeline-level issues.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecIssue {
    pub stream: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteWarning {
    /// The existing file contains comments outside the managed header;
    /// a canvas write will drop them.
    CommentsWillBeDropped,
}

// ---------------------------------------------------------------------------
// connections.yml

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Connections {
    pub version: u32,
    pub connections: IndexMap<String, Connection>,
}

/// One named connection. Every string value may contain `${VAR}`
/// placeholders resolved from the environment at run time — never store a
/// real credential in this file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Connection {
    Postgres(DbConn),
    Mysql(DbConn),
    Mssql(MssqlConn),
    Snowflake(SnowflakeConn),
    Duckdb(DuckdbConn),
    S3(ObjectStoreConn),
    Gcs(ObjectStoreConn),
    Azure(ObjectStoreConn),
    Local {},
}

impl Connection {
    pub fn kind(&self) -> &'static str {
        match self {
            Connection::Postgres(_) => "postgres",
            Connection::Mysql(_) => "mysql",
            Connection::Mssql(_) => "mssql",
            Connection::Snowflake(_) => "snowflake",
            Connection::Duckdb(_) => "duckdb",
            Connection::S3(_) => "s3",
            Connection::Gcs(_) => "gcs",
            Connection::Azure(_) => "azure",
            Connection::Local {} => "local",
        }
    }

    /// Every templated `${VAR}` reference in this connection's values —
    /// names only, for validation and the UI. Values never leave here.
    pub fn env_refs(&self) -> Vec<String> {
        let mut refs = Vec::new();
        let yaml = serde_yaml_ng::to_string(self).unwrap_or_default();
        crate::env::collect_var_refs(&yaml, &mut refs);
        refs.sort();
        refs.dedup();
        refs
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DbConn {
    /// Full connection URL, e.g. `postgres://user:pass@host:5432/db` —
    /// normally `${SOME_URL}`.
    pub url: String,
}

/// A DuckDB database file — the zero-credential source for local testing.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DuckdbConn {
    /// Project-relative or absolute path to the .duckdb file; may be
    /// `${VAR}`-templated.
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MssqlConn {
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SnowflakeConn {
    pub account: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warehouse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub auth: SnowflakeAuth,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum SnowflakeAuth {
    KeyPair { private_key_path: String },
    Password { password: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObjectStoreConn {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

// ---------------------------------------------------------------------------
// pipelines/<name>.yml

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Pipeline {
    pub version: u32,
    pub pipeline: String,
    /// Connection name in connections.yml.
    pub source: String,
    pub target: TargetSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<StreamDefaults>,
    pub streams: Vec<StreamSpec>,
    /// Node positions on the pipeline canvas. UI-owned; the engine only
    /// round-trips it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<CanvasMeta>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TargetSpec {
    /// Connection name; must be a snowflake connection.
    pub connection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub schema: String,
    /// Table-name template; `{stream}` expands to the upper-cased stream
    /// name. Per-stream `target_table` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StreamDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    FullRefresh,
    Incremental,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StreamSpec {
    /// Stream identity — also the incremental cursor's identity; renaming
    /// a stream resets its saved watermark.
    pub name: String,
    pub source: SourceObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_key: Vec<String>,
    /// The incremental cursor column (Airbyte's "cursor field").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<Select>,
    /// Per-column overrides; unlisted columns pass through with inferred
    /// types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnSpec>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

impl StreamSpec {
    pub fn mode(&self, defaults: Option<&StreamDefaults>) -> Mode {
        self.mode
            .or_else(|| defaults.and_then(|defaults| defaults.mode))
            .unwrap_or_default()
    }

    /// The resolved target table name for this stream.
    pub fn target_table(&self, target: &TargetSpec) -> String {
        if let Some(table) = &self.target_table {
            return table.clone();
        }
        let template = target.table.as_deref().unwrap_or("{stream}");
        template.replace("{stream}", &self.name.to_uppercase())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SourceObject {
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        table: String,
    },
    Path {
        /// Local path (project-relative or absolute) or a cloud URL
        /// (`s3://…`, `gs://…`, `az://…`). Globs allowed.
        path: String,
        format: FileFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        csv: Option<CsvOptions>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Csv,
    Parquet,
    Ndjson,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CsvOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<char>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Select {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ColumnSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast: Option<SnowflakeType>,
    /// Strict fails the stream on the first uncastable value; the default
    /// (lax) turns failures into NULLs, counted and reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// chrono format for string→temporal parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse: Option<String>,
    /// Target column name (applies after cast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CanvasMeta {
    pub nodes: IndexMap<String, NodePos>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NodePos {
    pub x: f32,
    pub y: f32,
}

// ---------------------------------------------------------------------------
// load / write / validate

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, SpecError> {
    let text = std::fs::read_to_string(path).map_err(|source| SpecError::Io {
        path: path.display().to_string(),
        source,
    })?;
    // serde_yaml_ng resolves aliases silently; reject them up front so a
    // canvas write can't destroy structure the user relies on.
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        if line.contains(" &") || line.starts_with('&') || line.contains(" *") && line.contains(": *")
        {
            return Err(SpecError::UnsupportedYamlFeature {
                path: path.display().to_string(),
            });
        }
    }
    serde_yaml_ng::from_str(&text).map_err(|error| SpecError::Parse {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

pub fn load_connections(path: &Path) -> Result<Connections, SpecError> {
    read_yaml(path)
}

pub fn load_pipeline(path: &Path) -> Result<Pipeline, SpecError> {
    read_yaml(path)
}

/// Every pipeline file in `<el_dir>/pipelines`, sorted.
pub fn list_pipelines(el_dir: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(el_dir.join("pipelines"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            )
        })
        .collect();
    paths.sort();
    paths
}

/// Serializes connections in canonical form. Same comment-loss caveat as
/// pipelines; the builder warns once.
pub fn to_canonical_connections_yaml(connections: &Connections) -> String {
    let body = serde_yaml_ng::to_string(connections).unwrap_or_default();
    format!(
        "# yaml-language-server: $schema=./.zdbt/el-connections.schema.json\n{MANAGED_HEADER}\n{body}"
    )
}

/// Serializes the pipeline in canonical form with the managed header and a
/// schema pointer for yaml-language-server.
pub fn to_canonical_yaml(pipeline: &Pipeline) -> String {
    let body = serde_yaml_ng::to_string(pipeline).unwrap_or_default();
    format!(
        "# yaml-language-server: $schema=../.zdbt/el-pipeline.schema.json\n{MANAGED_HEADER}\n{body}"
    )
}

pub fn write_pipeline(
    pipeline: &Pipeline,
    path: &Path,
) -> Result<Vec<WriteWarning>, SpecError> {
    let mut warnings = Vec::new();
    if let Ok(existing) = std::fs::read_to_string(path) {
        let has_foreign_comments = existing
            .lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .any(|line| {
                !line.contains("yaml-language-server") && !line.contains("Managed by zdbt")
            });
        if has_foreign_comments {
            warnings.push(WriteWarning::CommentsWillBeDropped);
        }
    }
    std::fs::write(path, to_canonical_yaml(pipeline)).map_err(|source| SpecError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(warnings)
}

/// Cross-file validation: pipeline against connections. Never reads a
/// credential value — messages name variables and columns only.
pub fn validate(pipeline: &Pipeline, connections: &Connections) -> Vec<SpecIssue> {
    let mut issues = Vec::new();
    let mut issue = |stream: Option<&str>, message: String| {
        issues.push(SpecIssue {
            stream: stream.map(str::to_owned),
            message,
        })
    };

    let source_conn = connections.connections.get(&pipeline.source);
    if source_conn.is_none() {
        issue(
            None,
            format!(
                "source connection {:?} is not defined in connections.yml",
                pipeline.source
            ),
        );
    }
    match connections.connections.get(&pipeline.target.connection) {
        None => issue(
            None,
            format!(
                "target connection {:?} is not defined in connections.yml",
                pipeline.target.connection
            ),
        ),
        Some(conn) if conn.kind() != "snowflake" => issue(
            None,
            format!(
                "target connection {:?} is {} — the target must be snowflake",
                pipeline.target.connection,
                conn.kind()
            ),
        ),
        Some(_) => {}
    }

    let mut seen = std::collections::HashSet::new();
    for stream in &pipeline.streams {
        if !seen.insert(stream.name.clone()) {
            issue(
                Some(&stream.name),
                format!("duplicate stream name {:?}", stream.name),
            );
        }
        let mode = stream.mode(pipeline.defaults.as_ref());
        if mode == Mode::Incremental {
            if stream.primary_key.is_empty() {
                issue(
                    Some(&stream.name),
                    "incremental mode requires primary_key".to_owned(),
                );
            }
            if stream.update_key.is_none() {
                issue(
                    Some(&stream.name),
                    "incremental mode requires update_key (the cursor column)".to_owned(),
                );
            }
        }
        if let Some(select) = &stream.select {
            if !select.include.is_empty() && !select.exclude.is_empty() {
                issue(
                    Some(&stream.name),
                    "select.include and select.exclude are mutually exclusive".to_owned(),
                );
            }
            for column in &stream.columns {
                if select.exclude.iter().any(|ex| ex == &column.name)
                    || (!select.include.is_empty()
                        && !select.include.iter().any(|inc| inc == &column.name))
                {
                    issue(
                        Some(&stream.name),
                        format!("column rule for {:?} targets an unselected column", column.name),
                    );
                }
            }
        }
        for column in &stream.columns {
            if column.parse.is_some() {
                let temporal = matches!(
                    column.cast.as_ref().map(|c| c.base),
                    Some(
                        crate::types::SfBase::Date
                            | crate::types::SfBase::Time
                            | crate::types::SfBase::TimestampNtz
                            | crate::types::SfBase::TimestampTz
                    )
                );
                if !temporal {
                    issue(
                        Some(&stream.name),
                        format!("column {:?} has parse: but no temporal cast", column.name),
                    );
                }
            }
        }
        // Stream shape must match the source connection's kind.
        if let Some(conn) = source_conn {
            let db_kind = matches!(conn.kind(), "duckdb" | "postgres" | "mysql" | "mssql");
            match &stream.source {
                SourceObject::Table { .. } if !db_kind => issue(
                    Some(&stream.name),
                    format!(
                        "table sources need a database connection, but {:?} is {}",
                        pipeline.source,
                        conn.kind()
                    ),
                ),
                SourceObject::Path { .. } if db_kind => issue(
                    Some(&stream.name),
                    format!(
                        "file sources can't use database connection {:?} — use a local/object-store connection",
                        pipeline.source
                    ),
                ),
                _ => {}
            }
        }
        // File streams can't be incremental in v1 (no cursor pushdown).
        if matches!(stream.source, SourceObject::Path { .. }) && mode == Mode::Incremental {
            issue(
                Some(&stream.name),
                "file sources do not support incremental mode".to_owned(),
            );
        }
    }

    // Env references must resolve (names only in the message).
    if let Some(conn) = source_conn {
        for var in conn.env_refs() {
            if std::env::var_os(&var).is_none() {
                issue(
                    None,
                    format!(
                        "connection {:?} references ${{{var}}} which is not set \
                         (checked real env only; .env files load at run time)",
                        pipeline.source
                    ),
                );
            }
        }
    }
    issues
}

/// JSON Schemas for yaml-language-server completion.
pub fn pipeline_json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Pipeline)).unwrap_or_default()
}

pub fn connections_json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Connections)).unwrap_or_default()
}
