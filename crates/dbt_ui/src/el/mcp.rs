//! MCP server exposing zdbt's extract-load tools to agents.
//!
//! Runs as `zdbt --el-mcp` over the same stdio loop as the dbt server. The
//! tools read the YAML under `el/` that the canvas edits, and reach
//! databases only through the connector worker or a connection's declared
//! remote — the same paths the EL panel uses — so an agent sees what the
//! developer sees.
//!
//! Two lines it does not cross. Credentials never appear: a connection
//! reports its kind and the NAMES of the variables it references, never a
//! URL or a value. And nothing here runs or deploys a pipeline: those stay
//! explicit developer actions.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};

use el_engine::spec::{Connections, Pipeline, SourceObject};

const LIST_TABLES_DEFAULT: usize = 200;
const LIST_TABLES_MAX: usize = 2_000;
const QUERY_ROWS_DEFAULT: usize = 50;
const QUERY_ROWS_MAX: usize = 500;
const PREVIEW_ROWS_DEFAULT: usize = 20;
const PREVIEW_ROWS_MAX: usize = 200;
const RECENT_RUNS: usize = 20;

pub fn serve() {
    crate::mcp::serve_stdio("ellode", tool_descriptors(), call_tool);
}

fn tool_descriptors() -> Value {
    let root = json!({
        "type": "string",
        "description": "Absolute path of the project directory that contains el/",
    });
    let connection = json!({
        "type": "string",
        "description": "Connection name from el/connections.yml (the active profile applies)",
    });
    let pipeline = json!({
        "type": "string",
        "description": "Pipeline name (its `pipeline:` key) or its file name without .yml",
    });
    json!([
        {
            "name": "el_list_pipelines",
            "description": "List the extract-load pipelines under el/pipelines: source and target connections, stream count, schedule, and how many validation issues each has. Files that fail to parse are reported with the reason.",
            "inputSchema": { "type": "object", "properties": { "project_root": root }, "required": ["project_root"] },
        },
        {
            "name": "el_pipeline_info",
            "description": "One pipeline in detail: source and target (with connection kinds), schedule, and every stream — its source table or file, load mode, primary and update keys, and the target table it lands in — plus the validation issues against the active connections.",
            "inputSchema": { "type": "object", "properties": { "project_root": root, "pipeline": pipeline }, "required": ["project_root", "pipeline"] },
        },
        {
            "name": "el_validate",
            "description": "Validate pipelines against the active connections and report every problem, including files that don't parse and connections with shape problems. Pass `pipeline` to check one; omit it to check them all. Use after editing YAML under el/.",
            "inputSchema": { "type": "object", "properties": { "project_root": root, "pipeline": pipeline }, "required": ["project_root"] },
        },
        {
            "name": "el_list_connections",
            "description": "List the connections of the active profile: kind, where its worker runs (`local` or a remote), whether its tables can be browsed, the names of the environment variables it references and which of them are unset, and shape problems. Never returns URLs, passwords, or any other value.",
            "inputSchema": { "type": "object", "properties": { "project_root": root }, "required": ["project_root"] },
        },
        {
            "name": "el_list_tables",
            "description": "List the tables and views of a database connection (duckdb, postgres, oracle, snowflake) as SCHEMA.TABLE — DATABASE.SCHEMA.TABLE on Snowflake — through its worker. Large catalogs page: pass `search` (every word must appear, any case), `offset`, and `limit`; the answer says how many match and the next offset.",
            "inputSchema": { "type": "object", "properties": {
                "project_root": root,
                "connection": connection,
                "search": { "type": "string", "description": "Words that must all appear in SCHEMA.TABLE" },
                "offset": { "type": "integer", "description": "Skip this many matches (default 0)" },
                "limit": { "type": "integer", "description": "At most this many tables (default 200, max 2000)" },
            }, "required": ["project_root", "connection"] },
        },
        {
            "name": "el_table_columns",
            "description": "The columns of one table on a database connection, from the database's own catalog: name, data type, nullability (and length, precision, scale on Oracle). On Snowflake pass the schema as DATABASE.SCHEMA, as el_list_tables shows it; reading its catalog needs a warehouse.",
            "inputSchema": { "type": "object", "properties": {
                "project_root": root,
                "connection": connection,
                "schema": { "type": "string" },
                "table": { "type": "string" },
            }, "required": ["project_root", "connection", "schema", "table"] },
        },
        {
            "name": "el_query",
            "description": "Run one read-only SQL query (SELECT or WITH) on a database connection through its worker and return the rows as JSON. Executes against the source database; use the database's own dialect.",
            "inputSchema": { "type": "object", "properties": {
                "project_root": root,
                "connection": connection,
                "sql": { "type": "string" },
                "limit": { "type": "integer", "description": "Row cap (default 50, max 500)" },
            }, "required": ["project_root", "connection", "sql"] },
        },
        {
            "name": "el_preview_stream",
            "description": "Preview what a pipeline stream would load: sample rows after its column mapping and casts, each column's source and target type, and the values that fail their cast. Reads from the source; writes nothing.",
            "inputSchema": { "type": "object", "properties": {
                "project_root": root,
                "pipeline": pipeline,
                "stream": { "type": "string" },
                "limit": { "type": "integer", "description": "Row cap (default 20, max 200)" },
            }, "required": ["project_root", "pipeline", "stream"] },
        },
        {
            "name": "el_list_remotes",
            "description": "List the EL servers declared in el/remotes.yml: name, host, the name of the token variable, and whether that variable is set. Never returns the token.",
            "inputSchema": { "type": "object", "properties": { "project_root": root }, "required": ["project_root"] },
        },
        {
            "name": "el_remote_status",
            "description": "Ask one EL server what it runs: its profile, the pipelines deployed there with their schedules, and its most recent runs with status, rows written, cast failures, and errors. Read-only.",
            "inputSchema": { "type": "object", "properties": {
                "project_root": root,
                "remote": { "type": "string", "description": "Server name from el/remotes.yml" },
            }, "required": ["project_root", "remote"] },
        },
    ])
}

