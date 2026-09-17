//! The EL panel: the extract-load plugin's own left-dock surface —
//! pipelines explorer, connections at a glance (names and kinds only,
//! never values), New pipeline, Initialize. Fully independent of the dbt
//! panels; works in standalone EL projects with no dbt_project.yml.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use editor::Editor;
use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
};
use ui::{HighlightedLabel, SpinnerLabel, Tooltip, WithScrollbar as _, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::local_runner::{self, Access, Container};
use crate::ToggleElPanelFocus;

pub struct ElPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    pipelines: Vec<PathBuf>,
    /// (name, kind, workspace) — the credential posture: never values.
    /// The workspace is where this connection's worker runs: a remote's
    /// name, or "local" when connections.yml declares none.
    connections: Vec<(SharedString, SharedString, SharedString)>,
    /// connections.yml failed to load (parse error — not absence).
    connections_error: Option<SharedString>,
    /// The active profile (dev/recette/prod…) and all declared ones.
    profile: Option<SharedString>,
    profiles: Vec<SharedString>,
    /// Declared remotes: (name, host shown muted — never the token).
    remotes: Vec<(SharedString, SharedString)>,
    /// Connection name → the `${VAR}` names it references that neither the
    /// environment nor .env provides. Names only, never values.
    missing_env: std::collections::HashMap<SharedString, Vec<String>>,
    /// Remote name → the token variable that is not set locally.
    remotes_missing_token: std::collections::HashMap<SharedString, String>,
    /// The built-in local runner: the licence check's answer, the
    /// container's state, and whatever start or stop is in flight.
    runner: RunnerState,
    /// Section keys the user folded ("pipelines", "connections", "remotes").
    collapsed: std::collections::HashSet<&'static str>,
    /// Connections whose table list is unfolded in the explorer.
    expanded: std::collections::HashSet<SharedString>,
    tables: std::collections::HashMap<SharedString, TablesState>,
    /// Each unfolded connection's table search and page, kept while folded
    /// so reopening one finds its list as it was left.
    browsers: std::collections::HashMap<SharedString, TableBrowser>,
    /// Bumped on profile/connection changes: in-flight table listings
    /// from the previous environment are dropped, not displayed.
    tables_epoch: u64,
    _list_tasks: std::collections::HashMap<SharedString, Task<()>>,
    scroll: UniformListScrollHandle,
    scroll_lower: UniformListScrollHandle,
    scroll_remotes: UniformListScrollHandle,
    /// Heights of the Pipelines and Connections sections (Remotes takes
    /// the rest); the two splitters drag them.
    split: f32,
    split_connections: f32,
    /// (which splitter, pointer y at drag start, split at drag start).
    split_drag: Option<(usize, f32, f32)>,
    _refresh: Task<()>,
}

const PROFILE_HINT: &str = "Which environment's connections this checkout uses; pipelines stay \
the same. Your choice is saved in el/.zdbt/profile for you only — connections.yml does not \
change.";

/// Tables per page under an unfolded connection.
const TABLES_PAGE: usize = 100;
/// Longer table lists get a search field; shorter ones read at a glance.
const TABLES_SEARCH_MIN: usize = 10;

enum TablesState {
    Loading,
    Loaded {
        tables: Vec<(String, String)>,
        /// Each table as lowercase `schema.table`, folded once for search.
        haystack: Vec<String>,
    },
    Failed(SharedString),
}

struct TableBrowser {
    search: Entity<Editor>,
    /// The search field's words, lowercased; a table must contain them all.
    terms: Vec<String>,
    /// Indices into the loaded tables that contain every term.
    matches: Vec<usize>,
    page: usize,
    _search_edits: Subscription,
}

/// The Remotes section's built-in `localhost` runner.
struct RunnerState {
    /// None until the first licence check answers.
    access: Option<Access>,
    /// None when Docker could not be asked.
    container: Option<Container>,
    /// "starting…" or "stopping…" while an action runs.
    busy: Option<&'static str>,
    error: Option<SharedString>,
    /// The project the last check ran for; another project checks again.
    checked_root: Option<PathBuf>,
    /// The server the runner answers as: one the project already declares at
    /// the runner's address, else `localhost`.
    remote_name: SharedString,
    _check: Task<()>,
    _action: Task<()>,
}

enum Row {
    /// Section headers: collapse chevron, label, and the "+" action.
    PipelinesHeader,
    ConnectionsHeader,
    RemotesHeader,
    Remote(SharedString, SharedString),
    Pipeline(PathBuf),
    Connection(SharedString, SharedString, SharedString),
    Table {
        connection: SharedString,
        schema: String,
        table: String,
    },
    ConnNote(SharedString, Color),
    /// A connection's tables are being listed; a 10g dictionary can take
    /// several seconds, so it spins rather than sits still.
    TablesLoading,
    /// The search field over a connection's tables.
    TableSearch(SharedString),
    /// After a page of tables: which ones out of how many, with page
    /// buttons when there is more than one page.
    TablePager {
        connection: SharedString,
        shown: Range<usize>,
        total: usize,
        searching: bool,
    },
    /// The built-in local runner (`localhost`), with its start/stop control.
    LocalRunner,
    /// A note about the local runner; the tooltip carries Docker's own words.
    RunnerNote(SharedString, Color, Option<SharedString>),
    /// "Set X in .env to connect" under a connection or server whose
    /// `${VAR}` references are unset — click opens .env.
    EnvHint(Vec<String>),
    /// The Remotes empty state: its tooltip says what a server is.
    RemotesEmpty,
    Initialize,
    Note(SharedString),
}

/// The pill that follows the cursor while a table is dragged.
struct DraggedTablePreview(SharedString);

impl gpui::Render for DraggedTablePreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.elevated_surface_background)
            .shadow_md()
            .child(
                Icon::new(IconName::Table)
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            )
            .child(Label::new(self.0.clone()).size(LabelSize::Small))
    }
}

