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
    /// Base connections — used as-is when no profile is active, and as
    /// the fallback for names a profile does not override.
    pub connections: IndexMap<String, Connection>,
    /// Environment profiles (dev / recette / prod …): each maps logical
    /// connection names to that environment's real connections. Pipelines
    /// never change across profiles — only what the names point to.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub profiles: IndexMap<String, ProfileSpec>,
    /// The profile used when neither ZDBT_EL_PROFILE nor the local
    /// selection picks one. Committed — the team's declared default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSpec {
    pub connections: IndexMap<String, Connection>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

impl Connections {
    /// The connection set for `profile`: base entries overridden (and
    /// extended) by the profile's. None = base only.
    pub fn resolved(&self, profile: Option<&str>) -> Result<IndexMap<String, Connection>, String> {
        let Some(profile) = profile else {
            return Ok(self.connections.clone());
        };
        let Some(spec) = self.profiles.get(profile) else {
            return Err(format!(
                "profile {profile:?} is not defined in connections.yml (has: {})",
                self.profiles
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        let mut resolved = self.connections.clone();
        for (name, connection) in &spec.connections {
            resolved.insert(name.clone(), connection.clone());
        }
        Ok(resolved)
    }
}

/// Where the local profile selection lives — per-checkout state, never
/// committed (the IDE writes it on switch).
pub fn profile_selection_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join("el").join(".zdbt").join("profile")
}

/// The active profile name: ZDBT_EL_PROFILE env wins, then the local
/// selection file, then the file's default_profile. None = base only.
pub fn active_profile(project_root: &Path, connections: &Connections) -> Option<String> {
    try_active_profile(project_root, connections).unwrap_or(None)
}

/// Like [`active_profile`], but an UNREADABLE selection file (present yet
/// failing to read) is an error — never a silent flip to the default
/// environment.
pub fn try_active_profile(
    project_root: &Path,
    connections: &Connections,
) -> Result<Option<String>, String> {
    if let Ok(name) = std::env::var("ZDBT_EL_PROFILE") {
        let name = name.trim().to_owned();
        if !name.is_empty() {
            return Ok(Some(name));
        }
    }
    match std::fs::read_to_string(profile_selection_path(project_root)) {
        Ok(name) => {
            let name = name.trim().to_owned();
            if !name.is_empty() {
                return Ok(Some(name));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "the profile selection file could not be read ({error}) — fix or                  delete el/.zdbt/profile"
            ));
        }
    }
    Ok(connections.default_profile.clone())
}

/// One-stop loader for run surfaces: connections.yml resolved through
/// the active profile, flattened into a plain `Connections`, plus the
/// profile name for display. An unknown active profile is an error —
/// never a silent fall-through to another environment's credentials.
/// connections.yml resolved through an EXPLICIT profile (deploy-pinned
/// runs); unknown profiles are hard errors.
pub fn load_connections_for_profile(
    project_root: &Path,
    profile: Option<&str>,
) -> Result<Connections, SpecError> {
    let path = project_root.join("el").join("connections.yml");
    let raw = load_connections(&path)?;
    let resolved = raw
        .resolved(profile)
        .map_err(|message| SpecError::Parse {
            path: path.display().to_string(),
            message,
        })?;
    Ok(Connections {
        version: raw.version,
        connections: resolved,
        profiles: IndexMap::new(),
        default_profile: None,
        extra: IndexMap::new(),
    })
}

pub fn load_active_connections(
    project_root: &Path,
) -> Result<(Connections, Option<String>), SpecError> {
    let path = project_root.join("el").join("connections.yml");
    let raw = load_connections(&path)?;
    let profile =
        try_active_profile(project_root, &raw).map_err(|message| SpecError::Parse {
            path: path.display().to_string(),
            message,
        })?;
    let resolved = raw
        .resolved(profile.as_deref())
        .map_err(|message| SpecError::Parse {
            path: path.display().to_string(),
            message,
        })?;
    Ok((
        Connections {
            version: raw.version,
            connections: resolved,
            profiles: IndexMap::new(),
            default_profile: None,
            extra: IndexMap::new(),
        },
        profile,
    ))
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
    Local {
        #[serde(flatten)]
        #[schemars(skip)]
        extra: IndexMap<String, serde_yaml_ng::Value>,
    },
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
            Connection::Local { .. } => "local",
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

    /// The `${VAR}` references this connection needs that `env` does not
    /// provide — names only, so the UI can say "set X in .env" without
    /// ever touching a value.
    pub fn missing_env_refs(&self, env: &crate::env::EnvMap) -> Vec<String> {
        self.env_refs()
            .into_iter()
            .filter(|var| !env.contains(var))
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DbConn {
    /// Full connection URL, e.g. `postgres://user:pass@host:5432/db` —
    /// normally `${SOME_URL}`.
    pub url: String,
    /// Unknown keys survive canonical rewrites — hand additions are
    /// never silently dropped.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

/// A DuckDB database file — the zero-credential source for local testing.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DuckdbConn {
    /// Project-relative or absolute path to the .duckdb file; may be
    /// `${VAR}`-templated.
    pub path: String,
    /// Unknown keys survive canonical rewrites — hand additions are
    /// never silently dropped.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
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
    /// Unknown keys survive canonical rewrites — hand additions are
    /// never silently dropped.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
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
    /// Unknown keys survive canonical rewrites — hand additions are
    /// never silently dropped.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
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
    /// Unknown keys survive canonical rewrites — hand additions are
    /// never silently dropped.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
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
    /// Cron expression executed by `el serve` (6/7-field, seconds first,
    /// e.g. "0 0 2 * * *" = daily 02:00). Absent = manual runs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// IANA timezone the schedule fires in; defaults to UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetrySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<OnFailure>,
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

/// Automatic re-runs after a failed run: up to `attempts` retries, each
/// `backoff` apart ("30s", "5m", "1h").
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RetrySpec {
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<String>,
}

/// What a failed run triggers, after retries are exhausted. The webhook
/// URL may be `${VAR}`-templated — never a secret in the spec.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct OnFailure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    /// Shell command run with ZDBT_EL_PIPELINE / ZDBT_EL_ERROR set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Parses "30s" / "5m" / "1h" (bare numbers = seconds).
pub fn parse_backoff(text: &str) -> Option<std::time::Duration> {
    let text = text.trim();
    let (digits, unit) = match text.find(|c: char| !c.is_ascii_digit()) {
        Some(ix) => text.split_at(ix),
        None => (text, "s"),
    };
    let value: u64 = digits.parse().ok()?;
    let seconds = match unit.trim() {
        "s" | "" => value,
        "m" => value * 60,
        "h" => value * 3600,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(seconds))
}

// ---------------------------------------------------------------------------
// el/remotes.yml — declared `el serve` daemons the IDE can drive.

/// One remote daemon. Non-loopback URLs must be https — the bearer token
/// never travels plaintext across a network.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSpec {
    /// e.g. `https://el.example.com:7431` (http allowed for loopback only).
    pub url: String,
    /// Bearer token, normally `${ZDBT_EL_TOKEN}` — never a literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Remotes {
    pub version: u32,
    pub remotes: IndexMap<String, RemoteSpec>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

pub fn load_remotes(path: &Path) -> Result<Remotes, SpecError> {
    read_yaml(path)
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

/// The `canvas.nodes` key prefixes the canvas writes per stream (see
/// `NodeId::spec_key` in the UI layout): one row = stream, map, target.
const CANVAS_NODE_PREFIXES: [&str; 3] = ["stream:", "map:", "target:"];

impl Pipeline {
    /// Renames a stream and re-keys its canvas positions, keeping their
    /// order. The name is also the incremental cursor's identity, so the
    /// saved watermark stays under the old name and the next run
    /// re-extracts; a derived target table (`{stream}` template) follows
    /// the new name — `target_table` is left alone either way.
    pub fn rename_stream(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("stream name can't be empty".to_owned());
        }
        if to != from && self.streams.iter().any(|stream| stream.name == to) {
            return Err(format!("{to} already exists in this pipeline"));
        }
        // The new name may derive a table another stream already loads,
        // which would make the two overwrite each other on every run.
        if to != from {
            if let Some(renamed) = self.streams.iter().find(|stream| stream.name == from) {
                let mut probe = renamed.clone();
                probe.name = to.to_owned();
                let table = probe.target_table(&self.target).to_uppercase();
                if let Some(other) = self.streams.iter().find(|stream| {
                    stream.name != from && stream.target_table(&self.target).to_uppercase() == table
                }) {
                    return Err(format!(
                        "{to} would load the same table as {} — pick another name",
                        other.name
                    ));
                }
            }
        }
        let Some(stream) = self.streams.iter_mut().find(|stream| stream.name == from) else {
            return Err(format!("stream {from} is gone from the spec"));
        };
        stream.name = to.to_owned();
        if let Some(canvas) = &mut self.canvas {
            let nodes = std::mem::take(&mut canvas.nodes);
            canvas.nodes = nodes
                .into_iter()
                .map(|(key, pos)| {
                    let rekeyed = CANVAS_NODE_PREFIXES
                        .iter()
                        .find(|prefix| key == format!("{prefix}{from}"))
                        .map(|prefix| format!("{prefix}{to}"));
                    (rekeyed.unwrap_or(key), pos)
                })
                .collect();
        }
        Ok(())
    }

    /// Drops a stream and its canvas positions; an emptied canvas block
    /// goes away rather than serializing as `nodes: {}`. Returns false
    /// when no stream has that name.
    pub fn remove_stream(&mut self, name: &str) -> bool {
        let before = self.streams.len();
        self.streams.retain(|stream| stream.name != name);
        if self.streams.len() == before {
            return false;
        }
        if let Some(canvas) = &mut self.canvas {
            for prefix in CANVAS_NODE_PREFIXES {
                canvas.nodes.shift_remove(&format!("{prefix}{name}"));
            }
            if canvas.nodes.is_empty() {
                self.canvas = None;
            }
        }
        true
    }

    /// Swaps a stream with its neighbour (`up` = towards the top of the
    /// list). Streams are laid out in list order, so this reorders the
    /// canvas too; pinned positions swap only when BOTH neighbours are
    /// pinned, otherwise a lone pin keeps its spot. Returns false at the
    /// boundary or when no stream has that name.
    pub fn move_stream(&mut self, name: &str, up: bool) -> bool {
        let Some(ix) = self.streams.iter().position(|stream| stream.name == name) else {
            return false;
        };
        let other = if up {
            ix.checked_sub(1)
        } else {
            (ix + 1 < self.streams.len()).then_some(ix + 1)
        };
        let Some(other) = other else { return false };
        let other_name = self.streams[other].name.clone();
        self.streams.swap(ix, other);
        if let Some(canvas) = &mut self.canvas {
            for prefix in CANVAS_NODE_PREFIXES {
                let mine = format!("{prefix}{name}");
                let theirs = format!("{prefix}{other_name}");
                if let (Some(a), Some(b)) = (
                    canvas.nodes.get(&mine).copied(),
                    canvas.nodes.get(&theirs).copied(),
                ) {
                    // insert on an existing key keeps its position.
                    canvas.nodes.insert(mine, b);
                    canvas.nodes.insert(theirs, a);
                }
            }
        }
        true
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
pub fn to_canonical_remotes_yaml(remotes: &Remotes) -> String {
    let body = serde_yaml_ng::to_string(remotes).unwrap_or_default();
    format!(
        "# el/remotes.yml — el serve daemons the IDE can drive from the Remote tab.\n\
         # Tokens are ${{VAR}} references, never literals. Non-loopback URLs must be https.\n\
         {MANAGED_HEADER}\n{body}"
    )
}

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
        Some(conn) if !matches!(conn.kind(), "snowflake" | "duckdb") => issue(
            None,
            format!(
                "target connection {:?} is {} — targets must be snowflake or duckdb",
                pipeline.target.connection,
                conn.kind()
            ),
        ),
        Some(_) => {}
    }

    let mut seen = std::collections::HashSet::new();
    let mut seen_tables: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for stream in &pipeline.streams {
        let fresh = seen.insert(stream.name.clone());
        if !fresh {
            issue(
                Some(&stream.name),
                format!("duplicate stream name {:?}", stream.name),
            );
        }
        // Two streams resolving to one table overwrite each other on every
        // run — the load replaces the whole table. Warehouse names are
        // case-insensitive, so compare them that way.
        if fresh {
            let table = stream.target_table(&pipeline.target);
            match seen_tables.entry(table.to_uppercase()) {
                std::collections::hash_map::Entry::Occupied(taken) => issue(
                    Some(&stream.name),
                    format!(
                        "streams {:?} and {:?} both load table {table} — \
                         set target_table on one of them",
                        taken.get(),
                        stream.name
                    ),
                ),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(stream.name.clone());
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::EnvMap;

    #[test]
    fn missing_env_refs_names_unset_vars_only() {
        let postgres = Connection::Postgres(DbConn {
            url: "${ZDBT_TEST_UNSET_ABC}".to_owned(),
            extra: IndexMap::new(),
        });
        assert_eq!(
            postgres.missing_env_refs(&EnvMap::empty()),
            vec!["ZDBT_TEST_UNSET_ABC".to_owned()]
        );

        let duckdb = Connection::Duckdb(DuckdbConn {
            path: "el/demo.duckdb".to_owned(),
            extra: IndexMap::new(),
        });
        assert!(duckdb.missing_env_refs(&EnvMap::empty()).is_empty());
    }
}
