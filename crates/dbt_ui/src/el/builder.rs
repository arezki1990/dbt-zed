//! The pipeline builder forms: add a connection, add a source stream, or
//! edit the target — all by mouse or keyboard, all writing the same YAML
//! files the AI agent and git see. The canvas is hands for the spec, never
//! a bypass.

use std::collections::HashSet;

use anyhow::{Result, bail};
use editor::Editor;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable as _, ScrollHandle, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window,
};
use indexmap::IndexMap;
use ui::{Checkbox, HighlightedLabel, Tooltip, WithScrollbar as _, prelude::*};

use el_engine::spec::{
    Connection, Connections, DbConn, DuckdbConn, FileFormat, MssqlConn, OracleConn, Pipeline,
    SnowflakeAuth, SnowflakeConn, SourceObject, StreamSpec, TargetSpec,
};

use super::canvas_item::ElPipelineCanvas;
use super::dialog;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuilderKind {
    Source,
    Connection,
    Target,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnType {
    Postgres,
    Mysql,
    Oracle,
    Mssql,
    Duckdb,
    Snowflake,
    Local,
}

impl ConnType {
    pub const ALL: &[ConnType] = &[
        ConnType::Duckdb,
        ConnType::Postgres,
        ConnType::Mysql,
        ConnType::Oracle,
        ConnType::Mssql,
        ConnType::Snowflake,
        ConnType::Local,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConnType::Postgres => "postgres",
            ConnType::Mysql => "mysql",
            ConnType::Oracle => "oracle",
            ConnType::Mssql => "sql server",
            ConnType::Duckdb => "duckdb",
            ConnType::Snowflake => "snowflake",
            ConnType::Local => "local files",
        }
    }
}

/// Checklist rows; fixed, because the list is virtualized.
const TABLE_ROW_HEIGHT: f32 = 28.;
/// Rows the checklist shows before it scrolls.
const TABLE_ROWS_VISIBLE: usize = 10;

/// One labeled single-line editor.
struct Field {
    label: &'static str,
    hint: Option<&'static str>,
    /// What the form opened with, to tell an untouched form from one that
    /// a stray click must not throw away.
    initial: String,
    editor: Entity<Editor>,
}

fn field(
    label: &'static str,
    placeholder: &str,
    hint: Option<&'static str>,
    initial: &str,
    window: &mut Window,
    cx: &mut Context<ElPipelineCanvas>,
) -> Field {
    let placeholder = placeholder.to_owned();
    let initial = initial.to_owned();
    Field {
        label,
        hint,
        initial: initial.clone(),
        editor: cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(&placeholder, window, cx);
            if !initial.is_empty() {
                editor.set_text(initial, window, cx);
            }
            editor
        }),
    }
}

pub struct BuilderForm {
    pub kind: BuilderKind,
    /// Connection type for BuilderKind::Connection; source-connection pick
    /// for BuilderKind::Source; target connection for Target.
    pub conn_type: ConnType,
    pub picked_connection: Option<SharedString>,
    pub connection_names: Vec<(SharedString, SharedString)>, // (name, kind)
    fields: Vec<Field>,
    pub format: FileFormat,
    /// The source connection's live table list — check several, each
    /// becomes a stream.
    pub tables: TablesPick,
    pub error: Option<SharedString>,
    focus_handle: FocusHandle,
    /// Narrows the table checklist; it holds focus while the list shows.
    search: Entity<Editor>,
    terms: Vec<String>,
    /// Indices into the loaded tables that contain every search term.
    matches: Vec<usize>,
    /// The checklist row the arrow keys are on, as a position among the
    /// rows shown: ⌘⏎ checks it, and ⏎ adds it when nothing is checked.
    highlighted: Option<usize>,
    tables_scroll: UniformListScrollHandle,
    body_scroll: ScrollHandle,
    /// Tables the pipeline already streams; they can't be added twice.
    existing: HashSet<String>,
    _search_edits: Subscription,
}

/// The multi-select table picker's state for the Add-stream form.
pub enum TablesPick {
    /// File/local source, or no worker: type the table or path by hand.
    Manual,
    Loading,
    Loaded {
        items: Vec<(String, String)>,
        /// Each item as lowercase `schema.table`, for search.
        haystack: Vec<String>,
        selected: HashSet<usize>,
    },
    Failed(SharedString),
}

/// What Apply produced: either or both files to write.
pub struct BuilderOutcome {
    pub pipeline: Option<Pipeline>,
    pub connections: Option<Connections>,
}