impl ElPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            Self::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            workspace: workspace_handle,
            root: None,
            pipelines: Vec::new(),
            connections: Vec::new(),
            connections_error: None,
            profile: None,
            profiles: Vec::new(),
            remotes: Vec::new(),
            missing_env: Default::default(),
            remotes_missing_token: Default::default(),
            runner: RunnerState {
                access: None,
                container: None,
                busy: None,
                error: None,
                checked_root: None,
                remote_name: local_runner::REMOTE_NAME.into(),
                _check: Task::ready(()),
                _action: Task::ready(()),
            },
            collapsed: Default::default(),
            expanded: Default::default(),
            tables: Default::default(),
            browsers: Default::default(),
            tables_epoch: 0,
            _list_tasks: Default::default(),
            scroll: UniformListScrollHandle::new(),
            scroll_lower: UniformListScrollHandle::new(),
            scroll_remotes: UniformListScrollHandle::new(),
            split: 200.,
            split_connections: 220.,
            split_drag: None,
            _refresh: Task::ready(()),
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone().or_else(|| {
            self.workspace
                .upgrade()
                .and_then(|workspace| super::discover_el_root(workspace.read(cx), cx))
        });
        self.root = root.clone();
        let Some(root) = root else {
            return;
        };
        if self.runner.checked_root.as_ref() != Some(&root) {
            self.check_runner(&root, cx);
        }
        let el = super::el_dir(&root);
        // Self-heal the derived JSON schemas the YAML headers point at —
        // hand-made projects never ran Initialize.
        if let Err(error) = super::scaffold::ensure_schemas(&root) {
            log::warn!("el: could not write schemas: {error:#}");
        }
        self.pipelines = el_engine::spec::list_pipelines(&el);
        // Real env + project .env: only ever probed for a NAME's presence.
        let env = el_engine::env::EnvMap::load(&root, None);
        self.remotes_missing_token.clear();
        self.runner.remote_name = local_runner::runner_remote(&root)
            .map(|remote| remote.name.into())
            .unwrap_or_else(|_| local_runner::REMOTE_NAME.into());
        self.remotes = el_engine::spec::load_remotes(&el.join("remotes.yml"))
            .map(|remotes| {
                remotes
                    .remotes
                    .iter()
                    .map(|(name, remote)| {
                        // Host only: scheme stripped, path/query dropped.
                        let host = remote
                            .url
                            .split_once("://")
                            .map(|(_, rest)| rest)
                            .unwrap_or(&remote.url)
                            .split(['/', '?'])
                            .next()
                            .unwrap_or("")
                            .to_owned();
                        let name: SharedString = name.clone().into();
                        let mut refs = Vec::new();
                        if let Some(token) = &remote.token {
                            el_engine::env::collect_var_refs(token, &mut refs);
                        }
                        if let Some(var) = refs.into_iter().find(|var| !env.contains(var)) {
                            self.remotes_missing_token.insert(name.clone(), var);
                        }
                        (name, host.into())
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.missing_env.clear();
        match el_engine::spec::load_active_connections(&root) {
            Ok((connections, profile)) => {
                self.profile = profile.map(Into::into);
                self.profiles = el_engine::spec::load_connections(&el.join("connections.yml"))
                    .map(|raw| {
                        raw.profiles
                            .keys()
                            .map(|name| name.clone().into())
                            .collect()
                    })
                    .unwrap_or_default();
                self.connections = connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        let name: SharedString = name.clone().into();
                        let missing = connection.missing_env_refs(&env);
                        if !missing.is_empty() {
                            self.missing_env.insert(name.clone(), missing);
                        }
                        let workspace: SharedString =
                            connection.workspace().unwrap_or("local").to_owned().into();
                        (name, connection.kind().to_owned().into(), workspace)
                    })
                    .collect();
                self.connections_error = None;
            }
            Err(el_engine::spec::SpecError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                self.connections = Vec::new();
                self.connections_error = None;
            }
            Err(error) => {
                // A broken file is not an empty file — say so instead of
                // rendering a list that invites a destructive rewrite.
                self.connections = Vec::new();
                self.connections_error =
                    Some(format!("connections.yml could not be read: {error}").into());
            }
        }
        cx.notify();
    }

    /// Switches the checkout's active profile: writes the local selection
    /// file (never the shared YAML), drops caches, and reloads every open
    /// canvas so validation and labels track the new environment.
    fn switch_profile(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let path = el_engine::spec::profile_selection_path(&root);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, name.as_ref()) {
            self.connections_error =
                Some(format!("could not save the profile selection: {error}").into());
            cx.notify();
            return;
        }
        self.tables.clear();
        self.expanded.clear();
        self.browsers.clear();
        self.tables_epoch += 1;
        self.refresh(cx);
        // The selection file is one voice among three — say what actually
        // happened instead of assuming the write took effect.
        let effective = self.profile.clone();
        let message = if effective.as_ref() == Some(&name) {
            format!("Profile: {name}")
        } else if std::env::var("ZDBT_EL_PROFILE").is_ok() {
            format!(
                "Saved, but ZDBT_EL_PROFILE overrides the selection — this window                  still runs as {}.",
                effective.as_deref().unwrap_or("the base connections")
            )
        } else {
            format!(
                "Saved, but the active profile is {} — check connections.yml.",
                effective.as_deref().unwrap_or("none")
            )
        };
        let console = self.console(cx);
        self.workspace
            .update(cx, |workspace, cx| {
                let canvases: Vec<_> = workspace
                    .items_of_type::<super::ElPipelineCanvas>(cx)
                    .collect();
                for canvas in canvases {
                    canvas.update(cx, |canvas, cx| canvas.reload(cx));
                }
                if effective.as_ref() == Some(&name) {
                    super::toast(workspace, &message, cx);
                } else {
                    super::toast_error(workspace, &message, None, cx);
                }
            })
            .ok();
        // The EL console's Query state is from the old environment — reset
        // it, outside the workspace lease (it reads the workspace).
        if let Some(console) = console {
            console.update(cx, |panel, cx| panel.profile_changed(cx));
        }
    }

    /// True when the project has nothing EL yet — one list with the
    /// invitation to initialize.
    fn is_empty_project(&self) -> bool {
        self.pipelines.is_empty() && self.connections.is_empty() && self.remotes.is_empty()
    }

    /// The upper list: pipelines.
    fn upper_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.is_empty_project() {
            rows.push(Row::Note(
                "Extract-load pipelines, defined as YAML in el/.".into(),
            ));
            rows.push(Row::Initialize);
            return rows;
        }
        rows.push(Row::PipelinesHeader);
        if !self.collapsed.contains("pipelines") {
            for path in &self.pipelines {
                rows.push(Row::Pipeline(path.clone()));
            }
            if self.pipelines.is_empty() {
                rows.push(Row::ConnNote(
                    "No pipelines yet — press + to start one.".into(),
                    Color::Muted,
                ));
                if self
                    .connections
                    .iter()
                    .any(|(_, kind, _)| super::browsable(kind))
                {
                    rows.push(Row::ConnNote(
                        "Then drag a table from Connections onto it.".into(),
                        Color::Muted,
                    ));
                }
            }
        }
        rows
    }

    /// The middle list: connections with their explorer trees.
    fn connection_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(error) = &self.connections_error {
            rows.push(Row::ConnectionsHeader);
            if !self.collapsed.contains("connections") {
                rows.push(Row::ConnNote(error.clone(), Color::Error));
            }
            return rows;
        }
        if !self.collapsed.contains("connections") {
            rows.push(Row::ConnectionsHeader);
            for (name, kind, workspace) in &self.connections {
                rows.push(Row::Connection(
                    name.clone(),
                    kind.clone(),
                    workspace.clone(),
                ));
                if let Some(missing) = self.missing_env.get(name) {
                    rows.push(Row::EnvHint(missing.clone()));
                }
                if !self.expanded.contains(name) {
                    continue;
                }
                match self.tables.get(name) {
                    None | Some(TablesState::Loading) => rows.push(Row::TablesLoading),
                    Some(TablesState::Failed(message)) => {
                        rows.push(Row::ConnNote(message.clone(), Color::Error));
                    }
                    Some(TablesState::Loaded { tables, .. }) if tables.is_empty() => {
                        rows.push(Row::ConnNote("No tables.".into(), Color::Muted));
                    }
                    Some(TablesState::Loaded { tables, .. }) => {
                        self.push_table_rows(&mut rows, name, tables);
                    }
                }
            }
        } else {
            rows.push(Row::ConnectionsHeader);
        }
        rows
    }

    /// One page of a connection's tables, narrowed by its search.
    fn push_table_rows(
        &self,
        rows: &mut Vec<Row>,
        name: &SharedString,
        tables: &[(String, String)],
    ) {
        let browser = self.browsers.get(name);
        let searching = browser.is_some_and(|browser| !browser.terms.is_empty());
        if tables.len() > TABLES_SEARCH_MIN || searching {
            rows.push(Row::TableSearch(name.clone()));
        }
        let total = match browser {
            Some(browser) if searching => browser.matches.len(),
            _ => tables.len(),
        };
        if total == 0 {
            rows.push(Row::ConnNote("No tables match.".into(), Color::Muted));
            return;
        }
        let shown = page_range(total, browser.map_or(0, |browser| browser.page));
        for position in shown.clone() {
            let index = match browser {
                Some(browser) if searching => browser.matches.get(position).copied(),
                _ => Some(position),
            };
            if let Some((schema, table)) = index.and_then(|index| tables.get(index)) {
                rows.push(Row::Table {
                    connection: name.clone(),
                    schema: schema.clone(),
                    table: table.clone(),
                });
            }
        }
        if searching || total > TABLES_PAGE {
            rows.push(Row::TablePager {
                connection: name.clone(),
                shown,
                total,
                searching,
            });
        }
    }

    /// The built-in runner, or what stands in its way. Only a licensed
    /// machine gets the row itself; everyone else is told what to do next.
    fn runner_rows(&self) -> Vec<Row> {
        let note = |text: &str, color: Color, detail: &String| {
            Row::RunnerNote(text.to_owned().into(), color, Some(detail.clone().into()))
        };
        match &self.runner.access {
            None => vec![Row::RunnerNote(
                "Checking the local runner licence…".into(),
                Color::Muted,
                None,
            )],
            Some(Access::Licensed) => {
                let mut rows = vec![Row::LocalRunner];
                if let Some(error) = &self.runner.error {
                    rows.push(Row::ConnNote(error.clone(), Color::Error));
                }
                rows
            }
            Some(Access::Unlicensed(detail)) => vec![note(
                "The local runner needs a licence — ask your provider for access.",
                Color::Warning,
                detail,
            )],
            Some(Access::SignIn(detail)) => vec![note(
                "Sign in to the runner's registry and refresh, or ask your provider for a licence.",
                Color::Warning,
                detail,
            )],
            Some(Access::NoDocker(detail)) => vec![note(
                "Start Docker and refresh to run pipelines on this machine.",
                Color::Muted,
                detail,
            )],
            Some(Access::Unavailable(detail)) => vec![note(
                "The local runner can't be checked right now.",
                Color::Muted,
                detail,
            )],
        }
    }

    /// Asks the registry whether this machine may run the EL image, and
    /// Docker what the project's runner container is doing, off the UI
    /// thread. A registry that never answers counts as unavailable.
    fn check_runner(&mut self, root: &std::path::Path, cx: &mut Context<Self>) {
        self.runner.checked_root = Some(root.to_path_buf());
        let root = root.to_path_buf();
        let timeout = cx.background_executor().timer(Duration::from_secs(20));
        let task = cx.background_spawn(async move {
            let image = local_runner::image(&root);
            let access = smol::future::or(local_runner::check_access(&image), async move {
                timeout.await;
                Access::Unavailable("the registry did not answer within 20 seconds".to_owned())
            })
            .await;
            let container = local_runner::container(&root).await.ok();
            (access, container)
        });
        self.runner._check = cx.spawn(async move |this, cx| {
            let (access, container) = task.await;
            this.update(cx, |this, cx| {
                this.runner.access = Some(access);
                this.runner.container = container;
                cx.notify();
            })
            .ok();
        });
    }

    fn start_runner(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.runner.busy = Some("starting…");
        self.runner.error = None;
        cx.notify();
        let task = cx.background_spawn({
            let root = root.clone();
            async move {
                let image = local_runner::image(&root);
                local_runner::start(&root, &image).await
            }
        });
        self.runner._action = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.runner.busy = None;
                if let Err(error) = result {
                    this.runner.error = Some(format!("{error:#}").into());
                }
                // remotes.yml may have gained `localhost`, and the container
                // has changed either way.
                this.refresh(cx);
                this.check_runner(&root, cx);
            })
            .ok();
        });
    }

    fn stop_runner(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.runner.busy = Some("stopping…");
        self.runner.error = None;
        cx.notify();
        let task = cx.background_spawn({
            let root = root.clone();
            async move { local_runner::stop(&root).await }
        });
        self.runner._action = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.runner.busy = None;
                if let Err(error) = result {
                    this.runner.error = Some(format!("{error:#}").into());
                }
                this.check_runner(&root, cx);
            })
            .ok();
        });
    }

    /// The bottom list: declared servers.
    fn remote_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        rows.push(Row::RemotesHeader);
        if self.collapsed.contains("remotes") {
            return rows;
        }
        let runner_offered = matches!(self.runner.access, Some(Access::Licensed));
        rows.extend(self.runner_rows());
        if self.remotes.is_empty() && !runner_offered {
            rows.push(Row::RemotesEmpty);
            rows.push(Row::ConnNote(
                "A server runs deployed pipelines on a schedule.".into(),
                Color::Muted,
            ));
        }
        for (name, host) in &self.remotes {
            // The runner row already stands for `localhost`.
            if runner_offered && *name == self.runner.remote_name {
                continue;
            }
            rows.push(Row::Remote(name.clone(), host.clone()));
            if let Some(var) = self.remotes_missing_token.get(name) {
                rows.push(Row::EnvHint(vec![var.clone()]));
            }
        }
        rows
    }

    fn toggle_section(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key);
        }
        cx.notify();
    }

    /// Opens the console's Remote tab on the named server.
    fn show_remote(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // Two separate leases: the console may read the workspace while it
        // refreshes, so it must not be updated inside workspace.update.
        let Some(console) = self.console(cx) else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.focus_panel::<super::ElRunsPanel>(window, cx);
            })
            .ok();
        console.update(cx, |panel, cx| panel.show_remote(name, cx));
    }

    /// The EL console panel, read without holding a workspace lease.
    fn console(&self, cx: &App) -> Option<Entity<super::ElRunsPanel>> {
        self.workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).panel::<super::ElRunsPanel>(cx))
    }

    pub fn remotes_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    fn edit_remote(
        &mut self,
        editing: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let panel = cx.entity().downgrade();
        self.workspace
            .update(cx, |workspace, cx| {
                super::remote_modal::ElRemoteModal::deploy(
                    workspace,
                    panel,
                    root,
                    editing.map(|name| name.to_string()),
                    window,
                    cx,
                );
            })
            .ok();
    }

    /// Kept for the YAML route: open (or scaffold) remotes.yml.
    #[allow(dead_code)]
    fn add_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let path = super::el_dir(&root).join("remotes.yml");
        if !path.exists() {
            let starter = "# el/remotes.yml — el serve daemons the IDE can drive from the \
Remote tab.\n# Tokens are ${VAR} references, never literals. Non-loopback URLs must be \
https.\nversion: 1\nremotes:\n  # local_daemon:\n  #   url: http://127.0.0.1:7431\n  #   \
token: \"${ZDBT_EL_TOKEN}\"\n";
            if let Err(error) = std::fs::write(&path, starter) {
                self.workspace
                    .update(cx, |workspace, cx| {
                        super::toast_error(
                            workspace,
                            &format!("could not create remotes.yml: {error}"),
                            None,
                            cx,
                        )
                    })
                    .ok();
                return;
            }
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                    .detach();
            })
            .ok();
    }

    /// Opens the project's .env — where `${VAR}` values live. Creates it
    /// with comment lines only when absent: .env is not a spec file, and
    /// no value is ever written here.
    fn open_env_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let path = root.join(".env");
        if !path.exists() {
            let starter = "# Values for the ${VAR} references in el/connections.yml and \
el/remotes.yml, one NAME=value per line.\n# Never commit this file.\n";
            if let Err(error) = std::fs::write(&path, starter) {
                self.workspace
                    .update(cx, |workspace, cx| {
                        super::toast_error(
                            workspace,
                            &format!("could not create .env: {error}"),
                            None,
                            cx,
                        )
                    })
                    .ok();
                return;
            }
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                    .detach();
            })
            .ok();
    }

    /// Opens el/connections.yml — the shared file that declares profiles.
    fn open_connections_yaml(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let path = super::el_dir(&root).join("connections.yml");
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, workspace::OpenOptions::default(), window, cx)
                    .detach();
            })
            .ok();
    }

    fn toggle_connection(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.expanded.contains(&name) {
            self.expanded.remove(&name);
        } else {
            self.expanded.insert(name.clone());
            if !self.browsers.contains_key(&name) {
                let browser = self.table_browser(name.clone(), window, cx);
                self.browsers.insert(name.clone(), browser);
            }
            if !self.tables.contains_key(&name) {
                self.load_tables(name.clone(), cx);
            }
        }
        // Touching a connection here is how the Query tab picks its target.
        if let Some(console) = self.console(cx) {
            console.update(cx, |panel, cx| panel.select_connection(name.clone(), cx));
        }
        cx.notify();
    }

    fn load_tables(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.tables.insert(name.clone(), TablesState::Loading);
        let epoch = self.tables_epoch;
        let connection_name = name.to_string();
        let task = cx.background_spawn(async move {
            super::list_tables(&root, &connection_name).map(|tables| {
                let haystack = super::fold_table_names(&tables);
                (tables, haystack)
            })
        });
        let key = name.clone();
        self._list_tasks.insert(
            key,
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if this.tables_epoch != epoch {
                        return; // a different environment answered
                    }
                    let state = match result {
                        Ok((tables, haystack)) => TablesState::Loaded { tables, haystack },
                        Err(error) => TablesState::Failed(format!("{error:#}").into()),
                    };
                    this.tables.insert(name.clone(), state);
                    // A search typed before a Refresh applies to the new list.
                    this.match_tables(&name);
                    cx.notify();
                })
                .ok();
            }),
        );
    }

    fn table_browser(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TableBrowser {
        let search = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Search tables…", window, cx);
            editor
        });
        let search_edits =
            cx.subscribe(&search, move |this, _, event: &editor::EditorEvent, cx| {
                if matches!(event, editor::EditorEvent::BufferEdited) {
                    this.search_tables(&name, cx);
                }
            });
        TableBrowser {
            search,
            terms: Vec::new(),
            matches: Vec::new(),
            page: 0,
            _search_edits: search_edits,
        }
    }

    fn search_tables(&mut self, name: &SharedString, cx: &mut Context<Self>) {
        let Some(browser) = self.browsers.get_mut(name) else {
            return;
        };
        browser.terms = super::search_terms(&browser.search.read(cx).text(cx));
        browser.page = 0;
        self.match_tables(name);
        cx.notify();
    }

    /// Recomputes a connection's matches against its loaded tables and
    /// keeps its page in range.
    fn match_tables(&mut self, name: &SharedString) {
        let (Some(browser), Some(TablesState::Loaded { haystack, .. })) =
            (self.browsers.get_mut(name), self.tables.get(name))
        else {
            return;
        };
        let total = if browser.terms.is_empty() {
            browser.matches.clear();
            haystack.len()
        } else {
            browser.matches = super::matching_tables(haystack, &browser.terms);
            browser.matches.len()
        };
        browser.page = page_range(total, browser.page).start / TABLES_PAGE;
    }

    /// Empties a connection's search field; false when there was nothing
    /// to empty.
    fn clear_table_search(
        &mut self,
        name: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(search) = self
            .browsers
            .get(name)
            .map(|browser| browser.search.clone())
        else {
            return false;
        };
        if search.read(cx).text(cx).is_empty() {
            return false;
        }
        search.update(cx, |editor, cx| editor.set_text("", window, cx));
        true
    }

    fn show_table_page(&mut self, name: &SharedString, page: usize, cx: &mut Context<Self>) {
        let Some(browser) = self.browsers.get_mut(name) else {
            return;
        };
        let total = match self.tables.get(name) {
            Some(TablesState::Loaded { tables, .. }) if browser.terms.is_empty() => tables.len(),
            Some(TablesState::Loaded { .. }) => browser.matches.len(),
            _ => 0,
        };
        browser.page = page_range(total, page).start / TABLES_PAGE;
        // A new page starts at its first table, not wherever the previous
        // page was scrolled to.
        if let Some(ix) = self
            .connection_rows()
            .iter()
            .position(|row| matches!(row, Row::Connection(connection, ..) if connection == name))
        {
            self.scroll_lower.scroll_to_item(ix, ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// Called by the connection modal after a successful write: drops
    /// stale table caches (names may have changed) and re-reads the spec.
    pub fn connections_changed(&mut self, cx: &mut Context<Self>) {
        self.tables.clear();
        self.expanded.clear();
        self.browsers.clear();
        self.tables_epoch += 1;
        self.refresh(cx);
    }

    fn edit_connection(
        &mut self,
        editing: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let panel = cx.entity().downgrade();
        self.workspace
            .update(cx, |workspace, cx| {
                super::connection_modal::ElConnectionModal::deploy(
                    workspace,
                    panel,
                    root,
                    editing.map(|name| name.to_string()),
                    window,
                    cx,
                );
            })
            .ok();
    }

    fn query_table(
        &mut self,
        connection: SharedString,
        schema: String,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(console) = self.console(cx) else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.focus_panel::<super::ElRunsPanel>(window, cx);
            })
            .ok();
        console.update(cx, |panel, cx| {
            panel.show_query_for_table(connection, &schema, &table, window, cx);
        });
    }

    fn open_pipeline(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| {
                super::ElPipelineCanvas::deploy(workspace, root, path, window, cx);
            })
            .ok();
    }

    fn new_pipeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let dir = super::el_dir(&root).join("pipelines");
        let _ = std::fs::create_dir_all(&dir);
        // First free pipeline_<n> name.
        let mut index = 1;
        let path = loop {
            let candidate = dir.join(format!("pipeline_{index}.yml"));
            if !candidate.exists() {
                break candidate;
            }
            index += 1;
        };
        // Seed source/target from the project's real connections so the
        // starter validates immediately.
        let source = self
            .connections
            .iter()
            .find(|(_, kind, _)| !matches!(kind.as_ref(), "snowflake"))
            .or(self.connections.first())
            .map(|(name, _, _)| name.to_string())
            .unwrap_or_else(|| "files".to_owned());
        let target = self
            .connections
            .iter()
            .find(|(_, kind, _)| matches!(kind.as_ref(), "duckdb" | "snowflake" | "oracle"))
            .map(|(name, _, _)| name.to_string())
            .unwrap_or_else(|| "warehouse".to_owned());
        let starter = format!(
            "# yaml-language-server: $schema=../.zdbt/el-pipeline.schema.json\n\
             version: 1\npipeline: pipeline_{index}\nsource: {source}\n\
             target:\n  connection: {target}\n  schema: LANDING\n  table: '{{stream}}'\nstreams: []\n"
        );
        if std::fs::write(&path, starter).is_ok() {
            self.refresh(cx);
            self.open_pipeline(path, window, cx);
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| {
                super::initialize_workspace(workspace, window, cx);
            })
            .ok();
        self.refresh(cx);
    }
}

