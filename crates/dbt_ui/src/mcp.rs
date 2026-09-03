//! Native MCP (Model Context Protocol) server exposing zdbt's dbt tools.
//!
//! Runs as `zdbt --dbt-mcp` speaking newline-delimited JSON-RPC over stdio —
//! the transport Zed's agent panel (and Claude Code, and any MCP client)
//! expects from a `context_servers` command. Tools reuse the same lineage
//! store, environment discovery, and dbt invocation the results panel uses,
//! so agents see exactly what the IDE sees.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde_json::{Value, json};

use crate::dbt_settings::DbtSettings;
use crate::lineage::LineageStore;
use crate::results_panel::{
    DbtResultsPanel, apply_common_args, parse_show_output,
};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Settings for headless tool runs: PATH/managed binary, in-project
/// discovery for profiles and .env, no auto-install.
fn headless_settings() -> DbtSettings {
    DbtSettings {
        show_limit: 100,
        binary: "dbt".to_owned(),
        auto_install: false,
        fusion_version: "latest".to_owned(),
        distribution: "fusion".to_owned(),
        core_adapter: String::new(),
        target: None,
        profiles_dir: None,
        env: Vec::new(),
        project_dir: None,
        parse_on_load: false,
        env_file: None,
        lineage_depth: 4,
        lineage_tree_depth: 8,
        lineage_max_nodes: 500,
    }
}

fn project_root(arguments: &Value) -> Result<PathBuf> {
    let root = arguments
        .get("project_root")
        .and_then(|value| value.as_str())
        .context("missing required argument `project_root`")?;
    let root = PathBuf::from(root);
    anyhow::ensure!(
        root.join("dbt_project.yml").is_file(),
        "{} does not contain dbt_project.yml",
        root.display()
    );
    Ok(root)
}

fn arg_str<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(|value| value.as_str())
}

pub fn serve() {
    let store = LineageStore::default();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(|method| method.as_str())
            .unwrap_or("");
        // Notifications (no id) need no response.
        let Some(id) = id else { continue };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = handle(&store, method, &params);
        let response = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": format!("{error:#}") },
            }),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{response}");
        let _ = out.flush();
    }
}

fn handle(store: &LineageStore, method: &str, params: &Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "zdbt", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|name| name.as_str())
                .context("tools/call without a name")?;
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let text = call_tool(store, name, &arguments)?;
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }))
        }
        other => anyhow::bail!("unsupported method: {other}"),
    }
}

fn tool_descriptors() -> Value {
    let root_prop = json!({
        "type": "string",
        "description": "Absolute path of the dbt project directory (contains dbt_project.yml)",
    });
    json!([
        {
            "name": "dbt_list_models",
            "description": "List every model, source, seed, and snapshot in the dbt project with its materialization and file path.",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop }, "required": ["project_root"] },
        },
        {
            "name": "dbt_model_info",
            "description": "Details for one model: description, relation, materialization, SQL operations summary (joins/aggregations/filters), and columns with their transformation expressions.",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop, "model": { "type": "string" } }, "required": ["project_root", "model"] },
        },
        {
            "name": "dbt_lineage",
            "description": "Model-level lineage around a model: upstream and downstream nodes with levels and edges.",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop, "model": { "type": "string" }, "depth": { "type": "integer", "description": "levels per direction (default 3)" } }, "required": ["project_root", "model"] },
        },
        {
            "name": "dbt_column_lineage",
            "description": "Trace one column across the lineage: every model on its path with the local column name and the expression that computes it.",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop, "model": { "type": "string" }, "column": { "type": "string" } }, "required": ["project_root", "model", "column"] },
        },
        {
            "name": "dbt_show",
            "description": "Run a data preview: `dbt show` for a model or an inline SQL query (Jinja allowed), returning result rows as JSON. Executes against the warehouse.",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop, "model": { "type": "string" }, "sql": { "type": "string", "description": "inline SQL instead of a model" }, "limit": { "type": "integer", "description": "row limit (default 50)" } }, "required": ["project_root"] },
        },
        {
            "name": "dbt_compile",
            "description": "Compile a model and return its rendered SQL (Jinja resolved).",
            "inputSchema": { "type": "object", "properties": { "project_root": root_prop, "model": { "type": "string" } }, "required": ["project_root", "model"] },
        },
    ])
}