fn call_tool(name: &str, arguments: &Value) -> Result<String> {
    let root = project_root(arguments)?;
    let answer = match name {
        "el_list_pipelines" => list_pipelines(&root)?,
        "el_pipeline_info" => pipeline_info(&root, required(arguments, "pipeline")?)?,
        "el_validate" => validate(&root, optional(arguments, "pipeline"))?,
        "el_list_connections" => list_connections(&root)?,
        "el_list_tables" => list_tables(
            &root,
            required(arguments, "connection")?,
            optional(arguments, "search").unwrap_or(""),
            count(arguments, "offset", 0, usize::MAX),
            count(arguments, "limit", LIST_TABLES_DEFAULT, LIST_TABLES_MAX),
        )?,
        "el_table_columns" => table_columns(
            &root,
            required(arguments, "connection")?,
            required(arguments, "schema")?,
            required(arguments, "table")?,
        )?,
        "el_query" => query(
            &root,
            required(arguments, "connection")?,
            required(arguments, "sql")?,
            count(arguments, "limit", QUERY_ROWS_DEFAULT, QUERY_ROWS_MAX),
        )?,
        "el_preview_stream" => preview_stream(
            &root,
            required(arguments, "pipeline")?,
            required(arguments, "stream")?,
            count(arguments, "limit", PREVIEW_ROWS_DEFAULT, PREVIEW_ROWS_MAX),
        )?,
        "el_list_remotes" => list_remotes(&root)?,
        "el_remote_status" => remote_status(&root, required(arguments, "remote")?)?,
        other => bail!("unknown tool: {other}"),
    };
    Ok(serde_json::to_string_pretty(&answer)?)
}