impl BuilderForm {
    pub fn open(
        kind: BuilderKind,
        pipeline: Option<&Pipeline>,
        connections: Option<&Connections>,
        window: &mut Window,
        cx: &mut Context<ElPipelineCanvas>,
    ) -> Self {
        let connection_names: Vec<(SharedString, SharedString)> = connections
            .map(|connections| {
                connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        (name.clone().into(), connection.kind().to_owned().into())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let secret_hint = Some("A ${VAR} reference — the value itself stays in .env.");
        let fields = match kind {
            BuilderKind::Source => vec![
                field("Stream name", "orders", None, "", window, cx),
                field(
                    "Schema",
                    "public",
                    Some("Database sources only."),
                    "",
                    window,
                    cx,
                ),
                field(
                    "Table or path",
                    "orders",
                    Some("A table, or a file path or glob such as exports/*.parquet."),
                    "",
                    window,
                    cx,
                ),
            ],
            BuilderKind::Connection => vec![
                field("Name", "pg_prod", None, "", window, cx),
                field(
                    "URL or path",
                    "${PG_PROD_URL}",
                    Some("Postgres and MySQL take a ${VAR} URL; DuckDB a file path."),
                    "",
                    window,
                    cx,
                ),
                field("Account", "${SNOWFLAKE_ACCOUNT}", None, "", window, cx),
                field("User", "${SNOWFLAKE_USER}", None, "", window, cx),
                field(
                    "Private key path",
                    "${SNOWFLAKE_PK_PATH}",
                    secret_hint,
                    "",
                    window,
                    cx,
                ),
                field("User", "${ORACLE_USER}", None, "", window, cx),
                field(
                    "Password",
                    "${ORACLE_PASSWORD}",
                    secret_hint,
                    "",
                    window,
                    cx,
                ),
                field(
                    "Connect",
                    "db.example.com:1521/ORCLPDB1",
                    Some("host:port/service_name, or a TNS alias."),
                    "",
                    window,
                    cx,
                ),
                field(
                    "Host",
                    "sql.example.com",
                    Some("Port 1433 unless the connection says otherwise."),
                    "",
                    window,
                    cx,
                ),
                field("Database", "Sales", None, "", window, cx),
                field("User", "${MSSQL_USER}", None, "", window, cx),
                field("Password", "${MSSQL_PASSWORD}", secret_hint, "", window, cx),
            ],
            BuilderKind::Target => {
                let target = pipeline.map(|pipeline| &pipeline.target);
                vec![
                    field(
                        "Database",
                        "RAW",
                        Some("Optional for DuckDB and Oracle."),
                        target.and_then(|t| t.database.as_deref()).unwrap_or(""),
                        window,
                        cx,
                    ),
                    field(
                        "Schema",
                        "LANDING",
                        None,
                        target.map(|t| t.schema.as_str()).unwrap_or(""),
                        window,
                        cx,
                    ),
                    field(
                        "Table name",
                        "{stream}",
                        Some("{stream} becomes each stream's name."),
                        target.and_then(|t| t.table.as_deref()).unwrap_or(""),
                        window,
                        cx,
                    ),
                    // A file store's, in place of the three above.
                    field(
                        "Path",
                        "s3://bucket/gold/{stream}/",
                        Some(
                            "Each stream's directory: {stream} becomes its name. Parquet, \
                             replaced each run, unless the YAML says format or layout.",
                        ),
                        target.and_then(|t| t.path.as_deref()).unwrap_or(""),
                        window,
                        cx,
                    ),
                ]
            }
        };

        let picked_connection = match kind {
            BuilderKind::Source => pipeline.map(|pipeline| pipeline.source.clone().into()),
            BuilderKind::Target => {
                pipeline.map(|pipeline| pipeline.target.connection.clone().into())
            }
            BuilderKind::Connection => None,
        };

        let search = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Search tables…", window, cx);
            editor
        });
        let search_edits = cx.subscribe(
            &search,
            |canvas: &mut ElPipelineCanvas, _, event: &editor::EditorEvent, cx| {
                if matches!(event, editor::EditorEvent::BufferEdited) {
                    if let Some(form) = canvas.builder_mut() {
                        form.search_changed(cx);
                    }
                    cx.notify();
                }
            },
        );

        Self {
            kind,
            conn_type: ConnType::Duckdb,
            picked_connection,
            connection_names,
            fields,
            format: FileFormat::Csv,
            tables: TablesPick::Manual,
            error: None,
            focus_handle: cx.focus_handle(),
            search,
            terms: Vec::new(),
            matches: Vec::new(),
            highlighted: None,
            tables_scroll: UniformListScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            existing: pipeline
                .map(|pipeline| {
                    pipeline
                        .streams
                        .iter()
                        .map(|stream| stream.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            _search_edits: search_edits,
        }
    }

    fn text(&self, ix: usize, cx: &App) -> String {
        self.fields
            .get(ix)
            .map(|field| field.editor.read(cx).text(cx).trim().to_owned())
            .unwrap_or_default()
    }

    /// The checklist, rather than typed fields, describes the stream.
    fn list_driven(&self) -> bool {
        self.kind == BuilderKind::Source
            && matches!(self.tables, TablesPick::Loading | TablesPick::Loaded { .. })
    }

    /// Whether the target picked is a file store: it takes a path, not a
    /// schema.
    fn target_writes_files(&self) -> bool {
        self.picked_connection
            .as_ref()
            .and_then(|picked| self.connection_names.iter().find(|(name, _)| name == picked))
            .is_some_and(|(_, kind)| el_engine::spec::is_file_kind(kind))
    }

    /// The fields the form shows for its kind and connection type.
    fn visible_fields(&self) -> Vec<&Field> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(ix, _)| match self.kind {
                BuilderKind::Source => !self.list_driven(),
                // A database's schema and names, or a file store's path.
                BuilderKind::Target => (*ix == 3) == self.target_writes_files(),
                BuilderKind::Connection => match ix {
                    0 => true,
                    1 => matches!(
                        self.conn_type,
                        ConnType::Postgres | ConnType::Mysql | ConnType::Duckdb
                    ),
                    2..=4 => self.conn_type == ConnType::Snowflake,
                    5..=7 => self.conn_type == ConnType::Oracle,
                    _ => self.conn_type == ConnType::Mssql,
                },
            })
            .map(|(_, field)| field)
            .collect()
    }

    /// Where typing should land: the search over a table list, else the
    /// first field.
    pub fn initial_focus(&self, cx: &App) -> FocusHandle {
        if self.list_driven() {
            return self.search.focus_handle(cx);
        }
        self.visible_fields()
            .first()
            .map(|field| field.editor.focus_handle(cx))
            .unwrap_or_else(|| self.focus_handle.clone())
    }

    /// Tab and shift-tab: through the fields, or along the table list.
    pub fn step(&mut self, forward: bool, window: &mut Window, cx: &mut App) {
        if self.list_driven() {
            self.move_highlight(forward);
            return;
        }
        let fields: Vec<FocusHandle> = self
            .visible_fields()
            .iter()
            .map(|field| field.editor.focus_handle(cx))
            .collect();
        dialog::cycle_focus(&fields, forward, window, cx);
    }

    /// Something typed or checked that a click outside must not discard.
    pub fn is_dirty(&self, cx: &App) -> bool {
        matches!(&self.tables, TablesPick::Loaded { selected, .. } if !selected.is_empty())
            || !self.search.read(cx).text(cx).is_empty()
            || self
                .fields
                .iter()
                .any(|field| field.editor.read(cx).text(cx) != field.initial)
    }

    pub fn tables_loaded(&mut self, items: Vec<(String, String)>, haystack: Vec<String>) {
        self.tables = TablesPick::Loaded {
            items,
            haystack,
            selected: HashSet::new(),
        };
        self.rematch();
    }

    fn search_changed(&mut self, cx: &App) {
        self.terms = super::search_terms(&self.search.read(cx).text(cx));
        self.error = None;
        self.rematch();
    }

    fn rematch(&mut self) {
        if let TablesPick::Loaded { haystack, .. } = &self.tables {
            self.matches = if self.terms.is_empty() {
                Vec::new()
            } else {
                super::matching_tables(haystack, &self.terms)
            };
        }
        // A search puts the arrows on its best match, so ⏎ adds it.
        self.highlighted = (!self.terms.is_empty() && self.shown_count() > 0).then_some(0);
        self.tables_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Rows the checklist shows: every table, or the search's matches.
    fn shown_count(&self) -> usize {
        match &self.tables {
            TablesPick::Loaded { items, .. } if self.terms.is_empty() => items.len(),
            TablesPick::Loaded { .. } => self.matches.len(),
            _ => 0,
        }
    }

    fn shown_table(&self, position: usize) -> Option<usize> {
        if self.terms.is_empty() {
            (position < self.shown_count()).then_some(position)
        } else {
            self.matches.get(position).copied()
        }
    }

    fn already_streamed(&self, table: usize) -> bool {
        match &self.tables {
            TablesPick::Loaded { items, .. } => items
                .get(table)
                .is_some_and(|(_, name)| self.existing.contains(name)),
            _ => false,
        }
    }

    pub fn move_highlight(&mut self, forward: bool) {
        let count = self.shown_count();
        if count == 0 {
            self.highlighted = None;
            return;
        }
        let next = match (self.highlighted, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(position), true) => (position + 1).min(count - 1),
            (Some(position), false) => position.saturating_sub(1),
        };
        self.highlighted = Some(next);
        self.tables_scroll
            .scroll_to_item(next, ScrollStrategy::Nearest);
    }

    fn toggle(&mut self, table: usize) {
        if self.already_streamed(table) {
            return;
        }
        if let TablesPick::Loaded { selected, .. } = &mut self.tables {
            if !selected.remove(&table) {
                selected.insert(table);
            }
        }
        self.error = None;
    }

    pub fn toggle_highlighted(&mut self) {
        if let Some(table) = self
            .highlighted
            .and_then(|position| self.shown_table(position))
        {
            self.toggle(table);
        }
    }

    fn select_all_shown(&mut self) {
        let shown: Vec<usize> = (0..self.shown_count())
            .filter_map(|position| self.shown_table(position))
            .filter(|table| !self.already_streamed(*table))
            .collect();
        if let TablesPick::Loaded { selected, .. } = &mut self.tables {
            selected.extend(shown);
        }
        self.error = None;
    }

    fn clear_selection(&mut self) {
        if let TablesPick::Loaded { selected, .. } = &mut self.tables {
            selected.clear();
        }
    }

    /// What Add takes from the checklist: the checked tables, or — with
    /// none checked — the highlighted one.
    fn chosen_tables(&self) -> Vec<usize> {
        let TablesPick::Loaded { selected, .. } = &self.tables else {
            return Vec::new();
        };
        if !selected.is_empty() {
            let mut chosen: Vec<usize> = selected.iter().copied().collect();
            chosen.sort_unstable();
            return chosen;
        }
        self.highlighted
            .and_then(|position| self.shown_table(position))
            .filter(|table| !self.already_streamed(*table))
            .into_iter()
            .collect()
    }

    /// Produces the updated spec(s). Never touches disk — the canvas
    /// routes the result through the buffer writers.
    pub fn build(
        &self,
        pipeline: Option<Pipeline>,
        connections: Option<Connections>,
        cx: &App,
    ) -> Result<BuilderOutcome> {
        match self.kind {
            BuilderKind::Source => {
                let mut pipeline = pipeline.ok_or_else(|| anyhow::anyhow!("no pipeline open"))?;
                if let Some(picked) = &self.picked_connection {
                    pipeline.source = picked.to_string();
                }
                match &self.tables {
                    TablesPick::Loading => bail!("the table list is still loading"),
                    TablesPick::Loaded { items, .. } => {
                        let chosen = self.chosen_tables();
                        if chosen.is_empty() {
                            bail!("check the tables to add");
                        }
                        let mut added = 0usize;
                        for table in chosen {
                            let Some((schema, name)) = items.get(table) else {
                                continue;
                            };
                            if pipeline.streams.iter().any(|stream| stream.name == *name) {
                                continue; // already a stream — skip quietly
                            }
                            pipeline.streams.push(StreamSpec {
                                name: name.clone(),
                                source: SourceObject::Table {
                                    schema: Some(schema.clone()),
                                    table: name.clone(),
                                },
                                mode: None,
                                primary_key: vec![],
                                update_key: None,
            partition: None,
                                target_table: None,
                                select: None,
                                columns: vec![],
                                share: None,
                                extra: IndexMap::new(),
                            });
                            added += 1;
                        }
                        if added == 0 {
                            bail!("every checked table is already in the pipeline");
                        }
                        return Ok(BuilderOutcome {
                            pipeline: Some(pipeline),
                            connections: None,
                        });
                    }
                    TablesPick::Manual | TablesPick::Failed(_) => {}
                }
                let name = self.text(0, cx);
                if name.is_empty() {
                    bail!("the stream needs a name");
                }
                if pipeline.streams.iter().any(|stream| stream.name == name) {
                    bail!("a stream named {name:?} already exists");
                }
                let schema = self.text(1, cx);
                let table_or_path = self.text(2, cx);
                if table_or_path.is_empty() {
                    bail!("give the source a table or a file path");
                }
                let source_kind = connections
                    .as_ref()
                    .and_then(|connections| connections.connections.get(&pipeline.source))
                    .map(Connection::kind);
                let source = match source_kind {
                    Some("local" | "s3" | "gcs" | "azure") => SourceObject::Path {
                        path: table_or_path,
                        format: self.format,
                        csv: None,
                    },
                    _ => SourceObject::Table {
                        schema: (!schema.is_empty()).then_some(schema),
                        table: table_or_path,
                    },
                };
                pipeline.streams.push(StreamSpec {
                    name,
                    source,
                    mode: None,
                    primary_key: vec![],
                    update_key: None,
            partition: None,
                    target_table: None,
                    select: None,
                    columns: vec![],
                    share: None,
                    extra: IndexMap::new(),
                });
                Ok(BuilderOutcome {
                    pipeline: Some(pipeline),
                    connections: None,
                })
            }
            BuilderKind::Connection => {
                let mut connections = connections.unwrap_or(Connections {
                    version: 1,
                    connections: IndexMap::new(),
                    profiles: IndexMap::new(),
                    default_profile: None,
                    dbt_targets: IndexMap::new(),
                    default_dbt_target: None,
                    shared_datasets: IndexMap::new(),
                    extra: IndexMap::new(),
                });
                let name = self.text(0, cx);
                if name.is_empty() {
                    bail!("the connection needs a name");
                }
                if connections.connections.contains_key(&name) {
                    bail!("a connection named {name:?} already exists");
                }
                let url_or_path = self.text(1, cx);
                let connection = match self.conn_type {
                    ConnType::Postgres => {
                        if url_or_path.is_empty() {
                            bail!("postgres needs a url (use ${{VAR}} for credentials)");
                        }
                        Connection::Postgres(DbConn {
                            url: url_or_path,
                            workspace: None,
                            extra: Default::default(),
                        })
                    }
                    ConnType::Mysql => {
                        if url_or_path.is_empty() {
                            bail!("mysql needs a url (use ${{VAR}} for credentials)");
                        }
                        Connection::Mysql(DbConn {
                            url: url_or_path,
                            workspace: None,
                            extra: Default::default(),
                        })
                    }
                    ConnType::Duckdb => {
                        if url_or_path.is_empty() {
                            bail!("duckdb needs a file path");
                        }
                        Connection::Duckdb(DuckdbConn {
                            path: url_or_path,
                            workspace: None,
                            extra: Default::default(),
                        })
                    }
                    ConnType::Oracle => {
                        let user = self.text(5, cx);
                        let password = self.text(6, cx);
                        let connect = self.text(7, cx);
                        if user.is_empty() || password.is_empty() || connect.is_empty() {
                            bail!(
                                "oracle needs user, password and connect — the connect \
                                 string is host:port/service_name or a TNS alias"
                            );
                        }
                        if !password.starts_with("${") {
                            bail!(
                                "store the password in the environment and reference it \
                                 like ${{ORACLE_PASSWORD}} — never a literal in YAML"
                            );
                        }
                        Connection::Oracle(OracleConn {
                            user,
                            password,
                            connect,
                            schema: None,
                            wallet_dir: None,
                            tns_admin: None,
                            workspace: None,
                            driver: None,
                            extra: Default::default(),
                        })
                    }
                    ConnType::Mssql => {
                        let host = self.text(8, cx);
                        let database = self.text(9, cx);
                        let user = self.text(10, cx);
                        let password = self.text(11, cx);
                        if host.is_empty()
                            || database.is_empty()
                            || user.is_empty()
                            || password.is_empty()
                        {
                            bail!("sql server needs host, database, user and password");
                        }
                        if !password.starts_with("${") {
                            bail!(
                                "store the password in the environment and reference it \
                                 like ${{MSSQL_PASSWORD}} — never a literal in YAML"
                            );
                        }
                        Connection::Mssql(MssqlConn {
                            host,
                            port: None,
                            database,
                            user,
                            password,
                            encrypt: None,
                            trust_server_certificate: None,
                            workspace: None,
                            extra: Default::default(),
                        })
                    }
                    ConnType::Local => Connection::Local {
                        extra: Default::default(),
                    },
                    ConnType::Snowflake => {
                        let account = self.text(2, cx);
                        let user = self.text(3, cx);
                        let key_path = self.text(4, cx);
                        if account.is_empty() || user.is_empty() || key_path.is_empty() {
                            bail!(
                                "snowflake needs account, user and a private-key path — \
                                 reference env variables like ${{SNOWFLAKE_ACCOUNT}}"
                            );
                        }
                        Connection::Snowflake(SnowflakeConn {
                            account,
                            user,
                            role: None,
                            warehouse: None,
                            database: None,
                            workspace: None,
                            auth: SnowflakeAuth::KeyPair {
                                private_key: None,
                                private_key_path: Some(key_path),
                                private_key_passphrase: None,
                            },
                            extra: Default::default(),
                        })
                    }
                };
                connections.connections.insert(name, connection);
                Ok(BuilderOutcome {
                    pipeline: None,
                    connections: Some(connections),
                })
            }
            BuilderKind::Target => {
                let mut pipeline = pipeline.ok_or_else(|| anyhow::anyhow!("no pipeline open"))?;
                if let Some(picked) = &self.picked_connection {
                    pipeline.target.connection = picked.to_string();
                }
                pipeline.target = if self.target_writes_files() {
                    let path = self.text(3, cx);
                    if path.is_empty() {
                        bail!("a file target needs a path: where each stream's files go");
                    }
                    TargetSpec {
                        connection: pipeline.target.connection.clone(),
                        database: None,
                        schema: String::new(),
                        table: None,
                        path: Some(path),
                        format: pipeline.target.format,
                        layout: pipeline.target.layout,
                    }
                } else {
                    let schema = self.text(1, cx);
                    if schema.is_empty() {
                        bail!("the target needs a schema");
                    }
                    let database = self.text(0, cx);
                    let table = self.text(2, cx);
                    TargetSpec {
                        connection: pipeline.target.connection.clone(),
                        database: (!database.is_empty()).then_some(database),
                        schema,
                        table: (!table.is_empty()).then_some(table),
                        path: None,
                        format: None,
                        layout: None,
                    }
                };
                Ok(BuilderOutcome {
                    pipeline: Some(pipeline),
                    connections: None,
                })
            }
        }
    }

    /// The primary button's words: exactly what ⏎ would do.
    fn primary_label(&self) -> (SharedString, bool) {
        match self.kind {
            BuilderKind::Connection => ("Add connection".into(), true),
            BuilderKind::Target => ("Save target".into(), true),
            BuilderKind::Source => match &self.tables {
                TablesPick::Loading => ("Add streams".into(), false),
                TablesPick::Loaded {
                    items, selected, ..
                } => match selected.len() {
                    0 => match self
                        .chosen_tables()
                        .first()
                        .and_then(|table| items.get(*table))
                    {
                        Some((_, name)) => (format!("Add {name}").into(), true),
                        None => ("Add streams".into(), false),
                    },
                    1 => ("Add 1 stream".into(), true),
                    count => (
                        format!("Add {} streams", super::thousands(count)).into(),
                        true,
                    ),
                },
                TablesPick::Manual | TablesPick::Failed(_) => ("Add stream".into(), true),
            },
        }
    }

    pub fn render(
        &self,
        window: &mut Window,
        cx: &mut Context<ElPipelineCanvas>,
    ) -> gpui::AnyElement {
        let (title, detail): (&str, Option<SharedString>) = match self.kind {
            BuilderKind::Source => (
                "Add source stream",
                Some("Each table you check becomes a stream.".into()),
            ),
            BuilderKind::Connection => ("Add connection", None),
            BuilderKind::Target => (
                "Pipeline target",
                Some("Where every stream of this pipeline lands.".into()),
            ),
        };
        let mut body = dialog::body();

        // Connection picker (Source/Target) or type picker (Connection).
        match self.kind {
            BuilderKind::Connection => {
                let chips = ConnType::ALL.iter().map(|conn_type| {
                    let conn_type = *conn_type;
                    dialog::chip(
                        SharedString::from(format!("el-ct-{}", conn_type.label())),
                        conn_type.label(),
                        self.conn_type == conn_type,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(form) = this.builder_mut() {
                            form.conn_type = conn_type;
                            let focus = form.initial_focus(cx);
                            window.focus(&focus, cx);
                        }
                        cx.notify();
                    }))
                });
                body = body.child(dialog::labeled("Type", dialog::chips(chips)));
            }
            BuilderKind::Source | BuilderKind::Target => {
                let target_kinds = self.kind == BuilderKind::Target;
                let chips = self
                    .connection_names
                    .iter()
                    .filter(|(_, kind)| !target_kinds || super::loadable(kind.as_ref()))
                    .map(|(name, kind)| {
                        let pick = name.clone();
                        dialog::chip(
                            SharedString::from(format!("el-pick-{name}")),
                            format!("{name} · {kind}"),
                            self.picked_connection.as_ref() == Some(name),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let is_source = match this.builder_mut() {
                                    Some(form) => {
                                        form.picked_connection = Some(pick.clone());
                                        form.kind == BuilderKind::Source
                                    }
                                    None => false,
                                };
                                if is_source {
                                    this.kick_builder_tables(cx);
                                }
                                if let Some(form) = this.builder_mut() {
                                    let focus = form.initial_focus(cx);
                                    window.focus(&focus, cx);
                                }
                                cx.notify();
                            },
                        ))
                    })
                    .collect::<Vec<_>>();
                let label = if target_kinds { "Into" } else { "From" };
                body = body.child(if chips.is_empty() {
                    dialog::labeled(
                        label,
                        dialog::notice(
                            Severity::Warning,
                            "No connection can serve here yet — add one first.",
                        ),
                    )
                } else {
                    dialog::labeled(label, dialog::chips(chips))
                });
            }
        }

        if self.kind == BuilderKind::Source {
            body = body.child(self.render_tables(window, cx));
        }

        for field in self.visible_fields() {
            body = body.child(dialog::field(
                field.label,
                &field.editor,
                field.hint.map(SharedString::from),
                window,
                cx,
            ));
        }

        if let Some(error) = &self.error {
            body = body.child(dialog::notice(Severity::Error, error.clone()));
        }

        let (primary, enabled) = self.primary_label();
        let hints = (self.kind == BuilderKind::Source
            && matches!(self.tables, TablesPick::Loaded { .. }))
        .then(|| {
            Label::new("↑↓ move · ⌘⏎ check")
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element()
        });
        dialog::frame(dialog::NARROW, cx)
            .track_focus(&self.focus_handle)
            .max_h(px(620.))
            .child(dialog::header(
                title,
                detail,
                cx.listener(|this, _, _, cx| this.close_builder(cx)),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("el-builder-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.body_scroll)
                            .child(body),
                    )
                    .custom_scrollbars(super::scrollbar(&self.body_scroll), window, cx),
            )
            .child(dialog::footer(
                hints,
                [
                    dialog::cancel_button("el-builder-cancel", &self.focus_handle, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.close_builder(cx)))
                        .into_any_element(),
                    dialog::primary_button("el-builder-apply", primary, &self.focus_handle, cx)
                        .disabled(!enabled)
                        .on_click(cx.listener(|this, _, window, cx| this.apply_builder(window, cx)))
                        .into_any_element(),
                ],
                cx,
            ))
            .into_any_element()
    }

    /// The source's table checklist: search, counts, select all, and the
    /// virtualized list — or why there isn't one.
    fn render_tables(
        &self,
        window: &mut Window,
        cx: &mut Context<ElPipelineCanvas>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let connection = self
            .picked_connection
            .clone()
            .unwrap_or_else(|| "the source".into());
        let focused = self.search.focus_handle(cx).is_focused(window);
        let search_box = || {
            h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_1p5()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                })
                .bg(colors.editor_background)
                .child(
                    Icon::new(IconName::MagnifyingGlass)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(div().flex_1().min_w_0().child(self.search.clone()))
        };
        match &self.tables {
            TablesPick::Manual => {
                let formats = [
                    (FileFormat::Csv, "csv"),
                    (FileFormat::Parquet, "parquet"),
                    (FileFormat::Ndjson, "ndjson"),
                ]
                .into_iter()
                .map(|(format, label)| {
                    dialog::chip(
                        SharedString::from(format!("el-fmt-{label}")),
                        label,
                        self.format == format,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.builder_mut() {
                            form.format = format;
                        }
                        cx.notify();
                    }))
                });
                dialog::labeled("File format", dialog::chips(formats))
            }
            TablesPick::Loading => v_flex()
                .w_full()
                .gap_2()
                .child(search_box())
                .child(dialog::loading(format!(
                    "Listing the tables in {connection}…"
                )))
                .into_any_element(),
            TablesPick::Failed(message) => v_flex()
                .w_full()
                .gap_1()
                .child(dialog::notice(
                    Severity::Error,
                    format!("Couldn't list the tables in {connection}: {message}"),
                ))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("el-builder-retry", "Try again")
                                .label_size(LabelSize::Small)
                                .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kick_builder_tables(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Label::new("or describe the stream by hand below.")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                )
                .into_any_element(),
            TablesPick::Loaded {
                items, selected, ..
            } => {
                let total = items.len();
                let shown = self.shown_count();
                let searching = !self.terms.is_empty();
                let status = match (searching, selected.len()) {
                    (false, 0) => format!("{} tables", super::thousands(total)),
                    (false, checked) => format!(
                        "{} of {} checked",
                        super::thousands(checked),
                        super::thousands(total)
                    ),
                    (true, 0) => format!(
                        "{} of {} match",
                        super::thousands(shown),
                        super::thousands(total)
                    ),
                    (true, checked) => format!(
                        "{} of {} match · {} checked",
                        super::thousands(shown),
                        super::thousands(total),
                        super::thousands(checked)
                    ),
                };
                let toolbar = h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        Label::new(status)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(
                            "el-builder-select-all",
                            if searching {
                                format!("Check {}", super::thousands(shown))
                            } else {
                                "Check all".to_owned()
                            },
                        )
                        .label_size(LabelSize::Small)
                        .disabled(shown == 0)
                        .tooltip(Tooltip::text(if searching {
                            "Check every table the search matches"
                        } else {
                            "Check every table not already in the pipeline"
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(form) = this.builder_mut() {
                                form.select_all_shown();
                            }
                            cx.notify();
                        })),
                    )
                    .when(!selected.is_empty(), |toolbar| {
                        toolbar.child(
                            Button::new("el-builder-clear", "Uncheck all")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(form) = this.builder_mut() {
                                        form.clear_selection();
                                    }
                                    cx.notify();
                                })),
                        )
                    });

                let list: gpui::AnyElement = if shown == 0 {
                    div()
                        .w_full()
                        .py_3()
                        .flex()
                        .justify_center()
                        .child(
                            Label::new(if total == 0 {
                                format!("{connection} has no tables.")
                            } else {
                                "No table matches the search.".to_owned()
                            })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                        )
                        .into_any_element()
                } else {
                    let entity = cx.entity();
                    let list =
                        gpui::uniform_list("el-builder-tables", shown, move |range, _, cx| {
                            entity.update(cx, |canvas, cx| {
                                let Some(form) = canvas.builder() else {
                                    return Vec::new();
                                };
                                range
                                    .filter_map(|position| form.render_table_row(position, cx))
                                    .collect()
                            })
                        })
                        .track_scroll(&self.tables_scroll)
                        .w_full()
                        .h(px(TABLE_ROW_HEIGHT * shown.min(TABLE_ROWS_VISIBLE) as f32));
                    div()
                        .w_full()
                        .child(list)
                        .custom_scrollbars(super::scrollbar(&self.tables_scroll), window, cx)
                        .into_any_element()
                };
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(search_box())
                    .child(toolbar)
                    .child(
                        div()
                            .w_full()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.border_variant)
                            .overflow_hidden()
                            .child(list),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_table_row(
        &self,
        position: usize,
        cx: &mut Context<ElPipelineCanvas>,
    ) -> Option<gpui::AnyElement> {
        let TablesPick::Loaded {
            items, selected, ..
        } = &self.tables
        else {
            return None;
        };
        let table = self.shown_table(position)?;
        let (schema, name) = items.get(table)?;
        let colors = cx.theme().colors();
        let added = self.existing.contains(name);
        let checked = added || selected.contains(&table);
        let label = format!("{schema}.{name}");
        let highlights = super::highlight_ranges(&label, &self.terms);
        let label_element: gpui::AnyElement = if highlights.is_empty() {
            Label::new(label)
                .size(LabelSize::Small)
                .color(if added { Color::Muted } else { Color::Default })
                .truncate()
                .into_any_element()
        } else {
            HighlightedLabel::from_ranges(label, highlights)
                .size(LabelSize::Small)
                .color(if added { Color::Muted } else { Color::Default })
                .truncate()
                .into_any_element()
        };
        Some(
            h_flex()
                .id(("el-builder-table", table))
                .h(px(TABLE_ROW_HEIGHT))
                .w_full()
                .px_2()
                .gap_2()
                .items_center()
                .when(self.highlighted == Some(position), |row| {
                    row.bg(colors.element_selected)
                })
                .child(Checkbox::new(("el-builder-check", table), checked.into()).disabled(added))
                .child(div().flex_1().min_w_0().child(label_element))
                .when(added, |row| {
                    row.child(
                        Label::new("in pipeline")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                })
                .when(!added, |row| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.builder_mut() {
                                form.toggle(table);
                                form.highlighted = Some(position);
                            }
                            cx.notify();
                        }))
                })
                .into_any_element(),
        )
    }
}
