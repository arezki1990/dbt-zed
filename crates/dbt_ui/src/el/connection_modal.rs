//! The connection editor: add, edit, rename, or delete a connection in
//! el/connections.yml from anywhere in the workspace — a centered modal,
//! no canvas required. Values shown are exactly what the YAML holds
//! (`${VAR}` templates included); resolved secrets never surface. Renames
//! propagate to every pipeline that references the connection; deletes
//! are refused while any pipeline still does. All writes are
//! buffer-routed and pre-checked for dirty buffers, so a multi-file
//! rename either starts cleanly or not at all.

use std::path::PathBuf;

use anyhow::{Result, bail};
use editor::Editor;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    WeakEntity, Window,
};
use indexmap::IndexMap;
use project::Project;
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use el_engine::spec::{
    Connection, Connections, DbConn, DuckdbConn, Pipeline, SnowflakeAuth, SnowflakeConn,
    SpecError,
};

use super::builder::ConnType;
use super::panel::ElPanel;

struct Field {
    label: &'static str,
    editor: Entity<Editor>,
}

pub struct ElConnectionModal {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<ElPanel>,
    project: Entity<Project>,
    root: PathBuf,
    /// The original name when editing; None while adding.
    editing: Option<String>,
    /// The original kind string — drives the unsupported-kind fallback.
    original_kind: Option<&'static str>,
    /// Set when the open-time load failed or the connection is missing —
    /// the form becomes read-only (Open YAML is the way forward).
    broken: bool,
    conn_type: ConnType,
    /// Snowflake: password auth instead of key pair.
    auth_password: bool,
    name: Entity<Editor>,
    fields: Vec<Field>,
    /// Pipelines referencing this connection at open time (display only —
    /// save and delete re-scan the disk at click time).
    referencing: Vec<String>,
    delete_armed: bool,
    /// A write is in flight: buttons disabled, dismissal held.
    writing: bool,
    error: Option<SharedString>,
}

/// True for kinds the form can round-trip; others fall back to YAML.
fn form_supported(kind: &str) -> bool {
    matches!(
        kind,
        "postgres" | "mysql" | "duckdb" | "snowflake" | "local"
    )
}

fn conn_type_of(kind: &str) -> ConnType {
    match kind {
        "postgres" => ConnType::Postgres,
        "mysql" => ConnType::Mysql,
        "snowflake" => ConnType::Snowflake,
        "local" => ConnType::Local,
        _ => ConnType::Duckdb,
    }
}

/// Loads connections.yml, treating only true absence as an empty file. A
/// parse failure is an error — starting from an empty map there would let
/// a later Save wipe every connection the broken file still holds.
fn load_connections_strict(root: &std::path::Path) -> Result<Connections> {
    let path = super::el_dir(root).join("connections.yml");
    match el_engine::spec::load_connections(&path) {
        Ok(connections) => Ok(connections),
        Err(SpecError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(Connections {
                version: 1,
                connections: IndexMap::new(),
                profiles: IndexMap::new(),
                default_profile: None,
                extra: IndexMap::new(),
            })
        }
        Err(error) => bail!("connections.yml could not be read: {error} — fix the file first"),
    }
}

/// Pipeline names whose source or target uses `name`. A pipeline file
/// that fails to load is an error: it might reference the connection, so
/// no rename or delete may proceed past it.
fn referencing_pipelines(root: &std::path::Path, name: &str) -> Result<Vec<String>> {
    let mut referencing = Vec::new();
    for path in el_engine::spec::list_pipelines(&super::el_dir(root)) {
        let pipeline = el_engine::spec::load_pipeline(&path).map_err(|error| {
            anyhow::anyhow!(
                "{} could not be read ({error}) — fix it before changing connections",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("a pipeline file")
            )
        })?;
        if pipeline.source == name || pipeline.target.connection == name {
            referencing.push(pipeline.pipeline);
        }
    }
    Ok(referencing)
}