fn project_root(arguments: &Value) -> Result<PathBuf> {
    let root = PathBuf::from(required(arguments, "project_root")?);
    anyhow::ensure!(
        super::el_dir(&root).is_dir(),
        "{} has no el/ directory — pass the project that holds the EL specs",
        root.display()
    );
    Ok(root)
}

fn required<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    optional(arguments, key).with_context(|| format!("missing required argument `{key}`"))
}

fn optional<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn count(arguments: &Value, key: &str, default: usize, max: usize) -> usize {
    arguments
        .get(key)
        .and_then(|value| value.as_u64())
        .map_or(default, |value| usize::try_from(value).unwrap_or(max))
        .min(max)
}

fn active_connections(root: &Path) -> Result<(Connections, Option<String>)> {
    el_engine::spec::load_active_connections(root)
        .map_err(|error| anyhow::anyhow!("el/connections.yml could not be read: {error}"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// Finds a pipeline by its `pipeline:` name or its file stem.
fn find_pipeline(root: &Path, name: &str) -> Result<(PathBuf, Pipeline)> {
    let mut unreadable = Vec::new();
    for path in el_engine::spec::list_pipelines(&super::el_dir(root)) {
        let stem_matches = path.file_stem().and_then(|stem| stem.to_str()) == Some(name);
        match el_engine::spec::load_pipeline(&path) {
            Ok(pipeline) if pipeline.pipeline == name || stem_matches => {
                return Ok((path, pipeline));
            }
            Ok(_) => {}
            Err(error) if stem_matches => {
                bail!("{} could not be read: {error}", file_name(&path))
            }
            Err(_) => unreadable.push(file_name(&path)),
        }
    }
    if unreadable.is_empty() {
        bail!("no pipeline named {name:?} under el/pipelines")
    }
    bail!(
        "no pipeline named {name:?} under el/pipelines ({} could not be read — el_validate says why)",
        unreadable.join(", ")
    )
}

fn issues_json(issues: &[el_engine::spec::SpecIssue]) -> Vec<Value> {
    issues
        .iter()
        .map(|issue| json!({ "stream": issue.stream, "message": issue.message }))
        .collect()
}

fn list_pipelines(root: &Path) -> Result<Value> {
    let connections = active_connections(root)
        .ok()
        .map(|(connections, _)| connections);
    let mut pipelines = Vec::new();
    let mut unreadable = Vec::new();
    for path in el_engine::spec::list_pipelines(&super::el_dir(root)) {
        match el_engine::spec::load_pipeline(&path) {
            Ok(pipeline) => {
                let issues = connections
                    .as_ref()
                    .map(|connections| el_engine::spec::validate(&pipeline, connections).len());
                pipelines.push(json!({
                    "name": pipeline.pipeline,
                    "file": format!("el/pipelines/{}", file_name(&path)),
                    "source": pipeline.source,
                    "target": pipeline.target.connection,
                    "streams": pipeline.streams.len(),
                    "schedule": pipeline.schedule,
                    "timezone": pipeline.timezone,
                    "issues": issues,
                }));
            }
            Err(error) => unreadable.push(json!({
                "file": format!("el/pipelines/{}", file_name(&path)),
                "error": error.to_string(),
            })),
        }
    }
    Ok(json!({ "pipelines": pipelines, "unreadable": unreadable }))
}

fn pipeline_info(root: &Path, name: &str) -> Result<Value> {
    let (path, pipeline) = find_pipeline(root, name)?;
    let connections = active_connections(root)?.0;
    let kind_of = |connection: &str| {
        connections
            .connections
            .get(connection)
            .map(|connection| connection.kind())
    };
    let streams: Vec<Value> = pipeline
        .streams
        .iter()
        .map(|stream| {
            let source = match &stream.source {
                SourceObject::Table { schema, table } => {
                    json!({ "schema": schema, "table": table })
                }
                SourceObject::Path { path, format, .. } => {
                    json!({ "path": path, "format": format })
                }
                SourceObject::Rest(rest) => json!({
                    "endpoint": rest.endpoint,
                    "method": rest.method.as_str(),
                    "records": rest.records,
                    "pagination": rest.pagination.kind(),
                }),
            };
            json!({
                "name": stream.name,
                "source": source,
                "mode": stream.mode(pipeline.defaults.as_ref()),
                "primary_key": stream.primary_key,
                "update_key": stream.update_key,
                // An API target's is the endpoint the stream's rows go to.
                "target_table": match kind_of(&pipeline.target.connection) {
                    Some(kind) if el_engine::spec::is_rest_kind(kind) => {
                        stream.target_location(&pipeline.target, kind)
                    }
                    _ => stream.target_table(&pipeline.target),
                },
                "columns_mapped": stream.columns.len(),
            })
        })
        .collect();
    Ok(json!({
        "name": pipeline.pipeline,
        "file": format!("el/pipelines/{}", file_name(&path)),
        "source": { "connection": pipeline.source, "kind": kind_of(&pipeline.source) },
        "target": {
            "connection": pipeline.target.connection,
            "kind": kind_of(&pipeline.target.connection),
            "database": pipeline.target.database,
            "schema": pipeline.target.schema,
            "table": pipeline.target.table,
        },
        "schedule": pipeline.schedule,
        "timezone": pipeline.timezone,
        "streams": streams,
        "issues": issues_json(&el_engine::spec::validate(&pipeline, &connections)),
    }))
}

fn validate(root: &Path, only: Option<&str>) -> Result<Value> {
    let (connections, profile) = active_connections(root)?;
    let mut pipelines = Vec::new();
    match only {
        Some(name) => {
            let (path, pipeline) = find_pipeline(root, name)?;
            let issues = el_engine::spec::validate(&pipeline, &connections);
            pipelines.push(json!({
                "pipeline": pipeline.pipeline,
                "file": format!("el/pipelines/{}", file_name(&path)),
                "valid": issues.is_empty(),
                "issues": issues_json(&issues),
            }));
        }
        None => {
            for path in el_engine::spec::list_pipelines(&super::el_dir(root)) {
                let file = format!("el/pipelines/{}", file_name(&path));
                pipelines.push(match el_engine::spec::load_pipeline(&path) {
                    Ok(pipeline) => {
                        let issues = el_engine::spec::validate(&pipeline, &connections);
                        json!({
                            "pipeline": pipeline.pipeline,
                            "file": file,
                            "valid": issues.is_empty(),
                            "issues": issues_json(&issues),
                        })
                    }
                    Err(error) => json!({
                        "file": file,
                        "valid": false,
                        "issues": [{ "stream": null, "message": error.to_string() }],
                    }),
                });
            }
        }
    }
    let connection_issues: Vec<Value> = connections
        .connections
        .iter()
        .filter_map(|(name, connection)| {
            let issues = connection.shape_issues();
            (!issues.is_empty()).then(|| json!({ "connection": name, "issues": issues }))
        })
        .collect();
    Ok(json!({
        "active_profile": profile,
        "pipelines": pipelines,
        "connections": connection_issues,
    }))
}

fn list_connections(root: &Path) -> Result<Value> {
    let (connections, profile) = active_connections(root)?;
    let profiles: Vec<String> =
        el_engine::spec::load_connections(&super::el_dir(root).join("connections.yml"))
            .map(|raw| raw.profiles.keys().cloned().collect())
            .unwrap_or_default();
    let env = el_engine::env::EnvMap::load(root, None);
    let listed: Vec<Value> = connections
        .connections
        .iter()
        .map(|(name, connection)| {
            json!({
                "name": name,
                "kind": connection.kind(),
                "runs_on": connection.workspace().unwrap_or("local"),
                "browsable": super::browsable(connection.kind()),
                "variables": connection.env_refs(),
                "missing_variables": connection.missing_env_refs(&env),
                "issues": connection.shape_issues(),
            })
        })
        .collect();
    Ok(json!({
        "active_profile": profile,
        "profiles": profiles,
        "connections": listed,
    }))
}

fn browsable_connection(root: &Path, name: &str) -> Result<&'static str> {
    let (connections, _) = active_connections(root)?;
    let kind = connections
        .connections
        .get(name)
        .with_context(|| format!("no connection named {name:?} in the active profile"))?
        .kind();
    anyhow::ensure!(
        super::browsable(kind),
        "{name} is a {kind} connection — only duckdb, postgres, oracle and snowflake can be browsed"
    );
    Ok(kind)
}

fn list_tables(
    root: &Path,
    connection: &str,
    search: &str,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    browsable_connection(root, connection)?;
    let tables = super::list_tables(root, connection)?;
    let terms = super::search_terms(search);
    let matching: Vec<usize> = if terms.is_empty() {
        (0..tables.len()).collect()
    } else {
        super::matching_tables(&super::fold_table_names(&tables), &terms)
    };
    let page: Vec<String> = matching
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|index| tables.get(*index))
        .map(|(schema, table)| format!("{schema}.{table}"))
        .collect();
    let next = offset.saturating_add(page.len());
    Ok(json!({
        "connection": connection,
        "total": tables.len(),
        "matching": matching.len(),
        "offset": offset,
        "tables": page,
        "next_offset": (next < matching.len()).then_some(next),
    }))
}

/// The catalog query for one table's columns, its names quoted as literals.
fn columns_sql(kind: &str, schema: &str, table: &str) -> Result<String> {
    let literal = |text: &str| text.replace('\'', "''");
    let table_literal = literal(table);
    Ok(match kind {
        "duckdb" | "postgres" => format!(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
             WHERE table_schema = '{}' AND table_name = '{table_literal}' \
             ORDER BY ordinal_position",
            literal(schema)
        ),
        "oracle" => format!(
            "SELECT column_name, data_type, data_length, data_precision, data_scale, nullable \
             FROM all_tab_columns WHERE owner = '{}' AND table_name = '{table_literal}' \
             ORDER BY column_id",
            literal(schema)
        ),
        // Snowflake keeps a catalog per database, so the schema arrives as
        // DATABASE.SCHEMA and the database becomes an identifier.
        "snowflake" => {
            let Some((database, schema)) = schema.split_once('.') else {
                bail!("pass a Snowflake schema as DATABASE.SCHEMA, as el_list_tables shows it");
            };
            format!(
                "SELECT column_name, data_type, is_nullable \
                 FROM \"{}\".information_schema.columns \
                 WHERE table_schema = '{}' AND table_name = '{table_literal}' \
                 ORDER BY ordinal_position",
                database.replace('"', "\"\""),
                literal(schema)
            )
        }
        "mssql" => format!(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME = '{table_literal}' \
             ORDER BY ORDINAL_POSITION",
            literal(schema)
        ),
        other => bail!("{other} connections have no catalog to read"),
    })
}

fn rows_json(columns: &[String], rows: &[Vec<String>]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            Value::Object(
                columns
                    .iter()
                    .zip(row)
                    .map(|(column, value)| (column.clone(), Value::String(value.clone())))
                    .collect::<Map<String, Value>>(),
            )
        })
        .collect()
}

