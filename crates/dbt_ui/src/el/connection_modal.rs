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
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle,
    SharedString, WeakEntity, Window,
};
use indexmap::IndexMap;
use project::Project;
use ui::{Tooltip, WithScrollbar as _, prelude::*};
use workspace::{ModalView, Workspace};

use el_engine::spec::{
    Connection, Connections, DbConn, DuckdbConn, MssqlConn, OracleConn, OracleDriver, Pipeline,
    SnowflakeAuth, SnowflakeConn, SpecError,
};

use super::builder::ConnType;
use super::dialog;
use super::panel::ElPanel;

struct Field {
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
    /// Where the connection lives: None = base `connections:`, Some =
    /// inside that profile's block. Chosen at open for edits; a chip
    /// picker while adding.
    scope: Option<String>,
    /// Profiles declared in the file (for the add-mode scope chips).
    declared_profiles: Vec<String>,
    /// The workspace whose worker answers this connection — the YAML
    /// `workspace:` key. None is the local worker: the default, and what
    /// an absent key means. Named `runs_on` here because `workspace` is
    /// already Zed's Workspace above.
    runs_on: Option<String>,
    /// Remotes from remotes.yml, offered beside the local worker.
    declared_remotes: Vec<String>,
    /// The original kind string — drives the unsupported-kind fallback.
    original_kind: Option<&'static str>,
    /// Set when the open-time load failed or the connection is missing —
    /// the form becomes read-only (Open YAML is the way forward).
    broken: bool,
    conn_type: ConnType,
    /// Snowflake: password auth instead of key pair.
    auth_password: bool,
    /// SQL Server: accept the server's certificate without validating it.
    mssql_trust_cert: bool,
    /// The optional settings are unfolded — from the start when one of
    /// them already holds a value.
    show_optional: bool,
    name: Entity<Editor>,
    fields: Vec<Field>,
    /// Pipelines referencing this connection at open time (display only —
    /// save and delete re-scan the disk at click time).
    referencing: Vec<String>,
    delete_armed: bool,
    /// A write is in flight: buttons disabled, dismissal held.
    writing: bool,
    body_scroll: ScrollHandle,
    error: Option<SharedString>,
}

/// Field indices of the settings most connections leave unset: Snowflake's
/// role, warehouse and database; Oracle's schema, wallet and TNS admin;
/// SQL Server's port.
const OPTIONAL_FIELDS: [usize; 7] = [3, 4, 5, 10, 11, 12, 15];
const ORACLE_DRIVER_FIELD: usize = 13;
/// SQL Server's fields, in the order the form shows them.
const MSSQL_HOST_FIELD: usize = 14;
const MSSQL_PORT_FIELD: usize = 15;
const MSSQL_DATABASE_FIELD: usize = 16;
const MSSQL_USER_FIELD: usize = 17;
const MSSQL_PASSWORD_FIELD: usize = 18;

/// True for kinds the form can round-trip; others fall back to YAML.
fn form_supported(kind: &str) -> bool {
    matches!(
        kind,
        "postgres" | "mysql" | "oracle" | "mssql" | "duckdb" | "snowflake" | "local"
    )
}

