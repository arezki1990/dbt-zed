//! The pipeline builder forms: add a connection, add a source stream, or
//! edit the target — all by mouse, all writing the same YAML files the AI
//! agent and git see. The canvas is hands for the spec, never a bypass.

use anyhow::{Result, bail};
use editor::Editor;
use gpui::{Context, Entity, SharedString, WeakEntity, Window};
use indexmap::IndexMap;
use ui::prelude::*;

use el_engine::spec::{
    Connection, Connections, DbConn, DuckdbConn, FileFormat, Pipeline, SnowflakeAuth,
    SnowflakeConn, SourceObject, StreamSpec, TargetSpec,
};

use super::canvas_item::ElPipelineCanvas;

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
    Duckdb,
    Snowflake,
    Local,
}

impl ConnType {
    pub const ALL: &[ConnType] = &[
        ConnType::Duckdb,
        ConnType::Postgres,
        ConnType::Mysql,
        ConnType::Snowflake,
        ConnType::Local,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConnType::Postgres => "postgres",
            ConnType::Mysql => "mysql",
            ConnType::Duckdb => "duckdb",
            ConnType::Snowflake => "snowflake",
            ConnType::Local => "local files",
        }
    }
}

/// One labeled single-line editor.
struct Field {
    label: &'static str,
    editor: Entity<Editor>,
}