fn table_columns(root: &Path, connection: &str, schema: &str, table: &str) -> Result<Value> {
    let kind = browsable_connection(root, connection)?;
    let result = super::run_query(root, connection, &columns_sql(kind, schema, table)?, 2_000)?;
    anyhow::ensure!(
        !result.rows.is_empty(),
        "{schema}.{table} has no columns visible to {connection} — check the name's case \
         (el_list_tables shows it as the database stores it)"
    );
    let columns: Vec<String> = result
        .columns
        .iter()
        .map(|column| column.to_lowercase())
        .collect();
    Ok(json!({
        "connection": connection,
        "table": format!("{schema}.{table}"),
        "columns": rows_json(&columns, &result.rows),
    }))
}

/// One read-only statement, trimmed of comments and a trailing semicolon.
/// The worker also caps a query by wrapping it as a subquery, which no
/// write can survive; this turns such a call into a clear refusal first.
fn read_only_statement(sql: &str) -> Result<&str> {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(comment) = rest.strip_prefix("--") {
            rest = comment.split_once('\n').map_or("", |(_, after)| after);
        } else if let Some(comment) = rest.strip_prefix("/*") {
            rest = comment.split_once("*/").map_or("", |(_, after)| after);
        } else {
            break;
        }
    }
    let statement = rest.trim_end().trim_end_matches(';').trim_end();
    anyhow::ensure!(!statement.is_empty(), "the query is empty");
    let mut quote = None;
    for character in statement.chars() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, ';') => bail!("pass one statement at a time"),
            _ => {}
        }
    }
    let keyword = statement
        .split(|character: char| !character.is_ascii_alphabetic())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    anyhow::ensure!(
        matches!(keyword.as_str(), "select" | "with"),
        "el_query runs read-only SELECT or WITH queries, not {}",
        if keyword.is_empty() {
            "this".to_owned()
        } else {
            keyword.to_uppercase()
        }
    );
    Ok(statement)
}