fn call_tool(store: &LineageStore, name: &str, arguments: &Value) -> Result<String> {
    match name {
        "dbt_list_models" => list_models(&project_root(arguments)?),
        "dbt_model_info" => {
            let root = project_root(arguments)?;
            let model = arg_str(arguments, "model").context("missing `model`")?;
            model_info(store, &root, model)
        }
        "dbt_lineage" => {
            let root = project_root(arguments)?;
            let model = arg_str(arguments, "model").context("missing `model`")?;
            let depth = arguments
                .get("depth")
                .and_then(|depth| depth.as_i64())
                .unwrap_or(3)
                .clamp(1, 12) as i32;
            lineage(store, &root, model, depth)
        }
        "dbt_column_lineage" => {
            let root = project_root(arguments)?;
            let model = arg_str(arguments, "model").context("missing `model`")?;
            let column = arg_str(arguments, "column").context("missing `column`")?;
            column_lineage(store, &root, model, &column.to_lowercase())
        }
        "dbt_show" => {
            let root = project_root(arguments)?;
            let limit = arguments
                .get("limit")
                .and_then(|limit| limit.as_u64())
                .unwrap_or(50)
                .clamp(1, 500);
            show(&root, arg_str(arguments, "model"), arg_str(arguments, "sql"), limit)
        }
        "dbt_compile" => {
            let root = project_root(arguments)?;
            let model = arg_str(arguments, "model").context("missing `model`")?;
            compile(&root, model)
        }
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

fn list_models(root: &Path) -> Result<String> {
    let manifest: Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(root.join("target").join("manifest.json"))
            .context("opening target/manifest.json — run `dbt parse` first")?,
    ))?;
    let mut models = Vec::new();
    for section in ["nodes", "sources"] {
        for node in manifest
            .get(section)
            .and_then(|section| section.as_object())
            .into_iter()
            .flat_map(|section| section.values())
        {
            let kind = node
                .get("resource_type")
                .and_then(|kind| kind.as_str())
                .unwrap_or("");
            if !matches!(kind, "model" | "source" | "seed" | "snapshot") {
                continue;
            }
            models.push(json!({
                "name": node.get("name").and_then(|name| name.as_str()).unwrap_or(""),
                "resource_type": kind,
                "materialized": node
                    .get("config")
                    .and_then(|config| config.get("materialized"))
                    .and_then(|materialized| materialized.as_str())
                    .unwrap_or(kind),
                "path": node
                    .get("original_file_path")
                    .and_then(|path| path.as_str())
                    .unwrap_or(""),
            }));
        }
    }
    Ok(serde_json::to_string_pretty(&json!({ "models": models }))?)
}