impl EventEmitter<PanelEvent> for ElPanel {}

impl Focusable for ElPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ElPanel {
    fn persistent_name() -> &'static str {
        "EL Panel"
    }

    fn panel_key() -> &'static str {
        "ElPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(280.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ArrowRightLeft)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("EL Pipelines")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElPanelFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |this, _, cx| this.refresh(cx));
        }
    }
}

impl Render for ElPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let make_list = |id: &'static str,
                         rows: Vec<Row>,
                         scroll: &UniformListScrollHandle,
                         cx: &mut Context<Self>| {
            let rows = std::sync::Arc::new(rows);
            let count = rows.len();
            let entity = cx.entity();
            gpui::uniform_list(id, count, {
                move |range, _window, cx| {
                    entity.update(cx, |this, cx| {
                        range
                            .filter_map(|ix| rows.get(ix).map(|row| this.render_row(row, ix, cx)))
                            .collect::<Vec<_>>()
                    })
                }
            })
            .track_scroll(scroll)
        };
        let empty = self.is_empty_project();
        let upper = make_list("el-panel-upper", self.upper_rows(), &self.scroll, cx);
        let middle = make_list(
            "el-panel-connections",
            self.connection_rows(),
            &self.scroll_lower,
            cx,
        );
        let bottom = make_list(
            "el-panel-remotes",
            self.remote_rows(),
            &self.scroll_remotes,
            cx,
        );
        let dragging = self.split_drag.is_some();
        const HEADER_ONLY: f32 = 26.;
        // A 1px line with a 5px grab area — drag to trade space.
        let splitter = |which: usize, dragging: bool, cx: &mut Context<Self>| {
            div()
                .id(("el-panel-split", which))
                .w_full()
                .h(px(5.))
                .flex_shrink_0()
                .cursor(gpui::CursorStyle::ResizeRow)
                .border_t_1()
                .border_color(if dragging {
                    colors.border_focused
                } else {
                    colors.border
                })
                .hover(|style| style.border_color(colors.border_focused))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let start = if which == 0 {
                            this.split
                        } else {
                            this.split_connections
                        };
                        this.split_drag = Some((which, f32::from(event.position.y), start));
                        cx.notify();
                    }),
                )
        };
        let pipelines_h = if self.collapsed.contains("pipelines") {
            HEADER_ONLY
        } else {
            self.split
        };
        let connections_h = if self.collapsed.contains("connections") {
            HEADER_ONLY
        } else {
            self.split_connections
        };
        let list: gpui::AnyElement = if empty {
            upper.flex_1().into_any_element()
        } else {
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .h(px(pipelines_h))
                        .flex_shrink_0()
                        .child(upper.size_full())
                        .vertical_scrollbar_for(&self.scroll, window, cx),
                )
                .child(splitter(0, dragging, cx))
                .child(
                    div()
                        .h(px(connections_h))
                        .flex_shrink_0()
                        .child(middle.size_full())
                        .vertical_scrollbar_for(&self.scroll_lower, window, cx),
                )
                .child(splitter(1, dragging, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(bottom.size_full())
                        .vertical_scrollbar_for(&self.scroll_remotes, window, cx),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElPanel")
            .bg(colors.panel_background)
            .when(dragging, |flex| {
                flex.on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((which, start_y, start_split)) = this.split_drag {
                        let next =
                            (start_split + f32::from(event.position.y) - start_y).clamp(60., 900.);
                        if which == 0 {
                            this.split = next;
                        } else {
                            this.split_connections = next;
                        }
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.split_drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.split_drag = None;
                        cx.notify();
                    }),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new("EL").size(LabelSize::Default))
                    .child(div().flex_1())
                    .children(self.profile.clone().map(|profile| {
                        div()
                            .id("el-profile-badge")
                            .tooltip(|_, cx| {
                                Tooltip::with_meta("Active profile", None, PROFILE_HINT, cx)
                            })
                            .child(
                                Label::new(profile)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            )
                    }))
                    .child(
                        IconButton::new("el-panel-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Explicit refresh also invalidates cached
                                // table listings.
                                this.tables.clear();
                                for name in this.expanded.clone() {
                                    this.load_tables(name, cx);
                                }
                                this.refresh(cx);
                                if let Some(root) = this.root.clone() {
                                    this.check_runner(&root, cx);
                                }
                            })),
                    ),
            )
            .children((!self.profiles.is_empty()).then(|| {
                // The environment switcher: same pipelines, different
                // connections. One dropdown, the active profile as its
                // face — the panel's single accent.
                let profiles = self.profiles.clone();
                let active = self.profile.clone();
                let panel = cx.entity().downgrade();
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("profile")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        ui::PopoverMenu::new("el-profile-select")
                            .trigger(
                                Button::new(
                                    "el-profile-trigger",
                                    active.clone().unwrap_or_else(|| "choose…".into()),
                                )
                                .label_size(LabelSize::XSmall)
                                .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                                .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                                .tooltip(|_, cx| {
                                    Tooltip::with_meta("Profile", None, PROFILE_HINT, cx)
                                }),
                            )
                            .menu(move |window, cx| {
                                let panel = panel.clone();
                                let profiles = profiles.clone();
                                let active = active.clone();
                                Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                                    menu = menu.header("Profiles in connections.yml");
                                    for name in profiles {
                                        let panel = panel.clone();
                                        let selected = active.as_ref() == Some(&name);
                                        let label = name.clone();
                                        menu = menu.toggleable_entry(
                                            label,
                                            selected,
                                            ui::IconPosition::Start,
                                            None,
                                            move |_, cx| {
                                                panel
                                                    .update(cx, |this, cx| {
                                                        this.switch_profile(name.clone(), cx)
                                                    })
                                                    .ok();
                                            },
                                        );
                                    }
                                    menu.separator().entry(
                                        "Edit connections.yml",
                                        None,
                                        move |window, cx| {
                                            panel
                                                .update(cx, |this, cx| {
                                                    this.open_connections_yaml(window, cx)
                                                })
                                                .ok();
                                        },
                                    )
                                }))
                            }),
                    )
            }))
            .child(list)
    }
}