fn query(root: &Path, connection: &str, sql: &str, limit: usize) -> Result<Value> {
    let statement = read_only_statement(sql)?;
    browsable_connection(root, connection)?;
    let result = super::run_query(root, connection, statement, limit)?;
    Ok(json!({
        "connection": connection,
        "columns": result.columns,
        "rows": rows_json(&result.columns, &result.rows),
        "row_count": result.rows.len(),
        "limit_reached": result.rows.len() >= limit,
    }))
}

fn preview_stream(root: &Path, pipeline: &str, stream: &str, limit: usize) -> Result<Value> {
    let (_, pipeline) = find_pipeline(root, pipeline)?;
    anyhow::ensure!(
        pipeline
            .streams
            .iter()
            .any(|candidate| candidate.name == stream),
        "{} has no stream named {stream:?}",
        pipeline.pipeline
    );
    let result = super::preview_stream(
        root,
        &pipeline,
        stream,
        limit,
        &el_engine::CancelFlag::default(),
    )?;
    let names: Vec<String> = result
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    Ok(json!({
        "pipeline": pipeline.pipeline,
        "stream": stream,
        "columns": result.columns.iter().map(|column| json!({
            "name": column.name,
            "source_type": column.source_dtype,
            "target_type": column.target_type,
        })).collect::<Vec<_>>(),
        "rows": rows_json(&names, &result.rows),
        "cast_failures": result.failures.iter().map(|failure| json!({
            "column": failure.column,
            "count": failure.count,
            "samples": failure.samples,
        })).collect::<Vec<_>>(),
    }))
}