fn model_info(store: &LineageStore, root: &Path, model: &str) -> Result<String> {
    let layout = store.lineage_layout(root, model, 1, 100, &HashSet::new())?;
    let node = layout
        .nodes
        .iter()
        .find(|node| node.is_center)
        .with_context(|| format!("{model} not found"))?;
    let columns: Vec<Value> = node
        .columns
        .iter()
        .map(|column| {
            let lower = column.to_lowercase();
            json!({
                "name": column,
                "expression": node.col_exprs.get(&lower),
                "sources": node.col_lineage.get(&lower),
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&json!({
        "name": node.name,
        "kind": node.kind,
        "materialization": node.materialization,
        "path": node.path,
        "details": node.details,
        "operations": node.ops.as_ref().map(|ops| json!({
            "joins": ops.joins,
            "aggregations": ops.aggregations,
            "group_by": ops.group_by,
            "filters": ops.filters,
            "windows": ops.windows,
            "distinct": ops.distinct,
            "unions": ops.unions,
        })),
        "columns": columns,
    }))?)
}

fn lineage(store: &LineageStore, root: &Path, model: &str, depth: i32) -> Result<String> {
    let layout = store.lineage_layout(root, model, depth, 500, &HashSet::new())?;
    let nodes: Vec<Value> = layout
        .nodes
        .iter()
        .map(|node| {
            json!({
                "name": node.name,
                "level": node.level,
                "materialization": node.materialization,
                "is_center": node.is_center,
                "truncated_upstream": node.truncated_up,
                "truncated_downstream": node.truncated_down,
            })
        })
        .collect();
    let edges: Vec<Value> = layout
        .edges
        .iter()
        .filter_map(|(from, to)| {
            Some(json!([
                layout.nodes.get(*from)?.name.clone(),
                layout.nodes.get(*to)?.name.clone(),
            ]))
        })
        .collect();
    Ok(serde_json::to_string_pretty(&json!({ "nodes": nodes, "edges": edges }))?)
}

fn column_lineage(store: &LineageStore, root: &Path, model: &str, column: &str) -> Result<String> {
    let layout = store.lineage_layout(root, model, 8, 500, &HashSet::new())?;
    let marks = DbtResultsPanel::column_highlights(&layout, column);
    let mut steps: Vec<Value> = Vec::new();
    let mut indexed: Vec<(i32, Value)> = Vec::new();
    for (ix, node) in layout.nodes.iter().enumerate() {
        if marks[ix].is_empty() {
            continue;
        }
        for local in &marks[ix] {
            indexed.push((
                node.level,
                json!({
                    "model": node.name,
                    "level": node.level,
                    "column": local,
                    "expression": node.col_exprs.get(local),
                }),
            ));
        }
    }
    indexed.sort_by_key(|(level, _)| *level);
    steps.extend(indexed.into_iter().map(|(_, step)| step));
    anyhow::ensure!(!steps.is_empty(), "column {column} not found on {model}'s lineage");
    Ok(serde_json::to_string_pretty(&json!({ "column": column, "path": steps }))?)
}

fn show(root: &Path, model: Option<&str>, sql: Option<&str>, limit: u64) -> Result<String> {
    let settings = headless_settings();
    smol::block_on(async {
        let binary = crate::dbt_install::ensure_binary(&settings, None).await?;
        let mut command = util::command::new_command(&binary);
        command.arg("show");
        match (model, sql) {
            (_, Some(sql)) => {
                command.args(["--inline", sql]);
            }
            (Some(model), None) => {
                command.args(["--select", model]);
            }
            (None, None) => anyhow::bail!("pass `model` or `sql`"),
        }
        command.args(["--limit", &limit.to_string(), "--output", "json"]);
        apply_common_args(&mut command, &settings, root);
        let output = command.current_dir(root).output().await?;
        let (columns, rows) =
            parse_show_output(&output.stdout, &output.stderr, output.status.success())?;
        let rows: Vec<HashMap<&str, &str>> = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .zip(row.iter())
                    .map(|(column, value)| (column.as_ref(), value.as_ref()))
                    .collect()
            })
            .collect();
        Ok(serde_json::to_string_pretty(&json!({ "rows": rows }))?)
    })
}

fn compile(root: &Path, model: &str) -> Result<String> {
    let settings = headless_settings();
    smol::block_on(async {
        let binary = crate::dbt_install::ensure_binary(&settings, None).await?;
        let mut command = util::command::new_command(&binary);
        command.args(["compile", "--select", model]);
        apply_common_args(&mut command, &settings, root);
        let output = command.current_dir(root).output().await?;
        anyhow::ensure!(
            output.status.success(),
            "dbt compile failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // The compiled file lives under target/compiled; find it by stem.
        let compiled_dir = root.join("target").join("compiled");
        let mut stack = vec![compiled_dir];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_stem().and_then(|stem| stem.to_str()) == Some(model) {
                    return Ok(std::fs::read_to_string(&path)?);
                }
            }
        }
        anyhow::bail!("compiled SQL for {model} not found under target/compiled")
    })
}