impl ElPanel {
    /// A section header: chevron + label toggle the fold; callers append
    /// the section's "+" action.
    fn render_section_header(
        &self,
        base: gpui::Stateful<gpui::Div>,
        key: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let folded = self.collapsed.contains(key);
        let colors = cx.theme().colors();
        base.cursor_pointer()
            .bg(colors.element_background)
            .border_b_1()
            .border_color(colors.border)
            .child(
                Icon::new(if folded {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(IconSize::XSmall)
                .color(Color::Muted),
            )
            .child(
                Label::new(label)
                    .size(LabelSize::Default)
                    .color(Color::Default),
            )
            .child(div().flex_1())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(key, cx)))
    }

    fn render_row(&self, row: &Row, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let base = h_flex()
            .id(ix)
            .h(px(24.))
            .w_full()
            .px_2()
            .gap_1()
            .items_center()
            .hover(|style| style.bg(cx.theme().colors().element_hover));
        match row {
            Row::PipelinesHeader => self
                .render_section_header(base, "pipelines", "Pipelines", cx)
                .child(
                    IconButton::new("el-new-pipeline", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("New pipeline"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.new_pipeline(window, cx)
                        })),
                )
                .into_any_element(),
            Row::ConnectionsHeader => self
                .render_section_header(base, "connections", "Connections", cx)
                .child(
                    IconButton::new("el-add-connection", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Add connection"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_connection(None, window, cx)
                        })),
                )
                .into_any_element(),
            Row::RemotesHeader => self
                .render_section_header(base, "remotes", "Remotes", cx)
                .child(
                    IconButton::new("el-add-remote", IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Add server"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_remote(None, window, cx)
                        })),
                )
                .into_any_element(),
            Row::Remote(name, host) => {
                let open_name = name.clone();
                base.cursor_pointer()
                    .child(
                        Icon::new(IconName::Server)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                    .child(div().flex_1())
                    .child(
                        Label::new(host.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .child({
                        let name = name.clone();
                        IconButton::new(("el-remote-edit", ix), IconName::Pencil)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Edit server"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.edit_remote(Some(name.clone()), window, cx);
                            }))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_remote(open_name.clone(), window, cx);
                    }))
                    .into_any_element()
            }
            Row::Note(text) => base
                .child(
                    Label::new(text.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Row::Pipeline(path) => {
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("pipeline")
                    .to_owned();
                let path = path.clone();
                base.cursor_pointer()
                    .child(
                        Icon::new(IconName::ArrowRightLeft)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name).size(LabelSize::Small).truncate())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_pipeline(path.clone(), window, cx);
                    }))
                    .into_any_element()
            }
            Row::Connection(name, kind, workspace) => {
                let can_browse = super::browsable(kind);
                let expanded = self.expanded.contains(name);
                let chevron = if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                };
                let toggle_name = name.clone();
                let kind_hint: SharedString = format!(
                    "{kind} connections can't be browsed yet — describe what to load with + Source on the canvas."
                )
                .into();
                base.when(can_browse, |row| {
                    row.cursor_pointer()
                        .child(
                            Icon::new(chevron)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_connection(toggle_name.clone(), window, cx);
                        }))
                })
                .when(!can_browse, |row| row.tooltip(Tooltip::text(kind_hint)))
                .child(
                    Icon::new(IconName::DatabaseZap)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                .child(div().flex_1())
                // Where this one runs. Shown even when it is "local", so
                // the answer is never something you have to open a modal
                // to learn.
                .child(Label::new(workspace.clone()).size(LabelSize::XSmall).color(
                    if workspace.as_ref() == "local" {
                        Color::Muted
                    } else {
                        Color::Accent
                    },
                ))
                .child(
                    Label::new(kind.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child({
                    let name = name.clone();
                    IconButton::new(("el-conn-edit", ix), IconName::Pencil)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Edit connection"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_connection(Some(name.clone()), window, cx);
                        }))
                })
                .into_any_element()
            }
            Row::Table {
                connection,
                schema,
                table,
            } => {
                let connection = connection.clone();
                let schema = schema.clone();
                let table = table.clone();
                let label = format!("{schema}.{table}");
                let dragged = super::DraggedTable {
                    connection: connection.clone(),
                    schema: schema.clone(),
                    table: table.clone(),
                };
                let drag_label: SharedString = label.clone().into();
                let highlights = self
                    .browsers
                    .get(&connection)
                    .map(|browser| super::highlight_ranges(&label, &browser.terms))
                    .unwrap_or_default();
                let name: gpui::AnyElement = if highlights.is_empty() {
                    Label::new(label)
                        .size(LabelSize::Small)
                        .truncate()
                        .into_any_element()
                } else {
                    HighlightedLabel::from_ranges(label, highlights)
                        .size(LabelSize::Small)
                        .truncate()
                        .into_any_element()
                };
                base.cursor_pointer()
                    .pl_6()
                    .child(
                        Icon::new(IconName::Table)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(name)
                    .tooltip(|_, cx| {
                        Tooltip::with_meta(
                            "Drag onto a pipeline canvas to add it as a stream",
                            None,
                            "Click to query it",
                            cx,
                        )
                    })
                    // Drag onto a pipeline canvas to add it as a stream.
                    .on_drag(dragged, move |_, _, _, cx| {
                        let drag_label = drag_label.clone();
                        cx.new(|_| DraggedTablePreview(drag_label))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.query_table(
                            connection.clone(),
                            schema.clone(),
                            table.clone(),
                            window,
                            cx,
                        );
                    }))
                    .into_any_element()
            }
            Row::LocalRunner => {
                let running = self.runner.container == Some(Container::Running);
                let busy = self.runner.busy;
                let status: SharedString = match (busy, &self.runner.container) {
                    (Some(busy), _) => busy.into(),
                    (None, Some(Container::Running)) => "running".into(),
                    (None, Some(Container::Stopped(status))) if !status.is_empty() => {
                        status.clone().into()
                    }
                    _ => "stopped".into(),
                };
                let control = if running {
                    IconButton::new("el-runner-stop", IconName::Stop)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .disabled(busy.is_some())
                        .tooltip(Tooltip::text("Stop the local runner"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.stop_runner(cx);
                        }))
                } else {
                    IconButton::new("el-runner-start", IconName::PlayFilled)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .disabled(busy.is_some())
                        .tooltip(Tooltip::text("Start the local runner"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.start_runner(cx);
                        }))
                };
                base.child(
                    Icon::new(IconName::Server)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(self.runner.remote_name.clone()).size(LabelSize::Small))
                .child(div().flex_1())
                .child(
                    Label::new(status)
                        .size(LabelSize::XSmall)
                        .color(if running {
                            Color::Success
                        } else {
                            Color::Muted
                        })
                        .truncate(),
                )
                .child(control)
                .tooltip({
                    let name = self.runner.remote_name.clone();
                    move |_, cx| {
                        Tooltip::with_meta(
                            "Local runner",
                            None,
                            format!(
                                "The EL daemon in this machine's Docker, serving this project at \
                                 http://127.0.0.1:7431 as the `{name}` server in el/remotes.yml. \
                                 Point a connection at it with `workspace: {name}`."
                            ),
                            cx,
                        )
                    }
                })
                .when(running, |row| {
                    let name = self.runner.remote_name.clone();
                    row.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.show_remote(name.clone(), window, cx);
                        }))
                })
                .into_any_element()
            }
            Row::RunnerNote(text, color, detail) => base
                .pl_6()
                .child(
                    Label::new(text.clone())
                        .size(LabelSize::XSmall)
                        .color(*color),
                )
                .when_some(detail.clone(), |row, detail| {
                    let text = text.clone();
                    row.tooltip(move |_, cx| {
                        Tooltip::with_meta(text.clone(), None, detail.clone(), cx)
                    })
                })
                .into_any_element(),
            Row::ConnNote(text, color) => base
                .pl_6()
                .child(
                    Label::new(text.clone())
                        .size(LabelSize::XSmall)
                        .color(*color),
                )
                .into_any_element(),
            Row::TablesLoading => base
                .pl_6()
                .child(
                    SpinnerLabel::new()
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child(
                    Label::new("Loading tables…")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Row::TableSearch(connection) => {
                let Some(browser) = self.browsers.get(connection) else {
                    return base.into_any_element();
                };
                let searching = !browser.terms.is_empty();
                let clear = connection.clone();
                let cancel = connection.clone();
                h_flex()
                    .id(ix)
                    .h(px(24.))
                    .w_full()
                    .pl_6()
                    .pr_2()
                    .gap_1()
                    .items_center()
                    .child(
                        Icon::new(IconName::MagnifyingGlass)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(browser.search.clone()),
                    )
                    .when(searching, |row| {
                        row.child(
                            IconButton::new(("el-table-search-clear", ix), IconName::Close)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Clear search"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.clear_table_search(&clear, window, cx);
                                })),
                        )
                    })
                    // Escape empties the search; with nothing to empty it
                    // goes on to whatever else handles Escape.
                    .on_action(
                        cx.listener(move |this, _: &editor::actions::Cancel, window, cx| {
                            if !this.clear_table_search(&cancel, window, cx) {
                                cx.propagate();
                            }
                        }),
                    )
                    .into_any_element()
            }
            Row::TablePager {
                connection,
                shown,
                total,
                searching,
            } => {
                let pages = total.div_ceil(TABLES_PAGE);
                let page = shown.start / TABLES_PAGE;
                let noun = match (*searching, *total) {
                    (true, 1) => "match",
                    (true, _) => "matches",
                    (false, 1) => "table",
                    (false, _) => "tables",
                };
                let status = if pages > 1 {
                    format!(
                        "{}–{} of {} {noun}",
                        super::thousands(shown.start + 1),
                        super::thousands(shown.end),
                        super::thousands(*total)
                    )
                } else {
                    format!("{} {noun}", super::thousands(*total))
                };
                let previous = connection.clone();
                let next = connection.clone();
                h_flex()
                    .id(ix)
                    .h(px(24.))
                    .w_full()
                    .pl_6()
                    .pr_2()
                    .gap_1()
                    .items_center()
                    .child(
                        Label::new(status)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .child(div().flex_1())
                    .when(pages > 1, |row| {
                        row.child(
                            IconButton::new(("el-tables-previous", ix), IconName::ChevronLeft)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .disabled(page == 0)
                                .tooltip(Tooltip::text("Previous page"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.show_table_page(&previous, page.saturating_sub(1), cx);
                                })),
                        )
                        .child(
                            Label::new(format!("{} / {}", page + 1, pages))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            IconButton::new(("el-tables-next", ix), IconName::ChevronRight)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .disabled(page + 1 >= pages)
                                .tooltip(Tooltip::text("Next page"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.show_table_page(&next, page + 1, cx);
                                })),
                        )
                    })
                    .into_any_element()
            }
            Row::EnvHint(vars) => {
                // The row is narrow and the names are the whole point, so
                // the tooltip repeats them rather than the explanation alone.
                let title =
                    super::runs_panel::tooltip_text(&format!("Set {} in .env", vars.join(", ")));
                base.pl_6()
                    .cursor_pointer()
                    .child(
                        Label::new(format!("Set {} in .env to connect", vars.join(", ")))
                            .size(LabelSize::XSmall)
                            .color(Color::Warning)
                            .truncate(),
                    )
                    .tooltip(move |_, cx| {
                        Tooltip::with_meta(
                            title.clone(),
                            None,
                            "The YAML references ${VAR}; the value lives in .env next to \
                             the project (or in your shell). Click to open .env.",
                            cx,
                        )
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.open_env_file(window, cx)))
                    .into_any_element()
            }
            Row::RemotesEmpty => base
                .pl_6()
                .child(
                    Label::new("No servers yet — press + to add one.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .tooltip(|_, cx| {
                    Tooltip::with_meta(
                        "Servers",
                        None,
                        "el/remotes.yml lists the el serve daemons the IDE deploys to and \
                         watches. Add server points at a running daemon or installs one over \
                         SSH (zdbt el install-remote). Tokens are ${VAR} references resolved \
                         from .env.",
                        cx,
                    )
                })
                .into_any_element(),
            Row::Initialize => base
                .cursor_pointer()
                .child(
                    Icon::new(IconName::Plus)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new("Initialize EL workspace…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .on_click(cx.listener(|this, _, window, cx| this.initialize(window, cx)))
                .into_any_element(),
        }
    }
}

/// The rows page `page` shows out of `total`, clamped to the last page.
fn page_range(total: usize, page: usize) -> Range<usize> {
    let last_page = total.saturating_sub(1) / TABLES_PAGE;
    let start = page.min(last_page) * TABLES_PAGE;
    start..(start + TABLES_PAGE).min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_stay_within_the_list() {
        assert_eq!(page_range(9_331, 0), 0..100);
        assert_eq!(page_range(9_331, 93), 9_300..9_331);
        assert_eq!(page_range(9_331, 500), 9_300..9_331);
        assert_eq!(page_range(100, 1), 0..100);
        assert_eq!(page_range(0, 3), 0..0);
    }
}