/// Carries hand-added unknown keys from the stored value into the freshly
/// built one when the kind is unchanged.
fn carry_extra(new_value: &mut Connection, stored: &Connection) {
    match (new_value, stored) {
        (Connection::Postgres(new), Connection::Postgres(old))
        | (Connection::Mysql(new), Connection::Mysql(old)) => {
            new.extra = old.extra.clone();
        }
        (Connection::Duckdb(new), Connection::Duckdb(old)) => new.extra = old.extra.clone(),
        (Connection::Snowflake(new), Connection::Snowflake(old)) => {
            new.extra = old.extra.clone();
        }
        (Connection::Local { extra }, Connection::Local { extra: old }) => {
            *extra = old.clone();
        }
        _ => {}
    }
}

impl ElConnectionModal {
    /// Deploys the modal over the workspace. `editing` = Some(name) opens
    /// the named connection; None opens a blank Add form.
    pub fn deploy(
        workspace: &mut Workspace,
        panel: WeakEntity<ElPanel>,
        root: PathBuf,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project().clone();
        let workspace_handle = cx.entity().downgrade();
        workspace.toggle_modal(window, cx, move |window, cx| {
            Self::new(workspace_handle, panel, project, root, editing, window, cx)
        });
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        panel: WeakEntity<ElPanel>,
        project: Entity<Project>,
        root: PathBuf,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut open_error: Option<SharedString> = None;
        let mut broken = false;
        let existing = editing.as_ref().and_then(|name| {
            match load_connections_strict(&root) {
                Ok(connections) => {
                    let found = connections.connections.get(name).cloned();
                    if found.is_none() {
                        open_error = Some(
                            format!("{name:?} is gone from connections.yml").into(),
                        );
                        broken = true;
                    }
                    found
                }
                Err(error) => {
                    open_error = Some(format!("{error:#}").into());
                    broken = true;
                    None
                }
            }
        });

        let mut make = |label: &'static str, placeholder: &str, initial: &str| Field {
            label,
            editor: {
                let placeholder = placeholder.to_owned();
                let initial = initial.to_owned();
                cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text(&placeholder, window, cx);
                    if !initial.is_empty() {
                        editor.set_text(initial, window, cx);
                    }
                    editor
                })
            },
        };

        // Prefill straight from the spec value; blank strings otherwise.
        let mut url_or_path = String::new();
        let mut account = String::new();
        let mut user = String::new();
        let mut role = String::new();
        let mut warehouse = String::new();
        let mut database = String::new();
        let mut secret = String::new();
        let mut auth_password = false;
        match &existing {
            Some(Connection::Postgres(conn)) | Some(Connection::Mysql(conn)) => {
                url_or_path = conn.url.clone();
            }
            Some(Connection::Duckdb(conn)) => url_or_path = conn.path.clone(),
            Some(Connection::Snowflake(conn)) => {
                account = conn.account.clone();
                user = conn.user.clone();
                role = conn.role.clone().unwrap_or_default();
                warehouse = conn.warehouse.clone().unwrap_or_default();
                database = conn.database.clone().unwrap_or_default();
                match &conn.auth {
                    SnowflakeAuth::KeyPair { private_key_path } => {
                        secret = private_key_path.clone();
                    }
                    SnowflakeAuth::Password { password } => {
                        auth_password = true;
                        // A ${VAR} reference round-trips; a literal
                        // credential is never echoed into the form.
                        if password.starts_with("${") {
                            secret = password.clone();
                        } else {
                            open_error = Some(
                                "the YAML holds a literal password — enter a ${VAR} \
                                 reference here and move the value to .env"
                                    .into(),
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        let fields = vec![
            make("url / path", "${PG_PROD_URL}  ·  or  el/data.duckdb", &url_or_path),
            make("account", "${SNOWFLAKE_ACCOUNT}", &account),
            make("user", "${SNOWFLAKE_USER}", &user),
            make("role", "LOADER (optional)", &role),
            make("warehouse", "LOAD_WH (optional)", &warehouse),
            make("database", "RAW (optional)", &database),
            make("key path / password", "${SNOWFLAKE_PK_PATH}", &secret),
        ];
        let name = make("name", "pg_prod", editing.as_deref().unwrap_or("")).editor;

        let referencing = editing
            .as_ref()
            .and_then(|name| referencing_pipelines(&root, name).ok())
            .unwrap_or_default();

        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            panel,
            project,
            root,
            original_kind: existing.as_ref().map(Connection::kind),
            broken,
            conn_type: existing
                .as_ref()
                .map(|connection| conn_type_of(connection.kind()))
                .unwrap_or(ConnType::Duckdb),
            editing,
            auth_password,
            name,
            fields,
            referencing,
            delete_armed: false,
            writing: false,
            error: open_error,
        }
    }

    fn form_supported(&self) -> bool {
        self.original_kind.map_or(true, form_supported)
    }

    fn text(&self, ix: usize, cx: &App) -> String {
        self.fields
            .get(ix)
            .map(|field| field.editor.read(cx).text(cx).trim().to_owned())
            .unwrap_or_default()
    }

    /// Builds the Connection value from the form. Not reached for
    /// unsupported kinds (their fields are hidden; Save keeps the value).
    fn build_connection(&self, cx: &App) -> Result<Connection> {
        let url_or_path = self.text(0, cx);
        Ok(match self.conn_type {
            ConnType::Postgres => {
                if url_or_path.is_empty() {
                    bail!("postgres needs a url — reference credentials like ${{PG_PROD_URL}}");
                }
                Connection::Postgres(DbConn {
                    url: url_or_path,
                    extra: Default::default(),
                })
            }
            ConnType::Mysql => {
                if url_or_path.is_empty() {
                    bail!("mysql needs a url — reference credentials like ${{MYSQL_URL}}");
                }
                Connection::Mysql(DbConn {
                    url: url_or_path,
                    extra: Default::default(),
                })
            }
            ConnType::Duckdb => {
                if url_or_path.is_empty() {
                    bail!("duckdb needs a file path");
                }
                Connection::Duckdb(DuckdbConn {
                    path: url_or_path,
                    extra: Default::default(),
                })
            }
            ConnType::Local => Connection::Local {
                extra: Default::default(),
            },
            ConnType::Snowflake => {
                let account = self.text(1, cx);
                let user = self.text(2, cx);
                let secret = self.text(6, cx);
                if account.is_empty() || user.is_empty() || secret.is_empty() {
                    bail!(
                        "snowflake needs account, user and {} — reference env variables \
                         like ${{SNOWFLAKE_ACCOUNT}}",
                        if self.auth_password {
                            "a password reference"
                        } else {
                            "a private-key path"
                        }
                    );
                }
                let auth = if self.auth_password {
                    if !secret.starts_with("${") {
                        bail!(
                            "store the password in the environment and reference it \
                             like ${{SNOWFLAKE_PASSWORD}} — never a literal in YAML"
                        );
                    }
                    SnowflakeAuth::Password { password: secret }
                } else {
                    SnowflakeAuth::KeyPair {
                        private_key_path: secret,
                    }
                };
                let optional = |text: String| (!text.is_empty()).then_some(text);
                Connection::Snowflake(SnowflakeConn {
                    account,
                    user,
                    role: optional(self.text(3, cx)),
                    warehouse: optional(self.text(4, cx)),
                    database: optional(self.text(5, cx)),
                    auth,
                    extra: Default::default(),
                })
            }
        })
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing || self.broken {
            return;
        }
        let new_name = self.name.read(cx).text(cx).trim().to_owned();
        if new_name.is_empty() {
            return self.fail("the connection needs a name".into(), cx);
        }

        // Everything below reads the disk fresh at click time — the modal
        // may have been open a while.
        let mut connections = match load_connections_strict(&self.root) {
            Ok(connections) => connections,
            Err(error) => return self.fail(format!("{error:#}"), cx),
        };

        let value = if self.form_supported() {
            match self.build_connection(cx) {
                Ok(mut value) => {
                    if let Some(stored) = self
                        .editing
                        .as_ref()
                        .and_then(|name| connections.connections.get(name))
                    {
                        carry_extra(&mut value, stored);
                    }
                    value
                }
                Err(error) => return self.fail(format!("{error:#}"), cx),
            }
        } else {
            // Unsupported kind: rename-only save keeps the value intact.
            match self
                .editing
                .as_ref()
                .and_then(|name| connections.connections.get(name).cloned())
            {
                Some(value) => value,
                None => return self.fail("the connection is gone from connections.yml".into(), cx),
            }
        };

        let mut renamed_from = None;
        match &self.editing {
            None => {
                if connections.connections.contains_key(&new_name) {
                    return self.fail(
                        format!("a connection named {new_name:?} already exists"),
                        cx,
                    );
                }
                connections.connections.insert(new_name.clone(), value);
            }
            Some(original) => {
                if new_name != *original && connections.connections.contains_key(&new_name) {
                    return self.fail(
                        format!("a connection named {new_name:?} already exists"),
                        cx,
                    );
                }
                if !connections.connections.contains_key(original) {
                    return self.fail("the connection is gone from connections.yml".into(), cx);
                }
                // Replace in place, preserving the file's ordering.
                connections.connections = connections
                    .connections
                    .iter()
                    .map(|(name, existing)| {
                        if name == original {
                            (new_name.clone(), value.clone())
                        } else {
                            (name.clone(), existing.clone())
                        }
                    })
                    .collect();
                if new_name != *original {
                    renamed_from = Some(original.clone());
                }
            }
        }

        // A rename carries every referencing pipeline along with it. An
        // unreadable pipeline file aborts the save — it might reference
        // the old name.
        let mut pipeline_updates: Vec<(PathBuf, Pipeline)> = Vec::new();
        if let Some(old_name) = &renamed_from {
            for path in el_engine::spec::list_pipelines(&super::el_dir(&self.root)) {
                let mut pipeline = match el_engine::spec::load_pipeline(&path) {
                    Ok(pipeline) => pipeline,
                    Err(error) => {
                        return self.fail(
                            format!(
                                "{} could not be read ({error}) — fix it, then rename",
                                path.file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("a pipeline file")
                            ),
                            cx,
                        );
                    }
                };
                let mut touched = false;
                if pipeline.source == *old_name {
                    pipeline.source = new_name.clone();
                    touched = true;
                }
                if pipeline.target.connection == *old_name {
                    pipeline.target.connection = new_name.clone();
                    touched = true;
                }
                if touched {
                    pipeline_updates.push((path, pipeline));
                }
            }
        }

        let workspace = self.workspace.clone();
        let panel = self.panel.clone();
        let project = self.project.clone();
        let connections_path = super::el_dir(&self.root).join("connections.yml");
        let updated_pipelines = pipeline_updates.len();
        let editing = self.editing.is_some();
        self.writing = true;
        cx.notify();
        // Detached: dismissing the modal must never cancel a write that
        // has already started touching files.
        cx.spawn_in(window, async move |this, cx| {
            // Refuse before ANY write when a target buffer is dirty — a
            // rename either starts cleanly or not at all.
            let mut targets = vec![connections_path.clone()];
            targets.extend(pipeline_updates.iter().map(|(path, _)| path.clone()));
            let mut result = super::spec_io::check_clean(project.clone(), &targets, cx).await;

            if result.is_ok() {
                result = super::spec_io::write_text(
                    workspace.clone(),
                    project.clone(),
                    connections_path,
                    el_engine::spec::to_canonical_connections_yaml(&connections),
                    cx,
                )
                .await;
            }
            if result.is_ok() {
                for (path, pipeline) in pipeline_updates {
                    result = super::spec_io::write_spec(
                        workspace.clone(),
                        project.clone(),
                        path.clone(),
                        pipeline,
                        cx,
                    )
                    .await;
                    if let Err(error) = result {
                        result = Err(error.context(format!(
                            "connections.yml is updated, but {} still names the old \
                             connection",
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("a pipeline")
                        )));
                        break;
                    }
                }
            }

            let succeeded = result.is_ok();
            if succeeded {
                panel.update(cx, |panel, cx| panel.connections_changed(cx)).ok();
                let message = match (editing, renamed_from) {
                    (false, _) => format!("Connection {new_name} added."),
                    (true, Some(old)) if updated_pipelines > 0 => format!(
                        "Renamed {old} to {new_name} — {updated_pipelines} pipeline(s) \
                         updated."
                    ),
                    (true, Some(old)) => format!("Renamed {old} to {new_name}."),
                    (true, None) => format!("Connection {new_name} saved."),
                };
                workspace
                    .update(cx, |workspace, cx| super::toast(workspace, &message, cx))
                    .ok();
            }
            let delivered = this
                .update(cx, |this, cx| {
                    this.writing = false;
                    match &result {
                        Ok(()) => cx.emit(DismissEvent),
                        Err(error) => {
                            this.error = Some(format!("{error:#}").into());
                        }
                    }
                    cx.notify();
                })
                .is_ok();
            if !delivered {
                if let Err(error) = &result {
                    workspace
                        .update(cx, |workspace, cx| {
                            super::toast(
                                workspace,
                                &format!("Connection save failed: {error:#}"),
                                cx,
                            )
                        })
                        .ok();
                }
            }
        })
        .detach();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let Some(original) = self.editing.clone() else { return };
        // Re-scan at click time — the open-time snapshot may be stale.
        match referencing_pipelines(&self.root, &original) {
            Ok(referencing) if !referencing.is_empty() => {
                self.referencing = referencing.clone();
                return self.fail(
                    format!(
                        "still used by {} — point the pipeline(s) at another connection \
                         first",
                        referencing.join(", ")
                    ),
                    cx,
                );
            }
            Ok(_) => {}
            Err(error) => return self.fail(format!("{error:#}"), cx),
        }
        if !self.delete_armed {
            self.delete_armed = true;
            cx.notify();
            return;
        }

        let mut connections = match load_connections_strict(&self.root) {
            Ok(connections) => connections,
            Err(error) => return self.fail(format!("{error:#}"), cx),
        };
        if connections.connections.shift_remove(&original).is_none() {
            return self.fail("the connection is gone from connections.yml".into(), cx);
        }

        let workspace = self.workspace.clone();
        let panel = self.panel.clone();
        let project = self.project.clone();
        let connections_path = super::el_dir(&self.root).join("connections.yml");
        self.writing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = super::spec_io::write_text(
                workspace.clone(),
                project,
                connections_path,
                el_engine::spec::to_canonical_connections_yaml(&connections),
                cx,
            )
            .await;
            if result.is_ok() {
                panel.update(cx, |panel, cx| panel.connections_changed(cx)).ok();
                workspace
                    .update(cx, |workspace, cx| {
                        super::toast(workspace, &format!("Connection {original} deleted."), cx)
                    })
                    .ok();
            }
            let delivered = this
                .update(cx, |this, cx| {
                    this.writing = false;
                    this.delete_armed = false;
                    match &result {
                        Ok(()) => cx.emit(DismissEvent),
                        Err(error) => {
                            this.error = Some(format!("{error:#}").into());
                        }
                    }
                    cx.notify();
                })
                .is_ok();
            if !delivered {
                if let Err(error) = &result {
                    workspace
                        .update(cx, |workspace, cx| {
                            super::toast(
                                workspace,
                                &format!("Connection delete failed: {error:#}"),
                                cx,
                            )
                        })
                        .ok();
                }
            }
        })
        .detach();
    }

    fn open_yaml(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = super::el_dir(&self.root).join("connections.yml");
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                    .detach();
            })
            .ok();
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for ElConnectionModal {}

impl ModalView for ElConnectionModal {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        // Hold the modal while a write is in flight so its outcome (error
        // or dismissal) is always shown. The write itself is detached and
        // completes regardless.
        workspace::DismissDecision::Dismiss(!self.writing)
    }
}