/// `https://user:secret@el.example.com:7431/path` → `el.example.com:7431`.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
}

fn list_remotes(root: &Path) -> Result<Value> {
    let path = super::el_dir(root).join("remotes.yml");
    if !path.is_file() {
        return Ok(json!({ "remotes": [] }));
    }
    let remotes = el_engine::spec::load_remotes(&path)
        .map_err(|error| anyhow::anyhow!("el/remotes.yml could not be read: {error}"))?;
    let env = el_engine::env::EnvMap::load(root, None);
    let listed: Vec<Value> = remotes
        .remotes
        .iter()
        .map(|(name, remote)| {
            let mut variables = Vec::new();
            if let Some(token) = &remote.token {
                el_engine::env::collect_var_refs(token, &mut variables);
            }
            let variable = variables.first();
            json!({
                "name": name,
                "host": host_of(&remote.url),
                "token_variable": variable,
                "token_set": variable.map(|variable| env.contains(variable)),
            })
        })
        .collect();
    Ok(json!({ "remotes": listed }))
}

fn remote_status(root: &Path, remote: &str) -> Result<Value> {
    let client = el_engine::server::RemoteClient::connect(root, remote)?;
    let health = client
        .health()
        .with_context(|| format!("{remote} did not answer"))?;
    let pipelines = client.pipelines()?;
    let mut runs = client.runs()?;
    runs.sort_by_key(|run| std::cmp::Reverse(run.started_unix));
    runs.truncate(RECENT_RUNS);
    Ok(json!({
        "remote": remote,
        "profile": health.get("profile"),
        "profiles": health.get("profiles"),
        "tracks_checkout": health.get("tracks_checkout"),
        "pipelines": pipelines.iter().map(|pipeline| json!({
            "name": pipeline.name,
            "profile": pipeline.profile,
            "schedule": pipeline.schedule,
            "timezone": pipeline.timezone,
            "next_run_unix": pipeline.next_run_unix,
            "deployed_unix": pipeline.deployed_unix,
        })).collect::<Vec<_>>(),
        "recent_runs": runs.iter().map(|run| json!({
            "id": run.id,
            "pipeline": run.pipeline,
            "status": run.status,
            "attempt": run.attempt,
            "started_unix": run.started_unix,
            "finished_unix": run.finished_unix,
            "rows_written": run.rows_written,
            "cast_failures": run.cast_failures,
            "error": run.error,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(connections: &str, pipelines: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let el = dir.path().join("el");
        std::fs::create_dir_all(el.join("pipelines")).expect("el dirs");
        std::fs::write(el.join("connections.yml"), connections).expect("connections");
        for (file, yaml) in pipelines {
            std::fs::write(el.join("pipelines").join(file), yaml).expect("pipeline");
        }
        dir
    }

    const CONNECTIONS: &str = "version: 1
connections:
  pg_src:
    type: postgres
    url: postgres://loader:hunter2@db.internal.example/app
  ora_src:
    type: oracle
    user: ${ORA_USER}
    password: ${ORA_PASSWORD}
    connect: db.internal.example:1521/ORCL
  wh:
    type: duckdb
    path: el/wh.duckdb
";

    const PIPELINE: &str = "version: 1
pipeline: orders_load
source: pg_src
target:
  connection: wh
  schema: raw
streams:
- name: orders
  source:
    schema: public
    table: orders
";

    fn call(root: &Path, tool: &str, extra: Value) -> Result<String> {
        let mut arguments = json!({ "project_root": root.to_string_lossy() });
        if let (Some(arguments), Some(extra)) = (arguments.as_object_mut(), extra.as_object()) {
            arguments.extend(extra.clone());
        }
        call_tool(tool, &arguments)
    }

    #[test]
    fn connections_carry_variable_names_never_values() {
        let dir = project(CONNECTIONS, &[]);
        let text = call(dir.path(), "el_list_connections", json!({})).expect("tool answers");
        assert!(text.contains("ORA_PASSWORD"), "{text}");
        assert!(text.contains("\"kind\": \"oracle\""), "{text}");
        for secret in ["hunter2", "loader", "db.internal.example", "postgres://"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }

    #[test]
    fn pipelines_are_listed_found_by_name_or_file_and_validated() {
        let dir = project(
            CONNECTIONS,
            &[("orders.yml", PIPELINE), ("broken.yml", "pipeline: [")],
        );
        let listed = call(dir.path(), "el_list_pipelines", json!({})).expect("list");
        assert!(listed.contains("orders_load"), "{listed}");
        assert!(listed.contains("broken.yml"), "{listed}");

        for name in ["orders_load", "orders"] {
            let info =
                call(dir.path(), "el_pipeline_info", json!({ "pipeline": name })).expect("info");
            assert!(info.contains("\"target_table\""), "{info}");
            assert!(info.contains("\"kind\": \"postgres\""), "{info}");
        }

        let checked = call(dir.path(), "el_validate", json!({})).expect("validate");
        assert!(checked.contains("broken.yml"), "{checked}");
        assert!(
            call(
                dir.path(),
                "el_pipeline_info",
                json!({ "pipeline": "nope" })
            )
            .is_err()
        );
    }

    #[test]
    fn an_api_pipeline_says_its_endpoints() {
        let dir = project(
            "version: 1\nconnections:\n  crm:\n    type: rest\n    base_url: https://api.example.com/v2\n    \
             auth: { method: bearer, token: '${CRM_TOKEN}' }\n  hooks: { type: rest, base_url: https://hooks.example.com }\n",
            &[(
                "crm_push.yml",
                "version: 1\npipeline: crm_push\nsource: crm\n\
                 target: { connection: hooks, endpoint: '/import/{stream}' }\n\
                 streams:\n- name: orders\n  source: { endpoint: /orders, records: data, pagination: { type: next_link } }\n",
            )],
        );
        let info = call(
            dir.path(),
            "el_pipeline_info",
            json!({ "pipeline": "crm_push" }),
        )
        .expect("info");
        assert!(info.contains("\"endpoint\": \"/orders\""), "{info}");
        assert!(info.contains("\"pagination\": \"next_link\""), "{info}");
        assert!(
            info.contains("\"target_table\": \"/import/orders\""),
            "{info}"
        );
    }

    #[test]
    fn a_project_without_el_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(call(dir.path(), "el_list_pipelines", json!({})).is_err());
    }

    #[test]
    fn only_single_read_only_statements_pass() {
        assert_eq!(read_only_statement("select 1").ok(), Some("select 1"));
        assert_eq!(
            read_only_statement("-- why\n/* how */ WITH x AS (SELECT 1) SELECT * FROM x;\n").ok(),
            Some("WITH x AS (SELECT 1) SELECT * FROM x")
        );
        assert_eq!(
            read_only_statement("select 'a;b' from dual").ok(),
            Some("select 'a;b' from dual")
        );
        for refused in [
            "",
            "  -- only a comment",
            "delete from orders",
            "select 1; drop table orders",
            "insert into t select 1",
        ] {
            assert!(read_only_statement(refused).is_err(), "{refused:?} passed");
        }
    }

    #[test]
    fn catalog_queries_quote_their_literals() {
        let sql = columns_sql("oracle", "O'HARA", "T").expect("oracle");
        assert!(sql.contains("owner = 'O''HARA'"), "{sql}");
        assert!(
            columns_sql("postgres", "public", "orders")
                .expect("postgres")
                .contains("information_schema.columns")
        );
        let snowflake = columns_sql("snowflake", "RAW.LANDING", "ORDERS").expect("snowflake");
        assert!(
            snowflake.contains("\"RAW\".information_schema.columns"),
            "{snowflake}"
        );
        assert!(
            snowflake.contains("table_schema = 'LANDING'"),
            "{snowflake}"
        );
        assert!(columns_sql("snowflake", "LANDING", "ORDERS").is_err());
        assert!(columns_sql("mysql", "a", "b").is_err());
    }

    #[test]
    fn hosts_drop_scheme_credentials_and_path() {
        assert_eq!(
            host_of("https://user:secret@el.example.com:7431/api?x=1"),
            "el.example.com:7431"
        );
        assert_eq!(host_of("http://127.0.0.1:7431"), "127.0.0.1:7431");
        assert_eq!(host_of("el.example.com"), "el.example.com");
    }

    #[test]
    fn every_tool_is_described_once() {
        let tools = tool_descriptors();
        let names: Vec<&str> = tools
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
            .collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len());
        // With a real project and no other arguments, each tool either
        // answers or asks for what it needs — none falls through.
        let dir = project(CONNECTIONS, &[]);
        for name in names {
            if let Err(error) = call(dir.path(), name, json!({})) {
                let message = format!("{error:#}");
                assert!(!message.contains("unknown tool"), "{name} is not routed");
            }
        }
    }
}