fn field(
    label: &'static str,
    placeholder: &str,
    initial: &str,
    window: &mut Window,
    cx: &mut Context<ElPipelineCanvas>,
) -> Field {
    let placeholder = placeholder.to_owned();
    let initial = initial.to_owned();
    Field {
        label,
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
}

/// The multi-select table picker's state for the Add-stream form.
pub enum TablesPick {
    /// File/local source, or no worker: type the table or path by hand.
    Manual,
    Loading,
    Loaded {
        items: Vec<(String, String)>,
        selected: std::collections::HashSet<usize>,
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

        let fields = match kind {
            BuilderKind::Source => vec![
                field("stream name", "orders", "", window, cx),
                field("schema (db sources)", "public", "", window, cx),
                field("table / path", "orders  ·  or  exports/*.parquet", "", window, cx),
            ],
            BuilderKind::Connection => vec![
                field("name", "pg_prod", "", window, cx),
                field("url / path", "${PG_PROD_URL}  ·  or  el/data.duckdb", "", window, cx),
                field("account (snowflake)", "${SNOWFLAKE_ACCOUNT}", "", window, cx),
                field("user (snowflake)", "${SNOWFLAKE_USER}", "", window, cx),
                field("key path (snowflake)", "${SNOWFLAKE_PK_PATH}", "", window, cx),
            ],
            BuilderKind::Target => {
                let target = pipeline.map(|pipeline| &pipeline.target);
                vec![
                    field(
                        "database",
                        "RAW",
                        target.and_then(|t| t.database.as_deref()).unwrap_or(""),
                        window,
                        cx,
                    ),
                    field(
                        "schema",
                        "LANDING",
                        target.map(|t| t.schema.as_str()).unwrap_or(""),
                        window,
                        cx,
                    ),
                    field(
                        "table template",
                        "{stream}",
                        target.and_then(|t| t.table.as_deref()).unwrap_or(""),
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

        Self {
            kind,
            conn_type: ConnType::Duckdb,
            picked_connection,
            connection_names,
            fields,
            format: FileFormat::Csv,
            tables: TablesPick::Manual,
            error: None,
        }
    }

    fn text(&self, ix: usize, cx: &Context<ElPipelineCanvas>) -> String {
        self.fields
            .get(ix)
            .map(|field| field.editor.read(cx).text(cx).trim().to_owned())
            .unwrap_or_default()
    }

    /// Produces the updated spec(s). Never touches disk — the canvas
    /// routes the result through the buffer writers.
    pub fn build(
        &self,
        pipeline: Option<Pipeline>,
        connections: Option<Connections>,
        cx: &Context<ElPipelineCanvas>,
    ) -> Result<BuilderOutcome> {
        match self.kind {
            BuilderKind::Source => {
                let mut pipeline = pipeline.ok_or_else(|| anyhow::anyhow!("no pipeline open"))?;
                if let Some(picked) = &self.picked_connection {
                    pipeline.source = picked.to_string();
                }
                // Checked tables win: one stream per selection.
                if let TablesPick::Loaded { items, selected } = &self.tables {
                    if !selected.is_empty() {
                        let mut added = 0usize;
                        for ix in 0..items.len() {
                            if !selected.contains(&ix) {
                                continue;
                            }
                            let (schema, table) = &items[ix];
                            if pipeline.streams.iter().any(|stream| stream.name == *table) {
                                continue; // already a stream — skip quietly
                            }
                            pipeline.streams.push(StreamSpec {
                                name: table.clone(),
                                source: SourceObject::Table {
                                    schema: Some(schema.clone()),
                                    table: table.clone(),
                                },
                                mode: None,
                                primary_key: vec![],
                                update_key: None,
                                target_table: None,
                                select: None,
                                columns: vec![],
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
                    target_table: None,
                    select: None,
                    columns: vec![],
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
                        Connection::Postgres(DbConn { url: url_or_path, extra: Default::default() })
                    }
                    ConnType::Mysql => {
                        if url_or_path.is_empty() {
                            bail!("mysql needs a url (use ${{VAR}} for credentials)");
                        }
                        Connection::Mysql(DbConn { url: url_or_path, extra: Default::default() })
                    }
                    ConnType::Duckdb => {
                        if url_or_path.is_empty() {
                            bail!("duckdb needs a file path");
                        }
                        Connection::Duckdb(DuckdbConn { path: url_or_path, extra: Default::default() })
                    }
                    ConnType::Local => Connection::Local { extra: Default::default() },
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
                            auth: SnowflakeAuth::KeyPair {
                                private_key_path: key_path,
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
                let schema = self.text(1, cx);
                if schema.is_empty() {
                    bail!("the target needs a schema");
                }
                let database = self.text(0, cx);
                let table = self.text(2, cx);
                if let Some(picked) = &self.picked_connection {
                    pipeline.target.connection = picked.to_string();
                }
                pipeline.target = TargetSpec {
                    connection: pipeline.target.connection.clone(),
                    database: (!database.is_empty()).then_some(database),
                    schema,
                    table: (!table.is_empty()).then_some(table),
                };
                Ok(BuilderOutcome {
                    pipeline: Some(pipeline),
                    connections: None,
                })
            }
        }
    }

    pub fn render(
        &self,
        canvas: WeakEntity<ElPipelineCanvas>,
        cx: &mut Context<ElPipelineCanvas>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let title = match self.kind {
            BuilderKind::Source => "Add source stream",
            BuilderKind::Connection => "Add connection",
            BuilderKind::Target => "Target",
        };

        let mut card = v_flex()
            .w(px(440.))
            .max_h(px(520.))
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .p_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new(title).size(LabelSize::Small))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-builder-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.close_builder(cx))),
                    ),
            );

        // Connection picker (Source/Target) or type picker (Connection).
        match self.kind {
            BuilderKind::Connection => {
                let mut picker = h_flex().w_full().px_2().pt_2().gap_1().flex_wrap();
                for conn_type in ConnType::ALL {
                    let conn_type = *conn_type;
                    picker = picker.child(
                        Button::new(
                            SharedString::from(format!("el-ct-{}", conn_type.label())),
                            conn_type.label(),
                        )
                        .label_size(LabelSize::XSmall)
                        .toggle_state(self.conn_type == conn_type)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = &mut this.builder_mut() {
                                form.conn_type = conn_type;
                            }
                            cx.notify();
                        })),
                    );
                }
                card = card.child(picker);
            }
            BuilderKind::Source | BuilderKind::Target => {
                let mut picker = h_flex().w_full().px_2().pt_2().gap_1().flex_wrap();
                let target_kinds = self.kind == BuilderKind::Target;
                for (name, kind) in &self.connection_names {
                    if target_kinds && !matches!(kind.as_ref(), "snowflake" | "duckdb") {
                        continue;
                    }
                    let name = name.clone();
                    picker = picker.child(
                        Button::new(
                            SharedString::from(format!("el-pick-{name}")),
                            format!("{name} ({kind})"),
                        )
                        .label_size(LabelSize::XSmall)
                        .toggle_state(self.picked_connection.as_ref() == Some(&name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let is_source = if let Some(form) = &mut this.builder_mut() {
                                form.picked_connection = Some(name.clone());
                                form.kind == BuilderKind::Source
                            } else {
                                false
                            };
                            if is_source {
                                this.kick_builder_tables(cx);
                            }
                            cx.notify();
                        })),
                    );
                }
                card = card.child(picker);
            }
        }

        // Format picker for file sources.
        let list_driven = matches!(
            (&self.kind, &self.tables),
            (BuilderKind::Source, TablesPick::Loading | TablesPick::Loaded { .. })
        );
        if self.kind == BuilderKind::Source && !list_driven {
            let mut formats = h_flex().w_full().px_2().pt_1().gap_1();
            for (format, label) in [
                (FileFormat::Csv, "csv"),
                (FileFormat::Parquet, "parquet"),
                (FileFormat::Ndjson, "ndjson"),
            ] {
                formats = formats.child(
                    Button::new(SharedString::from(format!("el-fmt-{label}")), label)
                        .label_size(LabelSize::XSmall)
                        .toggle_state(self.format == format)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = &mut this.builder_mut() {
                                form.format = format;
                            }
                            cx.notify();
                        })),
                );
            }
            card = card.child(formats);
        }

        let mut card = card;
        if self.kind == BuilderKind::Source {
            match &self.tables {
                TablesPick::Manual => {}
                TablesPick::Loading => {
                    card = card.child(div().px_2().pt_2().child(
                        Label::new("Loading tables…")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ));
                }
                TablesPick::Failed(message) => {
                    card = card.child(div().px_2().pt_2().child(
                        Label::new(message.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Error),
                    ));
                }
                TablesPick::Loaded { items, selected } => {
                    let mut list = v_flex()
                        .id("el-builder-tables")
                        .w_full()
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .mt_2()
                        .mx_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(colors.border);
                    for (ix, (schema, table)) in items.iter().enumerate() {
                        let checked = selected.contains(&ix);
                        list = list.child(
                            h_flex()
                                .id(("el-builder-table", ix))
                                .w_full()
                                .px_2()
                                .py_0p5()
                                .gap_1()
                                .items_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(colors.element_hover))
                                .child(
                                    Icon::new(if checked {
                                        IconName::Check
                                    } else {
                                        IconName::Circle
                                    })
                                    .size(IconSize::XSmall)
                                    .color(if checked { Color::Accent } else { Color::Muted }),
                                )
                                .child(
                                    Label::new(format!("{schema}.{table}"))
                                        .size(LabelSize::Small)
                                        .truncate(),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(form) = this.builder_mut() {
                                        if let TablesPick::Loaded { selected, .. } =
                                            &mut form.tables
                                        {
                                            if !selected.remove(&ix) {
                                                selected.insert(ix);
                                            }
                                        }
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    let count = selected.len();
                    card = card
                        .child(div().px_2().pt_2().child(
                            Label::new("Check tables — each becomes a stream.")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ))
                        .child(list)
                        .when(count > 0, |card| {
                            card.child(div().px_2().pt_1().child(
                                Label::new(format!("{count} selected"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ))
                        });
                }
            }
        }

        let mut fields = v_flex().w_full().p_2().gap_1();
        let manual_source = self.kind == BuilderKind::Source && list_driven;
        let show_snowflake_fields =
            self.kind != BuilderKind::Connection || self.conn_type == ConnType::Snowflake;
        for (ix, field) in self.fields.iter().enumerate() {
            if manual_source {
                break;
            }
            // Connection form: url/path for simple kinds, account/user/key
            // for snowflake.
            if self.kind == BuilderKind::Connection {
                let simple = matches!(
                    self.conn_type,
                    ConnType::Postgres | ConnType::Mysql | ConnType::Duckdb
                );
                let visible = match ix {
                    0 => true,
                    1 => simple,
                    _ => self.conn_type == ConnType::Snowflake,
                };
                if !visible {
                    continue;
                }
            }
            fields = fields.child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(140.)).flex_shrink_0().child(
                            Label::new(field.label)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                    )
                    .child(div().flex_1().child(field.editor.clone())),
            );
        }
        let _ = show_snowflake_fields;
        card = card.child(fields);

        if let Some(error) = &self.error {
            card = card.child(
                div().px_2().pb_1().child(
                    Label::new(error.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Error),
                ),
            );
        }

        let _ = canvas;
        card.child(
            h_flex()
                .w_full()
                .p_2()
                .gap_1()
                .border_t_1()
                .border_color(colors.border)
                .child(div().flex_1())
                .child(
                    Button::new("el-builder-cancel", "Cancel")
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.close_builder(cx))),
                )
                .child(
                    Button::new(
                        "el-builder-apply",
                        match &self.tables {
                            TablesPick::Loaded { selected, .. } if selected.len() > 1 => {
                                SharedString::from(format!("Add {} streams", selected.len()))
                            }
                            _ => "Add".into(),
                        },
                    )
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.apply_builder(window, cx)
                        })),
                ),
        )
        .into_any_element()
    }
}