impl Focusable for ElConnectionModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // The modal's focus is the name field, so typing lands somewhere
        // useful the moment it opens.
        self.name.read(cx).focus_handle(cx)
    }
}

impl Render for ElConnectionModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let editing = self.editing.is_some();
        let supported = self.form_supported();
        let title = if editing {
            "Edit connection"
        } else {
            "Add connection"
        };

        let mut card = v_flex()
            .key_context("ElConnectionModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(460.))
            .max_h(px(560.))
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
                        IconButton::new("el-conn-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            );

        if self.broken {
            // The file (or the connection) is unreadable: explain, offer
            // the YAML, change nothing.
            if let Some(error) = &self.error {
                card = card.child(div().p_2().child(
                    Label::new(error.clone()).size(LabelSize::Small).color(Color::Error),
                ));
            }
            return card.child(
                h_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        Button::new("el-conn-open-yaml", "Open connections.yml")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_yaml(window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("el-conn-cancel", "Cancel")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            );
        }

        if supported {
            let mut picker = h_flex().w_full().px_2().pt_2().gap_1().flex_wrap();
            for conn_type in ConnType::ALL {
                let conn_type = *conn_type;
                picker = picker.child(
                    Button::new(
                        SharedString::from(format!("el-conn-type-{}", conn_type.label())),
                        conn_type.label(),
                    )
                    .label_size(LabelSize::XSmall)
                    .toggle_state(self.conn_type == conn_type)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.conn_type = conn_type;
                        cx.notify();
                    })),
                );
            }
            card = card.child(picker);
        }

        let field_row = |label: &'static str, editor: Entity<Editor>| {
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(div().w(px(140.)).flex_shrink_0().child(
                    Label::new(label).size(LabelSize::XSmall).color(Color::Muted),
                ))
                .child(div().flex_1().child(editor))
        };

        let mut fields = v_flex().w_full().p_2().gap_1();
        fields = fields.child(field_row("name", self.name.clone()));
        if supported {
            match self.conn_type {
                ConnType::Postgres | ConnType::Mysql | ConnType::Duckdb => {
                    fields = fields
                        .child(field_row(self.fields[0].label, self.fields[0].editor.clone()));
                }
                ConnType::Local => {}
                ConnType::Snowflake => {
                    let auth = h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .child(div().w(px(140.)).flex_shrink_0().child(
                            Label::new("auth").size(LabelSize::XSmall).color(Color::Muted),
                        ))
                        .child(
                            Button::new("el-conn-auth-key", "key pair")
                                .label_size(LabelSize::XSmall)
                                .toggle_state(!self.auth_password)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_password = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("el-conn-auth-pw", "password")
                                .label_size(LabelSize::XSmall)
                                .toggle_state(self.auth_password)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_password = true;
                                    cx.notify();
                                })),
                        );
                    for field in &self.fields[1..6] {
                        fields = fields.child(field_row(field.label, field.editor.clone()));
                    }
                    fields = fields.child(auth).child(field_row(
                        if self.auth_password {
                            "password env"
                        } else {
                            "private key path"
                        },
                        self.fields[6].editor.clone(),
                    ));
                }
            }
        } else {
            let kind = self.original_kind.unwrap_or("this");
            fields = fields.child(
                Label::new(format!(
                    "{kind} connections are edited in YAML for now — the name can still \
                     be changed here."
                ))
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            );
        }
        card = card.child(fields);

        if editing {
            let note: SharedString = if self.referencing.is_empty() {
                "Used by no pipelines.".into()
            } else {
                format!("Used by: {}", self.referencing.join(", ")).into()
            };
            card = card.child(
                div().px_2().pb_1().child(
                    Label::new(note).size(LabelSize::XSmall).color(Color::Muted),
                ),
            );
        }

        if let Some(error) = &self.error {
            card = card.child(div().px_2().pb_1().child(
                Label::new(error.clone()).size(LabelSize::XSmall).color(Color::Error),
            ));
        }

        let mut footer = h_flex().w_full().p_2().gap_1().border_t_1().border_color(colors.border);
        if editing {
            footer = footer.child(
                Button::new(
                    "el-conn-delete",
                    if self.delete_armed {
                        "Confirm delete"
                    } else {
                        "Delete"
                    },
                )
                .label_size(LabelSize::Small)
                .color(Color::Error)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            );
        }
        if !supported {
            footer = footer.child(
                Button::new("el-conn-open-yaml", "Open connections.yml")
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| this.open_yaml(window, cx))),
            );
        }
        footer = footer
            .child(div().flex_1())
            .child(
                Button::new("el-conn-cancel", "Cancel")
                    .label_size(LabelSize::Small)
                    .disabled(self.writing)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
            )
            .child(
                Button::new(
                    "el-conn-save",
                    if self.writing {
                        "Saving…"
                    } else if editing {
                        "Save changes"
                    } else {
                        "Add connection"
                    },
                )
                .label_size(LabelSize::Small)
                .style(ButtonStyle::Filled)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            );
        card.child(footer)
    }
}