fn conn_type_of(kind: &str) -> ConnType {
    match kind {
        "postgres" => ConnType::Postgres,
        "mysql" => ConnType::Mysql,
        "oracle" => ConnType::Oracle,
        "mssql" => ConnType::Mssql,
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
        (Connection::Oracle(new), Connection::Oracle(old)) => new.extra = old.extra.clone(),
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
        let mut scope: Option<String> = None;
        let mut declared_profiles: Vec<String> = Vec::new();
        let raw = match load_connections_strict(&root) {
            Ok(connections) => {
                declared_profiles = connections.profiles.keys().cloned().collect();
                Some(connections)
            }
            Err(error) => {
                open_error = Some(format!("{error:#}").into());
                broken = true;
                None
            }
        };
        let existing = editing.as_ref().and_then(|name| {
            let connections = raw.as_ref()?;
            // Base first; else the ACTIVE profile if it defines the name;
            // else the first profile that does.
            if let Some(found) = connections.connections.get(name) {
                return Some(found.clone());
            }
            let active = el_engine::spec::active_profile(&root, connections);
            let candidates = active
                .iter()
                .cloned()
                .chain(connections.profiles.keys().cloned())
                .collect::<Vec<_>>();
            for profile in candidates {
                if let Some(found) = connections
                    .profiles
                    .get(&profile)
                    .and_then(|block| block.connections.get(name))
                {
                    scope = Some(profile);
                    return Some(found.clone());
                }
            }
            open_error = Some(format!("{name:?} is gone from connections.yml").into());
            broken = true;
            None
        });

        let mut make = |placeholder: &str, initial: &str| Field {
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
        let mut oracle_user = String::new();
        let mut oracle_password = String::new();
        let mut oracle_connect = String::new();
        let mut oracle_schema = String::new();
        let mut oracle_wallet = String::new();
        let mut oracle_tns = String::new();
        let mut oracle_driver = String::new();
        let mut mssql_host = String::new();
        let mut mssql_port = String::new();
        let mut mssql_database = String::new();
        let mut mssql_user = String::new();
        let mut mssql_password = String::new();
        let mut mssql_trust_cert = false;
        match &existing {
            Some(Connection::Postgres(conn)) | Some(Connection::Mysql(conn)) => {
                url_or_path = conn.url.clone();
            }
            Some(Connection::Duckdb(conn)) => url_or_path = conn.path.clone(),
            Some(Connection::Oracle(conn)) => {
                oracle_user = conn.user.clone();
                // A ${VAR} reference round-trips; a literal credential is
                // never echoed into the form.
                if conn.password.is_empty() || conn.password.starts_with("${") {
                    oracle_password = conn.password.clone();
                } else {
                    open_error = Some(
                        "the YAML holds a literal password — enter a ${VAR} \
                         reference here and move the value to .env"
                            .into(),
                    );
                }
                oracle_connect = conn.connect.clone();
                oracle_schema = conn.schema.clone().unwrap_or_default();
                oracle_wallet = conn.wallet_dir.clone().unwrap_or_default();
                oracle_tns = conn.tns_admin.clone().unwrap_or_default();
                oracle_driver = conn
                    .driver
                    .map(|driver| driver.as_str().to_owned())
                    .unwrap_or_default();
            }
            Some(Connection::Mssql(conn)) => {
                mssql_host = conn.host.clone();
                mssql_port = conn.port.map(|port| port.to_string()).unwrap_or_default();
                mssql_database = conn.database.clone();
                mssql_user = conn.user.clone();
                mssql_trust_cert = conn.trust_server_certificate.unwrap_or(false);
                // A ${VAR} reference round-trips; a literal credential is
                // never echoed into the form.
                if conn.password.is_empty() || conn.password.starts_with("${") {
                    mssql_password = conn.password.clone();
                } else {
                    open_error = Some(
                        "the YAML holds a literal password — enter a ${VAR} \
                         reference here and move the value to .env"
                            .into(),
                    );
                }
            }
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
            make("${PG_PROD_URL}  ·  or  el/data.duckdb", &url_or_path),
            make("${SNOWFLAKE_ACCOUNT}", &account),
            make("${SNOWFLAKE_USER}", &user),
            make("LOADER (optional)", &role),
            make("LOAD_WH (optional)", &warehouse),
            make("RAW (optional)", &database),
            make("${SNOWFLAKE_PK_PATH}", &secret),
            make("${ORACLE_USER}", &oracle_user),
            make("${ORACLE_PASSWORD}", &oracle_password),
            make("db.example.com:1521/ORCLPDB1", &oracle_connect),
            make("ERP (optional)", &oracle_schema),
            make("el/wallet (optional)", &oracle_wallet),
            make("el/tns (optional)", &oracle_tns),
            make("auto — thick for Oracle 10g/11g (optional)", &oracle_driver),
            make("sql.example.com", &mssql_host),
            make("1433 (optional)", &mssql_port),
            make("Sales", &mssql_database),
            make("${MSSQL_USER}", &mssql_user),
            make("${MSSQL_PASSWORD}", &mssql_password),
        ];
        let name = make("pg_prod", editing.as_deref().unwrap_or("")).editor;

        // The workspaces this connection can name: every declared
        // remote, plus the local worker, which needs no remotes.yml entry.
        let declared_remotes: Vec<String> =
            el_engine::spec::load_remotes(&super::el_dir(&root).join("remotes.yml"))
                .map(|remotes| remotes.remotes.keys().cloned().collect())
                .unwrap_or_default();

        let referencing = editing
            .as_ref()
            .and_then(|name| referencing_pipelines(&root, name).ok())
            .unwrap_or_default();

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            body_scroll: ScrollHandle::new(),
            workspace,
            panel,
            project,
            root,
            original_kind: existing.as_ref().map(Connection::kind),
            scope,
            declared_profiles,
            runs_on: existing
                .as_ref()
                .and_then(|connection| connection.workspace().map(str::to_owned)),
            declared_remotes,
            broken,
            conn_type: existing
                .as_ref()
                .map(|connection| conn_type_of(connection.kind()))
                .unwrap_or(ConnType::Duckdb),
            editing,
            auth_password,
            mssql_trust_cert,
            show_optional: false,
            name,
            fields,
            referencing,
            delete_armed: false,
            writing: false,
            error: open_error,
        };
        this.show_optional = OPTIONAL_FIELDS
            .iter()
            .any(|ix| !this.text(*ix, cx).is_empty());
        this
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
                    workspace: self.runs_on.clone(),
                    extra: Default::default(),
                })
            }
            ConnType::Mysql => {
                if url_or_path.is_empty() {
                    bail!("mysql needs a url — reference credentials like ${{MYSQL_URL}}");
                }
                Connection::Mysql(DbConn {
                    url: url_or_path,
                    workspace: self.runs_on.clone(),
                    extra: Default::default(),
                })
            }
            ConnType::Duckdb => {
                if url_or_path.is_empty() {
                    bail!("duckdb needs a file path");
                }
                Connection::Duckdb(DuckdbConn {
                    path: url_or_path,
                    workspace: self.runs_on.clone(),
                    extra: Default::default(),
                })
            }
            ConnType::Oracle => {
                let user = self.text(7, cx);
                let password = self.text(8, cx);
                let connect = self.text(9, cx);
                if user.is_empty() || password.is_empty() || connect.is_empty() {
                    bail!(
                        "oracle needs user, password and connect — the connect string \
                         is host:port/service_name or a TNS alias"
                    );
                }
                if !password.starts_with("${") {
                    bail!(
                        "store the password in the environment and reference it like \
                         ${{ORACLE_PASSWORD}} — never a literal in YAML"
                    );
                }
                let optional = |text: String| (!text.is_empty()).then_some(text);
                let driver_text = self.text(13, cx);
                let driver = match OracleDriver::parse(&driver_text) {
                    Some(OracleDriver::Auto) => None,
                    Some(driver) => Some(driver),
                    None => bail!("driver must be auto, thin or thick"),
                };
                Connection::Oracle(OracleConn {
                    user,
                    password,
                    connect,
                    schema: optional(self.text(10, cx)),
                    wallet_dir: optional(self.text(11, cx)),
                    tns_admin: optional(self.text(12, cx)),
                    driver,
                    workspace: self.runs_on.clone(),
                    extra: Default::default(),
                })
            }
            ConnType::Mssql => {
                let host = self.text(MSSQL_HOST_FIELD, cx);
                let database = self.text(MSSQL_DATABASE_FIELD, cx);
                let user = self.text(MSSQL_USER_FIELD, cx);
                let password = self.text(MSSQL_PASSWORD_FIELD, cx);
                if host.is_empty() || database.is_empty() || user.is_empty() {
                    bail!("sql server needs a host, a database and a user");
                }
                if password.is_empty() {
                    bail!("sql server needs a password — reference it like ${{MSSQL_PASSWORD}}");
                }
                if !password.starts_with("${") {
                    bail!(
                        "store the password in the environment and reference it like \
                         ${{MSSQL_PASSWORD}} — never a literal in YAML"
                    );
                }
                let port = match self.text(MSSQL_PORT_FIELD, cx) {
                    port if port.is_empty() => None,
                    port => Some(
                        port.parse::<u16>()
                            .map_err(|_| anyhow::anyhow!("{port:?} is not a port number"))?,
                    ),
                };
                Connection::Mssql(MssqlConn {
                    host,
                    port,
                    database,
                    user,
                    password,
                    encrypt: None,
                    trust_server_certificate: self.mssql_trust_cert.then_some(true),
                    workspace: self.runs_on.clone(),
                    extra: Default::default(),
                })
            }
            ConnType::Local => Connection::Local {
                // `local` keeps every key in `extra`, so its workspace goes
                // there too — Connection::workspace() reads it back out.
                extra: self
                    .runs_on
                    .iter()
                    .map(|workspace| {
                        (
                            "workspace".to_owned(),
                            serde_yaml_ng::Value::String(workspace.clone()),
                        )
                    })
                    .collect(),
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
                    workspace: self.runs_on.clone(),
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

        // The map this edit lives in: base, or one profile's block.
        let scope = self.scope.clone();
        if let Some(profile) = &scope {
            if !connections.profiles.contains_key(profile) {
                return self.fail(
                    format!("profile {profile:?} is gone from connections.yml"),
                    cx,
                );
            }
        }
        let scope_map = |connections: &Connections| -> IndexMap<String, Connection> {
            match &scope {
                None => connections.connections.clone(),
                Some(profile) => connections
                    .profiles
                    .get(profile)
                    .map(|block| block.connections.clone())
                    .unwrap_or_default(),
            }
        };
        let value = if self.form_supported() {
            match self.build_connection(cx) {
                Ok(mut value) => {
                    if let Some(stored) = self
                        .editing
                        .as_ref()
                        .and_then(|name| scope_map(&connections).get(name).cloned())
                    {
                        carry_extra(&mut value, &stored);
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
                .and_then(|name| scope_map(&connections).get(name).cloned())
            {
                Some(value) => value,
                None => return self.fail("the connection is gone from connections.yml".into(), cx),
            }
        };

        // A name exists if the base OR any profile defines it — profiles
        // override by name, so a collision anywhere is a collision.
        let name_taken = |connections: &Connections, name: &str| {
            connections.connections.contains_key(name)
                || connections
                    .profiles
                    .values()
                    .any(|profile| profile.connections.contains_key(name))
        };
        let mut renamed_from = None;
        // In-place replacement inside ONE map, preserving order.
        let replace_in = |map: &mut IndexMap<String, Connection>,
                          original: &str,
                          new_name: &str,
                          value: &Connection| {
            *map = map
                .iter()
                .map(|(name, existing)| {
                    if name == original {
                        (new_name.to_owned(), value.clone())
                    } else {
                        (name.clone(), existing.clone())
                    }
                })
                .collect();
        };
        match &self.editing {
            None => {
                if name_taken(&connections, &new_name) {
                    return self.fail(
                        format!("a connection named {new_name:?} already exists"),
                        cx,
                    );
                }
                match &scope {
                    None => {
                        connections.connections.insert(new_name.clone(), value);
                    }
                    Some(profile) => {
                        if let Some(block) = connections.profiles.get_mut(profile) {
                            block.connections.insert(new_name.clone(), value);
                        }
                    }
                }
            }
            Some(original) => {
                if new_name != *original && name_taken(&connections, &new_name) {
                    return self.fail(
                        format!("a connection named {new_name:?} already exists"),
                        cx,
                    );
                }
                if !scope_map(&connections).contains_key(original) {
                    return self.fail("the connection is gone from connections.yml".into(), cx);
                }
                match &scope {
                    None => replace_in(&mut connections.connections, original, &new_name, &value),
                    Some(profile) => {
                        if let Some(block) = connections.profiles.get_mut(profile) {
                            replace_in(&mut block.connections, original, &new_name, &value);
                        }
                    }
                }
                if new_name != *original {
                    // The logical name changes everywhere it is defined.
                    if scope.is_some() && connections.connections.contains_key(original) {
                        let stored = connections.connections[original].clone();
                        replace_in(&mut connections.connections, original, &new_name, &stored);
                    }
                    // Profile overrides ride along with the rename — a
                    // detached override would silently resolve the new
                    // name to the wrong environment's base connection.
                    for (profile_name, profile) in connections.profiles.iter_mut() {
                        if scope.as_deref() == Some(profile_name.as_str()) {
                            continue; // already replaced above
                        }
                        if let Some(stored) = profile.connections.get(original).cloned() {
                            replace_in(&mut profile.connections, original, &new_name, &stored);
                        }
                    }
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
                panel
                    .update(cx, |panel, cx| panel.connections_changed(cx))
                    .ok();
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
        let Some(original) = self.editing.clone() else {
            return;
        };
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
        let in_base = connections.connections.shift_remove(&original).is_some();
        let mut in_profiles = false;
        for profile in connections.profiles.values_mut() {
            in_profiles |= profile.connections.shift_remove(&original).is_some();
        }
        if !in_base && !in_profiles {
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
                panel
                    .update(cx, |panel, cx| panel.connections_changed(cx))
                    .ok();
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

impl ElConnectionModal {
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.writing {
            cx.emit(DismissEvent);
        }
    }

    /// The editors on screen, top to bottom: the order Tab walks.
    fn visible_editors(&self) -> Vec<Entity<Editor>> {
        let mut editors = vec![self.name.clone()];
        if self.broken || !self.form_supported() {
            return editors;
        }
        let shown: &[usize] = match (self.conn_type, self.show_optional) {
            (ConnType::Postgres | ConnType::Mysql | ConnType::Duckdb, _) => &[0],
            (ConnType::Oracle, false) => &[7, 8, 9],
            (ConnType::Oracle, true) => &[7, 8, 9, 10, 11, 12],
            (ConnType::Mssql, false) => &[14, 16, 17, 18],
            (ConnType::Mssql, true) => &[14, 16, 17, 18, 15],
            (ConnType::Snowflake, false) => &[1, 2, 6],
            (ConnType::Snowflake, true) => &[1, 2, 6, 3, 4, 5],
            (ConnType::Local, _) => &[],
        };
        editors.extend(
            shown
                .iter()
                .filter_map(|ix| self.fields.get(*ix))
                .map(|field| field.editor.clone()),
        );
        editors
    }

    fn step_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let fields: Vec<FocusHandle> = self
            .visible_editors()
            .iter()
            .map(|editor| editor.focus_handle(cx))
            .collect();
        dialog::cycle_focus(&fields, forward, window, cx);
    }

    fn field_at(
        &self,
        ix: usize,
        label: &'static str,
        hint: Option<&'static str>,
        window: &Window,
        cx: &App,
    ) -> Option<AnyElement> {
        self.fields.get(ix).map(|field| {
            dialog::field(
                label,
                &field.editor,
                hint.map(SharedString::from),
                window,
                cx,
            )
        })
    }
}

impl Render for ElConnectionModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.editing.is_some();
        let supported = self.form_supported();
        let focus = self.focus_handle(cx);
        let detail: SharedString = match (&self.editing, &self.scope) {
            (Some(name), Some(profile)) => format!("{name} · defined in profile {profile}").into(),
            (Some(name), None) => format!("{name} · shared by every profile").into(),
            (None, _) => "Saved to el/connections.yml; secrets stay in .env as ${VAR}.".into(),
        };
        let frame = dialog::frame(dialog::WIDE, cx)
            .key_context("ElConnectionModal")
            .track_focus(&self.focus_handle)
            .max_h(px(680.))
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                if this.broken {
                    this.open_yaml(window, cx);
                } else {
                    this.save(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &menu::SelectNext, window, cx| {
                this.step_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                this.step_focus(false, window, cx)
            }))
            .child(dialog::header(
                if editing {
                    "Edit connection"
                } else {
                    "Add connection"
                },
                Some(detail),
                cx.listener(|this, _, _, cx| this.cancel(cx)),
                cx,
            ));

        if self.broken {
            // The file (or the connection) is unreadable: explain, offer
            // the YAML, change nothing.
            return frame
                .child(
                    dialog::body().children(
                        self.error
                            .clone()
                            .map(|error| dialog::notice(Severity::Error, error)),
                    ),
                )
                .child(dialog::footer(
                    [],
                    [
                        dialog::cancel_button("el-conn-cancel", &focus, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                            .into_any_element(),
                        dialog::primary_button(
                            "el-conn-open-yaml",
                            "Open connections.yml",
                            &focus,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.open_yaml(window, cx)))
                        .into_any_element(),
                    ],
                    cx,
                ));
        }

        let mut body = dialog::body();
        if supported {
            let chips = ConnType::ALL.iter().map(|conn_type| {
                let conn_type = *conn_type;
                dialog::chip(
                    SharedString::from(format!("el-conn-type-{}", conn_type.label())),
                    conn_type.label(),
                    self.conn_type == conn_type,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.conn_type = conn_type;
                    cx.notify();
                }))
            });
            body = body.child(dialog::labeled("Type", dialog::chips(chips)));
        }
        body = body.child(dialog::field("Name", &self.name, None, window, cx));

        let optional_toggle = |open: bool, cx: &mut Context<Self>| {
            dialog::labeled(
                "",
                Button::new(
                    "el-conn-optional",
                    if open {
                        "Hide optional settings"
                    } else {
                        "Optional settings"
                    },
                )
                .label_size(LabelSize::Small)
                .start_icon(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(IconSize::XSmall),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_optional = !this.show_optional;
                    cx.notify();
                })),
            )
        };
        if supported {
            let secret_hint = Some("A ${VAR} reference — the value itself stays in .env.");
            match self.conn_type {
                ConnType::Postgres | ConnType::Mysql => {
                    body = body.children(self.field_at(
                        0,
                        "URL",
                        Some("A ${VAR} reference keeps the credentials in .env."),
                        window,
                        cx,
                    ));
                }
                ConnType::Duckdb => {
                    body = body.children(self.field_at(
                        0,
                        "Database file",
                        Some("The .duckdb file, relative to the project root."),
                        window,
                        cx,
                    ));
                }
                ConnType::Oracle => {
                    let driver = OracleDriver::parse(&self.text(ORACLE_DRIVER_FIELD, cx));
                    let driver_chips = [
                        ("auto", OracleDriver::Auto),
                        ("thin", OracleDriver::Thin),
                        ("thick", OracleDriver::Thick),
                    ]
                    .into_iter()
                    .map(|(label, choice)| {
                        dialog::chip(
                            SharedString::from(format!("el-conn-driver-{label}")),
                            label,
                            driver == Some(choice),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if let Some(field) = this.fields.get(ORACLE_DRIVER_FIELD) {
                                    // auto is what an absent key means: write nothing.
                                    let text = if choice == OracleDriver::Auto {
                                        ""
                                    } else {
                                        label
                                    };
                                    field
                                        .editor
                                        .update(cx, |editor, cx| editor.set_text(text, window, cx));
                                }
                                cx.notify();
                            },
                        ))
                    });
                    let driver_hint = match driver {
                        Some(OracleDriver::Thin) => "No client software; Oracle 12.1 and later.",
                        Some(OracleDriver::Thick) => {
                            "Oracle Instant Client; reaches 10g and 11g too."
                        }
                        _ => "Thin first; a server before 12.1 falls back to the thick client.",
                    };
                    body = body
                        .children(self.field_at(7, "User", None, window, cx))
                        .children(self.field_at(8, "Password", secret_hint, window, cx))
                        .children(self.field_at(
                            9,
                            "Connect",
                            Some("host:port/service_name, or a TNS alias."),
                            window,
                            cx,
                        ))
                        .child(dialog::labeled(
                            "Driver",
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(dialog::chips(driver_chips))
                                .child(
                                    Label::new(driver_hint)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        ))
                        .child(optional_toggle(self.show_optional, cx));
                    if self.show_optional {
                        body = body
                            .children(self.field_at(
                                10,
                                "Schema",
                                Some("Defaults to the user's own schema."),
                                window,
                                cx,
                            ))
                            .children(self.field_at(11, "Wallet directory", None, window, cx))
                            .children(self.field_at(12, "TNS admin", None, window, cx));
                    }
                }
                ConnType::Mssql => {
                    body = body
                        .children(self.field_at(
                            MSSQL_HOST_FIELD,
                            "Host",
                            Some("The server's name or address, without the port."),
                            window,
                            cx,
                        ))
                        .children(self.field_at(MSSQL_DATABASE_FIELD, "Database", None, window, cx))
                        .children(self.field_at(MSSQL_USER_FIELD, "User", None, window, cx))
                        .children(self.field_at(
                            MSSQL_PASSWORD_FIELD,
                            "Password",
                            secret_hint,
                            window,
                            cx,
                        ))
                        .child(optional_toggle(self.show_optional, cx));
                    if self.show_optional {
                        let trust = self.mssql_trust_cert;
                        body = body
                            .children(self.field_at(
                                MSSQL_PORT_FIELD,
                                "Port",
                                Some("1433 unless this says otherwise."),
                                window,
                                cx,
                            ))
                            .child(dialog::labeled(
                                "Certificate",
                                v_flex()
                                    .w_full()
                                    .gap_1()
                                    .child(dialog::chips(
                                        [("validate", false), ("trust the server's own", true)]
                                            .into_iter()
                                            .map(|(label, choice)| {
                                                dialog::chip(
                                                    SharedString::from(format!(
                                                        "el-conn-cert-{label}"
                                                    )),
                                                    label,
                                                    trust == choice,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.mssql_trust_cert = choice;
                                                    cx.notify();
                                                }))
                                            }),
                                    ))
                                    .child(
                                        Label::new(if trust {
                                            "The connection is still encrypted; the \
                                             certificate is simply not checked."
                                        } else {
                                            "The server's certificate must be one this \
                                             machine trusts."
                                        })
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                    ),
                            ));
                    }
                }
                ConnType::Local => {
                    body = body.child(dialog::notice(
                        Severity::Info,
                        "Local files need no settings — each stream names its own path.",
                    ));
                }
                ConnType::Snowflake => {
                    let auth_chips =
                        [
                            dialog::chip("el-conn-auth-key", "key pair", !self.auth_password)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_password = false;
                                    cx.notify();
                                })),
                            dialog::chip("el-conn-auth-pw", "password", self.auth_password)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_password = true;
                                    cx.notify();
                                })),
                        ];
                    body = body
                        .children(self.field_at(1, "Account", None, window, cx))
                        .children(self.field_at(2, "User", None, window, cx))
                        .child(dialog::labeled("Auth", dialog::chips(auth_chips)))
                        .children(self.field_at(
                            6,
                            if self.auth_password {
                                "Password"
                            } else {
                                "Private key path"
                            },
                            secret_hint,
                            window,
                            cx,
                        ))
                        .child(optional_toggle(self.show_optional, cx));
                    if self.show_optional {
                        body = body
                            .children(self.field_at(3, "Role", None, window, cx))
                            .children(self.field_at(4, "Warehouse", None, window, cx))
                            .children(self.field_at(5, "Database", None, window, cx));
                    }
                }
            }
        } else {
            let kind = self.original_kind.unwrap_or("this");
            body = body.child(dialog::notice(
                Severity::Info,
                format!(
                    "{kind} connections are edited in YAML for now — the name can still be \
                     changed here."
                ),
            ));
        }

        // Which workspace answers this connection's tables, queries and
        // previews. `local` is the default: it writes no key at all, so a
        // connection belongs to this machine until it says otherwise.
        let mut runs_on = vec![
            dialog::chip("el-conn-workspace-local", "local", self.runs_on.is_none()).on_click(
                cx.listener(|this, _, _, cx| {
                    this.runs_on = None;
                    cx.notify();
                }),
            ),
        ];
        for (ix, remote) in self.declared_remotes.iter().enumerate() {
            let selected = self.runs_on.as_deref() == Some(remote);
            let remote = remote.clone();
            runs_on.push(
                dialog::chip(
                    ("el-conn-workspace", ix),
                    SharedString::from(remote.clone()),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.runs_on = Some(remote.clone());
                    cx.notify();
                })),
            );
        }
        body = body.child(dialog::labeled(
            "Runs on",
            v_flex()
                .w_full()
                .gap_1()
                .child(dialog::chips(runs_on))
                .child(
                    Label::new("The worker that lists its tables, previews and runs pipelines.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
        ));

        if !editing && !self.declared_profiles.is_empty() {
            // Where the new connection goes: shared base, or one profile.
            let mut scopes = vec![
                dialog::chip("el-conn-scope-base", "every profile", self.scope.is_none()).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.scope = None;
                        cx.notify();
                    }),
                ),
            ];
            for (ix, profile) in self.declared_profiles.iter().enumerate() {
                let selected = self.scope.as_deref() == Some(profile);
                let profile = profile.clone();
                scopes.push(
                    dialog::chip(
                        ("el-conn-scope", ix),
                        SharedString::from(profile.clone()),
                        selected,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.scope = Some(profile.clone());
                        cx.notify();
                    })),
                );
            }
            body = body.child(dialog::labeled("Defined in", dialog::chips(scopes)));
        }

        if editing {
            body = body.child(dialog::notice(
                Severity::Info,
                if self.referencing.is_empty() {
                    "No pipeline uses this connection yet.".to_owned()
                } else {
                    format!("Used by {}.", self.referencing.join(", "))
                },
            ));
        }
        if let Some(error) = &self.error {
            body = body.child(dialog::notice(Severity::Error, error.clone()));
        }

        let referenced = !self.referencing.is_empty();
        let delete = editing.then(|| {
            dialog::danger_button(
                "el-conn-delete",
                if self.delete_armed {
                    "Confirm delete"
                } else {
                    "Delete…"
                },
            )
            .disabled(self.writing || referenced)
            .when(referenced, |button| {
                button.tooltip(Tooltip::text(
                    "Pipelines still use this connection — point them elsewhere first",
                ))
            })
            .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx)))
            .into_any_element()
        });
        let open_yaml = (!supported).then(|| {
            Button::new("el-conn-open-yaml", "Open connections.yml")
                .label_size(LabelSize::Small)
                .on_click(cx.listener(|this, _, window, cx| this.open_yaml(window, cx)))
                .into_any_element()
        });
        frame
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("el-conn-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.body_scroll)
                            .child(body),
                    )
                    .custom_scrollbars(super::scrollbar(&self.body_scroll), window, cx),
            )
            .child(dialog::footer(
                delete.into_iter().chain(open_yaml),
                [
                    dialog::cancel_button("el-conn-cancel", &focus, cx)
                        .disabled(self.writing)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                        .into_any_element(),
                    dialog::primary_button(
                        "el-conn-save",
                        if self.writing {
                            "Saving…"
                        } else if editing {
                            "Save changes"
                        } else {
                            "Add connection"
                        },
                        &focus,
                        cx,
                    )
                    .disabled(self.writing)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))
                    .into_any_element(),
                ],
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle(user: &str) -> Connection {
        Connection::Oracle(OracleConn {
            user: user.to_owned(),
            password: "${ORACLE_PASSWORD}".to_owned(),
            connect: "db.example.com:1521/ORCLPDB1".to_owned(),
            schema: Some("ERP".to_owned()),
            wallet_dir: None,
            tns_admin: None,
            driver: None,
            workspace: None,
            extra: IndexMap::new(),
        })
    }

    /// Every kind the form claims to support maps to a ConnType, and back
    /// to the same kind — a kind that fell through to the default would
    /// silently rewrite the connection as duckdb on save.
    #[test]
    fn supported_kinds_round_trip_through_conn_type() {
        for kind in ["postgres", "mysql", "oracle", "duckdb", "snowflake"] {
            assert!(form_supported(kind), "{kind} is not form-supported");
            assert_eq!(conn_type_of(kind).label(), kind);
        }
        // These two spell their kind differently from their label.
        assert!(form_supported("local"));
        assert!(conn_type_of("local") == ConnType::Local);
        assert!(form_supported("mssql"));
        assert!(conn_type_of("mssql") == ConnType::Mssql);
        // Kinds without a form still fall back to the YAML rename path.
        assert!(!form_supported("bigquery"));
    }

    #[test]
    fn oracle_extras_survive_an_edit() {
        let mut stored = oracle("ERP_READER");
        if let Connection::Oracle(conn) = &mut stored {
            conn.extra.insert(
                "pool_max".to_owned(),
                serde_yaml_ng::Value::Number(8.into()),
            );
        }
        let mut rebuilt = oracle("ERP_READER");
        carry_extra(&mut rebuilt, &stored);
        let Connection::Oracle(conn) = &rebuilt else {
            panic!("kind changed");
        };
        assert!(conn.extra.contains_key("pool_max"));
    }

    /// What the form builds is what the file holds: the canonical writer
    /// keeps every Oracle field, and the password stays a ${VAR}.
    #[test]
    fn oracle_round_trips_through_the_canonical_writer() {
        let mut map = IndexMap::new();
        map.insert("ora_erp".to_owned(), oracle("ERP_READER"));
        let connections = Connections {
            version: 1,
            connections: map,
            profiles: IndexMap::new(),
            default_profile: None,
            extra: IndexMap::new(),
        };
        let yaml = el_engine::spec::to_canonical_connections_yaml(&connections);
        assert!(yaml.contains("type: oracle"), "{yaml}");
        assert!(
            yaml.contains("connect: db.example.com:1521/ORCLPDB1"),
            "{yaml}"
        );
        assert!(yaml.contains("${ORACLE_PASSWORD}"), "{yaml}");
        // Unset optionals are not written back as nulls.
        assert!(!yaml.contains("wallet_dir"), "{yaml}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.yml");
        std::fs::write(&path, &yaml).unwrap();
        let reloaded = el_engine::spec::load_connections(&path).unwrap();
        let stored = reloaded.connections.get("ora_erp").unwrap();
        assert_eq!(stored.kind(), "oracle");
        assert_eq!(
            el_engine::spec::to_canonical_connections_yaml(&reloaded),
            yaml
        );
        assert!(
            stored.shape_issues().is_empty(),
            "{:?}",
            stored.shape_issues()
        );
    }
}
