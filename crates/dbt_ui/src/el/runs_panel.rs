//! The EL console — the plugin's bottom-dock surface. Two views: Runs
//! (hosting [`super::run_view::ElRunView`]) and Query (ad-hoc SQL against
//! any EL connection, through the on-demand worker). Mapping-editor
//! previews overlay either view and return with Back.

use std::path::PathBuf;
use std::time::Duration;

use editor::Editor;
use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::run_view::ElRunView;
use crate::ToggleElRunsFocus;

const QUERY_ROW_CAP: usize = 500;

#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Runs,
    Query,
    Remote,
}

pub struct ElRunsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    run_view: Entity<ElRunView>,
    surface: Surface,
    /// A preview table (stream sample or failed casts) — overlays the
    /// current surface until Back.
    preview: Option<PreviewTable>,
    preview_scroll: UniformListScrollHandle,
    // Query surface state.
    sql: Entity<Editor>,
    /// (name, kind) — names only, credential posture as everywhere.
    connections: Vec<(SharedString, SharedString)>,
    selected: Option<usize>,
    root: Option<PathBuf>,
    running: bool,
    result: Option<PreviewTable>,
    result_scroll: UniformListScrollHandle,
    elapsed: Option<Duration>,
    query_error: Option<SharedString>,
    _query: Task<()>,
    // Remote surface state (from el/remotes.yml).
    remotes: Vec<SharedString>,
    selected_remote: Option<usize>,
    remote_pipelines: Vec<el_engine::server::RemotePipeline>,
    remote_runs: Vec<el_engine::server::RemoteRun>,
    remote_error: Option<SharedString>,
    /// A failed Run/Cancel action — kept visible until the next action
    /// or remote switch (the 2s poll must not wipe it).
    remote_action_error: Option<SharedString>,
    remote_health: Option<SharedString>,
    remote_logs: Vec<SharedString>,
    remote_log_next: u64,
    remote_show_logs: bool,
    /// A pipeline opened in the detail view; None = overview.
    remote_detail: Option<SharedString>,
    /// The daemon's active profile (from /health) — shown when confirming
    /// a deploy so the developer knows which environment receives it.
    remote_profile: Option<String>,
    /// Deploy needs a second click: first arms, second sends.
    deploy_armed: bool,
    remote_epoch: u64,
    _remote_poll: Task<()>,
}

pub struct PreviewTable {
    pub title: SharedString,
    pub columns: std::sync::Arc<Vec<SharedString>>,
    pub rows: std::sync::Arc<Vec<Vec<SharedString>>>,
}

impl ElRunsPanel {
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
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        let sql = cx.new(|cx| {
            let mut editor = Editor::auto_height(3, 8, window, cx);
            editor.set_placeholder_text("SELECT …", window, cx);
            editor
        });
        cx.new(|cx| {
            let run_view = cx.new(|cx| ElRunView::new(workspace_handle.clone(), cx));
            Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                run_view,
                surface: Surface::Runs,
                preview: None,
                preview_scroll: UniformListScrollHandle::new(),
                sql,
                connections: Vec::new(),
                selected: None,
                root: None,
                running: false,
                result: None,
                result_scroll: UniformListScrollHandle::new(),
                elapsed: None,
                query_error: None,
                _query: Task::ready(()),
                remotes: Vec::new(),
                selected_remote: None,
                remote_pipelines: Vec::new(),
                remote_runs: Vec::new(),
                remote_error: None,
                remote_action_error: None,
                remote_health: None,
                remote_logs: Vec::new(),
                remote_log_next: 0,
                remote_show_logs: false,
                remote_detail: None,
                remote_profile: None,
                deploy_armed: false,
                remote_epoch: 0,
                _remote_poll: Task::ready(()),
            }
        })
    }

    pub fn run_view(&self) -> Entity<ElRunView> {
        self.run_view.clone()
    }

    pub fn show_preview(
        &mut self,
        title: SharedString,
        columns: Vec<SharedString>,
        rows: Vec<Vec<SharedString>>,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(PreviewTable {
            title,
            columns: std::sync::Arc::new(columns),
            rows: std::sync::Arc::new(rows),
        });
        cx.notify();
    }

    pub fn show_runs(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        cx.notify();
    }

    /// A profile switch changed what every connection name means: drop
    /// the old environment's query state and re-read the resolved set.
    pub fn profile_changed(&mut self, cx: &mut Context<Self>) {
        self.remote_detail = None;
        self.deploy_armed = false;
        self.result = None;
        self.query_error = None;
        self.elapsed = None;
        self.refresh_connections(cx);
        if self.surface == Surface::Remote {
            self.start_remote_poll(cx);
        }
        cx.notify();
    }

    /// The explorer's click-a-table entry point: switches to Query,
    /// selects the connection, seeds the SQL, and runs it.
    pub fn show_query_for_table(
        &mut self,
        connection: SharedString,
        schema: &str,
        table: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = None;
        self.surface = Surface::Query;
        self.refresh_connections(cx);
        self.selected = self
            .connections
            .iter()
            .position(|(name, _)| *name == connection)
            .or(self.selected);
        let quote = |ident: &str| format!("\"{}\"", ident.replace('"', "\"\""));
        let sql = format!(
            "SELECT * FROM {}.{} LIMIT 200",
            quote(schema),
            quote(table)
        );
        self.sql.update(cx, |editor, cx| {
            editor.set_text(sql, window, cx);
        });
        self.run_query(cx);
    }

    fn refresh_connections(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone().or_else(|| {
            self.workspace
                .upgrade()
                .and_then(|workspace| super::discover_el_root(workspace.read(cx), cx))
        });
        self.root = root.clone();
        let Some(root) = root else { return };
        self.connections = el_engine::spec::load_active_connections(&root)
            .map(|(connections, _)| {
                connections
                    .connections
                    .iter()
                    .map(|(name, connection)| {
                        (name.clone().into(), connection.kind().to_owned().into())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.selected.map_or(true, |ix| ix >= self.connections.len()) {
            self.selected = (!self.connections.is_empty()).then_some(0);
        }
        self.remotes = el_engine::spec::load_remotes(
            &super::el_dir(&root).join("remotes.yml"),
        )
        .map(|remotes| remotes.remotes.keys().map(|name| name.clone().into()).collect())
        .unwrap_or_default();
        if self.selected_remote.map_or(true, |ix| ix >= self.remotes.len()) {
            self.selected_remote = (!self.remotes.is_empty()).then_some(0);
        }
    }

    /// Restarts the remote poll loop: fetch pipelines + runs now, then
    /// every two seconds while the Remote surface stays visible.
    fn start_remote_poll(&mut self, cx: &mut Context<Self>) {
        self.remote_epoch += 1;
        self.remote_health = None;
        self.remote_logs.clear();
        self.remote_log_next = 0;
        let epoch = self.remote_epoch;
        let Some(root) = self.root.clone() else { return };
        let Some(name) = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix))
            .map(|name| name.to_string())
        else {
            return;
        };
        self._remote_poll = cx.spawn(async move |this, cx| {
            let mut log_cursor = 0u64;
            loop {
                let root = root.clone();
                let name = name.clone();
                let since = log_cursor;
                let fetch = cx
                    .background_spawn(async move {
                        let client = el_engine::server::RemoteClient::connect(&root, &name)?;
                        let pipelines = client.pipelines()?;
                        let runs = client.runs()?;
                        let health = client.health().ok();
                        let logs = client.logs(since).ok();
                        anyhow::Ok((pipelines, runs, health, logs))
                    })
                    .await;
                if let Ok((_, _, _, Some((_, next)))) = &fetch {
                    log_cursor = *next;
                }
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.remote_epoch != epoch {
                            return false;
                        }
                        match fetch {
                            Ok((pipelines, runs, health, logs)) => {
                                this.remote_pipelines = pipelines;
                                this.remote_runs = runs;
                                this.remote_error = None;
                                this.remote_profile = health
                                    .as_ref()
                                    .and_then(|value| value.get("profile"))
                                    .and_then(|name| name.as_str())
                                    .map(str::to_owned);
                                this.remote_health = health.map(|value| {
                                    let uptime = value
                                        .get("uptime_secs")
                                        .and_then(|secs| secs.as_u64())
                                        .unwrap_or(0);
                                    let running = value
                                        .get("running")
                                        .and_then(|count| count.as_u64())
                                        .unwrap_or(0);
                                    let uptime = if uptime >= 3600 {
                                        format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
                                    } else if uptime >= 60 {
                                        format!("{}m", uptime / 60)
                                    } else {
                                        format!("{uptime}s")
                                    };
                                    let profile = value
                                        .get("profile")
                                        .and_then(|name| name.as_str())
                                        .map(|name| format!(", profile {name}"))
                                        .unwrap_or_default();
                                    format!(
                                        "connected — up {uptime}, {running} running{profile}"
                                    )
                                    .into()
                                });
                                if let Some((lines, next)) = logs {
                                    this.remote_log_next = next;
                                    this.remote_logs
                                        .extend(lines.into_iter().map(SharedString::from));
                                    let overflow =
                                        this.remote_logs.len().saturating_sub(400);
                                    if overflow > 0 {
                                        this.remote_logs.drain(..overflow);
                                    }
                                }
                            }
                            Err(error) => {
                                this.remote_health = None;
                                this.remote_error = Some(format!("{error:#}").into());
                            }
                        }
                        cx.notify();
                        this.surface == Surface::Remote
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(2))
                    .await;
            }
        });
    }

    /// Deploys local pipeline YAMLs to the selected remote — all of them,
    /// or just `only`. Two-click armed elsewhere; this sends.
    fn deploy_to_remote(&mut self, only: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let Some(remote) = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix))
            .map(|name| name.to_string())
        else {
            return;
        };
        self.deploy_armed = false;
        self.remote_action_error = None;
        let workspace = self.workspace.clone();
        let task = cx.background_spawn(async move {
            use anyhow::Context as _;
            let mut bundle: Vec<(String, String)> = Vec::new();
            for path in el_engine::spec::list_pipelines(&super::el_dir(&root)) {
                let pipeline = el_engine::spec::load_pipeline(&path).map_err(|error| {
                    anyhow::anyhow!(
                        "{} could not be read ({error}) — fix it before deploying",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("a pipeline file")
                    )
                })?;
                if only.as_ref().is_some_and(|name| pipeline.pipeline != *name) {
                    continue;
                }
                let yaml =
                    std::fs::read_to_string(&path).context("reading pipeline file")?;
                bundle.push((pipeline.pipeline, yaml));
            }
            if bundle.is_empty() {
                anyhow::bail!("nothing to deploy");
            }
            let client = el_engine::server::RemoteClient::connect(&root, &remote)?;
            let deployed = client.deploy(&bundle)?;
            anyhow::Ok((remote, deployed))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((remote, deployed)) => {
                        workspace
                            .update(cx, |workspace, cx| {
                                super::toast(
                                    workspace,
                                    &format!(
                                        "Deployed {} pipeline(s) to {remote}.",
                                        deployed.len()
                                    ),
                                    cx,
                                );
                            })
                            .ok();
                        this.start_remote_poll(cx);
                    }
                    Err(error) => {
                        this.remote_action_error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fire-and-refresh action against the selected remote.
    fn remote_action(
        &mut self,
        action: impl FnOnce(&el_engine::server::RemoteClient) -> anyhow::Result<()>
            + Send
            + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else { return };
        let Some(name) = self
            .selected_remote
            .and_then(|ix| self.remotes.get(ix))
            .map(|name| name.to_string())
        else {
            return;
        };
        self.remote_action_error = None;
        let task = cx.background_spawn(async move {
            let client = el_engine::server::RemoteClient::connect(&root, &name)?;
            action(&client)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.start_remote_poll(cx),
                    Err(error) => {
                        this.remote_action_error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_query(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.query_error = None;
        let Some(root) = self.root.clone() else {
            self.query_error = Some("No EL project in this workspace — open one or run el: initialize workspace.".into());
            cx.notify();
            return;
        };
        let Some((name, _)) = self.selected.and_then(|ix| self.connections.get(ix)) else {
            self.query_error = Some("Pick a connection first.".into());
            cx.notify();
            return;
        };
        let sql = self.sql.read(cx).text(cx).trim().to_owned();
        if sql.is_empty() {
            self.query_error =
                Some("Write a query first — or click a table in the EL panel.".into());
            cx.notify();
            return;
        }
        let Some(worker) = super::find_worker() else {
            self.query_error = Some(
                "Connector worker not found — build zdbt-el-worker or set ZDBT_EL_WORKER.".into(),
            );
            cx.notify();
            return;
        };
        let connection_name = name.to_string();
        self.running = true;
        self.result = None;
        self.elapsed = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            let started = std::time::Instant::now();
            let (connections, _) = el_engine::spec::load_active_connections(&root)?;
            let connection = connections
                .connections
                .get(&connection_name)
                .ok_or_else(|| anyhow::anyhow!("connection {connection_name:?} is gone from connections.yml"))?;
            let env = el_engine::env::EnvMap::load(&root, None);
            let result = el_engine::explore::run_query(
                &worker,
                &root,
                connection,
                &env,
                &sql,
                QUERY_ROW_CAP,
            )?;
            anyhow::Ok((result, started.elapsed()))
        });
        self._query = cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.running = false;
                match outcome {
                    Ok((result, elapsed)) => {
                        this.elapsed = Some(elapsed);
                        this.result = Some(PreviewTable {
                            title: "query".into(),
                            columns: std::sync::Arc::new(
                                result.columns.into_iter().map(Into::into).collect(),
                            ),
                            rows: std::sync::Arc::new(
                                result
                                    .rows
                                    .into_iter()
                                    .map(|row| row.into_iter().map(Into::into).collect())
                                    .collect(),
                            ),
                        });
                    }
                    Err(error) => this.query_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn render_query(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let chips = h_flex().gap_1().flex_wrap().children(
            self.connections
                .iter()
                .enumerate()
                .map(|(ix, (name, kind))| {
                    let selected = self.selected == Some(ix);
                    Button::new(("el-query-conn", ix), name.clone())
                        .label_size(LabelSize::Small)
                        .toggle_state(selected)
                        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                        .tooltip(ui::Tooltip::text(format!("{kind} connection")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(ix);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>(),
        );

        let status: Option<SharedString> = if self.running {
            Some("Running…".into())
        } else if let (Some(result), Some(elapsed)) = (&self.result, self.elapsed) {
            let count = result.rows.len();
            let capped = if count >= QUERY_ROW_CAP {
                format!(" (first {QUERY_ROW_CAP})")
            } else {
                String::new()
            };
            Some(format!("{count} rows{capped} in {:.2}s", elapsed.as_secs_f64()).into())
        } else {
            None
        };

        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .child(chips)
            .child(div().flex_1())
            .children(status.map(|status| {
                Label::new(status).size(LabelSize::XSmall).color(Color::Muted)
            }))
            .child(
                Button::new("el-query-run", "Run query")
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .disabled(self.running)
                    .on_click(cx.listener(|this, _, _, cx| this.run_query(cx))),
            );

        let editor = div()
            .w_full()
            .px_1()
            .pb_1()
            .child(
                div()
                    .w_full()
                    .p_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.editor_background)
                    .child(self.sql.clone()),
            );

        let body: gpui::AnyElement = if let Some(error) = &self.query_error {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(error.clone())
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .into_any_element()
        } else if let Some(result) = &self.result {
            render_grid(result, &self.result_scroll, "el-query-grid")
        } else if self.connections.is_empty() {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "No connections yet — run el: initialize workspace to create el/connections.yml.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "Pick a connection and run a query — or click a table in the EL panel.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(toolbar)
            .child(editor)
            .child(body)
            .into_any_element()
    }
}

impl ElRunsPanel {
    fn render_remote(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let chips = h_flex().gap_1().flex_wrap().children(
            self.remotes
                .iter()
                .enumerate()
                .map(|(ix, name)| {
                    Button::new(("el-remote", ix), name.clone())
                        .label_size(LabelSize::Small)
                        .toggle_state(self.selected_remote == Some(ix))
                        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_remote = Some(ix);
                            this.remote_pipelines.clear();
                            this.remote_runs.clear();
                            this.remote_action_error = None;
                            this.remote_detail = None;
                            this.deploy_armed = false;
                            this.start_remote_poll(cx);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>(),
        );
        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .child(chips)
            .child(div().flex_1())
            .children(self.remote_health.clone().map(|health| {
                Label::new(health).size(LabelSize::XSmall).color(Color::Success)
            }))
            .children(self.remote_error.clone().map(|error| {
                Label::new(error).size(LabelSize::XSmall).color(Color::Error)
            }))
            .children(self.remote_action_error.clone().map(|error| {
                Label::new(error).size(LabelSize::XSmall).color(Color::Error)
            }))
            .child({
                let remote = self
                    .selected_remote
                    .and_then(|ix| self.remotes.get(ix))
                    .cloned()
                    .unwrap_or_else(|| "remote".into());
                let profile = self
                    .remote_profile
                    .clone()
                    .unwrap_or_else(|| "base connections".into());
                let only = self.remote_detail.clone();
                let label: SharedString = if self.deploy_armed {
                    match &only {
                        Some(name) => {
                            format!("Confirm: {name} → {remote} ({profile})").into()
                        }
                        None => format!("Confirm: all → {remote} ({profile})").into(),
                    }
                } else if only.is_some() {
                    "Deploy this pipeline".into()
                } else {
                    "Deploy pipelines".into()
                };
                Button::new("el-remote-deploy", label)
                    .label_size(LabelSize::XSmall)
                    .style(if self.deploy_armed {
                        ButtonStyle::Tinted(ui::TintColor::Warning)
                    } else {
                        ButtonStyle::Subtle
                    })
                    .tooltip(ui::Tooltip::text(
                        "Nothing runs on a remote until you deploy it there",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.deploy_armed {
                            let only = this.remote_detail.as_ref().map(|s| s.to_string());
                            this.deploy_to_remote(only, cx);
                        } else {
                            this.deploy_armed = true;
                        }
                        cx.notify();
                    }))
            })
            .child(
                Button::new("el-remote-logs", "Logs")
                    .label_size(LabelSize::XSmall)
                    .toggle_state(self.remote_show_logs)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_show_logs = !this.remote_show_logs;
                        cx.notify();
                    })),
            );

        let body: gpui::AnyElement = if self.remote_pipelines.is_empty()
            && self.remote_runs.is_empty()
            && self.remote_error.is_none()
        {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new("Connecting to the remote…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else if let Some(detail) = self.remote_detail.clone() {
            self.render_remote_detail(&detail, cx)
        } else {
            self.render_remote_overview(cx)
        };

        if self.remote_show_logs {
            let mut log_pane = v_flex()
                .id("el-remote-logs-pane")
                .h(px(140.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .border_t_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .px_2()
                .py_1();
            if self.remote_logs.is_empty() {
                log_pane = log_pane.child(
                    Label::new("No daemon activity yet.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
            }
            for line in self.remote_logs.iter().rev().take(200) {
                log_pane = log_pane.child(
                    Label::new(line.clone()).size(LabelSize::XSmall).color(Color::Muted),
                );
            }
            return v_flex()
                .size_full()
                .child(toolbar)
                .child(body)
                .child(log_pane)
                .into_any_element();
        }
        v_flex().size_full().child(toolbar).child(body).into_any_element()
    }

    /// The overview: pipelines as a striped table (click a row for its
    /// detail view), recent runs below.
    fn render_remote_overview(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let head = |width: f32, text: &'static str| {
            div().w(px(width)).flex_shrink_0().child(
                Label::new(text).size(LabelSize::XSmall).color(Color::Accent),
            )
        };
        let pipeline_header = h_flex()
            .w_full()
            .px_2()
            .gap_2()
            .child(head(150., "pipeline"))
            .child(head(70., "streams"))
            .child(head(150., "schedule"))
            .child(head(90., "next run"))
            .child(head(70., "state"));
        let mut pipelines = v_flex().w_full().px_1();
        for (ix, pipeline) in self.remote_pipelines.iter().enumerate() {
            let name: SharedString = pipeline.name.clone().into();
            let run_name = pipeline.name.clone();
            let cell = |width: f32, text: String, color: Color| {
                div().w(px(width)).flex_shrink_0().overflow_hidden().child(
                    Label::new(text).size(LabelSize::XSmall).color(color).truncate(),
                )
            };
            let open_name = name.clone();
            pipelines = pipelines.child(
                h_flex()
                    .id(("el-remote-pipeline", ix))
                    .w_full()
                    .h(px(26.))
                    .px_1()
                    .gap_2()
                    .items_center()
                    .rounded_sm()
                    .when(ix % 2 == 1, |row| row.bg(colors.element_background))
                    .hover(|style| style.bg(colors.element_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_detail = Some(open_name.clone());
                        this.deploy_armed = false;
                        cx.notify();
                    }))
                    .child(cell(150., pipeline.name.clone(), Color::Default))
                    .child(cell(70., pipeline.streams.to_string(), Color::Muted))
                    .child(cell(
                        150.,
                        pipeline.schedule.clone().unwrap_or_else(|| "manual".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        90.,
                        pipeline
                            .next_run_unix
                            .map(|next| relative_time(next, true))
                            .unwrap_or_else(|| "—".into()),
                        Color::Muted,
                    ))
                    .child(cell(
                        70.,
                        if pipeline.running { "running".into() } else { "idle".into() },
                        if pipeline.running { Color::Accent } else { Color::Muted },
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new(("el-remote-run", ix), "Run")
                            .label_size(LabelSize::XSmall)
                            .disabled(pipeline.running)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let name = run_name.clone();
                                this.remote_action(
                                    move |client| client.start_run(&name).map(|_| ()),
                                    cx,
                                );
                            })),
                    ),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .child(pipeline_header)
            .child(pipelines)
            .child(
                div().px_2().pt_1().child(
                    Label::new("Recent runs")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(self.render_runs_table(None, cx))
            .into_any_element()
    }

    /// One pipeline's page: back button, its facts, its runs.
    fn render_remote_detail(
        &mut self,
        name: &SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let pipeline = self
            .remote_pipelines
            .iter()
            .find(|pipeline| pipeline.name == name.as_ref());
        let run_name = name.to_string();
        let running = pipeline.map(|pipeline| pipeline.running).unwrap_or(false);
        let meta: SharedString = match pipeline {
            None => "no longer on the server".into(),
            Some(pipeline) => {
                let streams = format!(
                    "{} stream{}",
                    pipeline.streams,
                    if pipeline.streams == 1 { "" } else { "s" }
                );
                match (&pipeline.schedule, pipeline.next_run_unix) {
                    (Some(schedule), Some(next)) => format!(
                        "{streams} — runs on {schedule}, next {}",
                        relative_time(next, true)
                    )
                    .into(),
                    (Some(schedule), None) => format!("{streams} — runs on {schedule}").into(),
                    (None, _) => format!("{streams} — manual runs only").into(),
                }
            }
        };
        let header = h_flex()
            .w_full()
            .px_1()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .child(
                IconButton::new("el-remote-back", IconName::ArrowLeft)
                    .icon_size(IconSize::Small)
                    .tooltip(ui::Tooltip::text("Back to pipelines"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_detail = None;
                        this.deploy_armed = false;
                        cx.notify();
                    })),
            )
            .child(Label::new(name.clone()).size(LabelSize::Small))
            .child(Label::new(meta).size(LabelSize::XSmall).color(Color::Muted))
            .child(div().flex_1())
            .children(running.then(|| {
                Label::new("running").size(LabelSize::XSmall).color(Color::Accent)
            }))
            .child(
                Button::new("el-remote-detail-run", "Run")
                    .label_size(LabelSize::XSmall)
                    .style(ButtonStyle::Filled)
                    .disabled(running || pipeline.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let name = run_name.clone();
                        this.remote_action(
                            move |client| client.start_run(&name).map(|_| ()),
                            cx,
                        );
                    })),
            );

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                div().px_2().pt_1().child(
                    Label::new("Runs of this pipeline")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(self.render_runs_table(Some(name.as_ref()), cx))
            .into_any_element()
    }

    /// The run-history table, striped; `filter` narrows to one pipeline.
    fn render_runs_table(
        &mut self,
        filter: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let colors = &colors;
        let col = |width: f32, element: gpui::AnyElement| {
            div().w(px(width)).flex_shrink_0().overflow_hidden().child(element)
        };
        let head = |width: f32, text: &'static str| {
            col(
                width,
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Accent)
                    .into_any_element(),
            )
        };
        let show_pipeline = filter.is_none();
        let mut header = h_flex().w_full().px_2().gap_2().child(head(44., "run"));
        if show_pipeline {
            header = header.child(head(130., "pipeline"));
        }
        let header = header
            .child(head(70., "status"))
            .child(head(80., "started"))
            .child(head(64., "duration"))
            .child(head(70., "rows"))
            .child(head(50., "attempt"))
            .child(
                Label::new("error")
                    .size(LabelSize::XSmall)
                    .color(Color::Accent),
            );

        let mut runs = v_flex()
            .id("el-remote-runs")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_1();
        let mut shown = 0usize;
        for run in &self.remote_runs {
            if filter.is_some_and(|name| run.pipeline != name) {
                continue;
            }
            let stripe = shown % 2 == 1;
            shown += 1;
            let status_color = match run.status.as_str() {
                "ok" => Color::Success,
                "failed" => Color::Error,
                "cancelled" => Color::Warning,
                _ => Color::Accent,
            };
            let run_id = run.id;
            let is_running = run.status == "running";
            let cell = |width: f32, text: String, color: Color| {
                col(
                    width,
                    Label::new(text)
                        .size(LabelSize::XSmall)
                        .color(color)
                        .truncate()
                        .into_any_element(),
                )
            };
            let mut row = h_flex()
                .w_full()
                .h(px(24.))
                .px_1()
                .gap_2()
                .items_center()
                .rounded_sm()
                .when(stripe, |row| row.bg(colors.element_background))
                .child(cell(44., format!("#{}", run.id), Color::Muted));
            if show_pipeline {
                row = row.child(cell(130., run.pipeline.clone(), Color::Default));
            }
            runs = runs.child(
                row.child(cell(70., run.status.clone(), status_color))
                    .child(cell(
                        80.,
                        relative_time(run.started_unix, false),
                        Color::Muted,
                    ))
                    .child(cell(
                        64.,
                        duration_text(run.started_unix, run.finished_unix),
                        Color::Muted,
                    ))
                    .child(cell(
                        70.,
                        if is_running {
                            "…".to_owned()
                        } else {
                            run.rows_written.to_string()
                        },
                        Color::Default,
                    ))
                    .child(cell(
                        50.,
                        if run.attempt == 0 {
                            "—".to_owned()
                        } else {
                            format!("retry {}", run.attempt)
                        },
                        if run.attempt == 0 { Color::Muted } else { Color::Warning },
                    ))
                    .child(
                        Label::new(run.error.clone().unwrap_or_default())
                            .size(LabelSize::XSmall)
                            .color(Color::Error)
                            .truncate(),
                    )
                    .child(div().flex_1())
                    .children(is_running.then(|| {
                        Button::new(("el-remote-cancel", run.id as usize), "Cancel")
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote_action(
                                    move |client| client.cancel(run_id),
                                    cx,
                                );
                            }))
                    })),
            );
        }
        if shown == 0 {
            runs = runs.child(div().px_1().py_1().child(
                Label::new("No runs yet — press Run above.")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            ));
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(runs)
            .into_any_element()
    }
}

/// "in 1m 20s" / "3m ago" for a unix instant, relative to now.
fn relative_time(unix: u64, future: bool) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let delta = if future {
        unix.saturating_sub(now)
    } else {
        now.saturating_sub(unix)
    };
    let text = if delta >= 3600 {
        format!("{}h {}m", delta / 3600, (delta % 3600) / 60)
    } else if delta >= 60 {
        format!("{}m {}s", delta / 60, delta % 60)
    } else {
        format!("{delta}s")
    };
    if future { format!("in {text}") } else { format!("{text} ago") }
}

fn duration_text(started: u64, finished: Option<u64>) -> String {
    let Some(finished) = finished else {
        return "…".to_owned();
    };
    let delta = finished.saturating_sub(started);
    if delta >= 60 {
        format!("{}m {}s", delta / 60, delta % 60)
    } else {
        format!("{delta}s")
    }
}

/// The shared results grid: fixed-width columns, accent header, uniform
/// rows. Used by mapping previews and query results alike.
fn render_grid(
    table: &PreviewTable,
    scroll: &UniformListScrollHandle,
    list_id: &'static str,
) -> gpui::AnyElement {
    const COL_WIDTH: f32 = 170.;
    let columns = table.columns.clone();
    let rows = table.rows.clone();
    let total = px(columns.len() as f32 * COL_WIDTH);
    let header = h_flex().w(total).flex_shrink_0().children(columns.iter().map(|column| {
        div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
            Label::new(column.clone())
                .size(LabelSize::XSmall)
                .color(Color::Accent)
                .truncate(),
        )
    }));
    let count = rows.len();
    let list = gpui::uniform_list(list_id, count, {
        move |range, _, _| {
            range
                .filter_map(|ix| rows.get(ix))
                .map(|row| {
                    h_flex()
                        .w(total)
                        .h(px(24.))
                        .flex_shrink_0()
                        .children(row.iter().map(|cell| {
                            div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
                                Label::new(cell.clone()).size(LabelSize::XSmall).truncate(),
                            )
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        }
    })
    .flex_1()
    .track_scroll(scroll);

    div()
        .id(gpui::SharedString::from(format!("{list_id}-scroll")))
        .flex_1()
        .min_h_0()
        .overflow_x_scroll()
        .child(v_flex().w(total).h_full().child(header).child(list))
        .into_any_element()
}

impl EventEmitter<PanelEvent> for ElRunsPanel {}

impl Focusable for ElRunsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ElRunsPanel {
    fn persistent_name() -> &'static str {
        "EL Runs Panel"
    }

    fn panel_key() -> &'static str {
        "ElRunsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayFilled)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("EL Console")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElRunsFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |this, _, cx| this.refresh_connections(cx));
        }
    }
}

impl Render for ElRunsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let body: gpui::AnyElement = match &self.preview {
            Some(preview) => {
                let count = preview.rows.len();
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .w_full()
                            .p_1()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                IconButton::new("el-preview-back", IconName::ArrowLeft)
                                    .icon_size(IconSize::Small)
                                    .tooltip(ui::Tooltip::text("Back"))
                                    .on_click(cx.listener(|this, _, _, cx| this.show_runs(cx))),
                            )
                            .child(Label::new(preview.title.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(format!("{count} rows"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(render_grid(preview, &self.preview_scroll, "el-preview-rows"))
                    .into_any_element()
            }
            None => {
                let tab = |id: &'static str,
                           label: &'static str,
                           surface: Surface,
                           this: &Self,
                           cx: &mut Context<Self>| {
                    Button::new(id, label)
                        .label_size(LabelSize::Small)
                        .toggle_state(this.surface == surface)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.surface = surface;
                            match surface {
                                Surface::Query => this.refresh_connections(cx),
                                Surface::Remote => {
                                    this.refresh_connections(cx);
                                    this.start_remote_poll(cx);
                                }
                                Surface::Runs => {}
                            }
                            cx.notify();
                        }))
                };
                let header = h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(tab("el-console-runs", "Runs", Surface::Runs, self, cx))
                    .child(tab("el-console-query", "Query", Surface::Query, self, cx))
                    .children((!self.remotes.is_empty()).then(|| {
                        tab("el-console-remote", "Remote", Surface::Remote, self, cx)
                    }));
                let content: gpui::AnyElement = match self.surface {
                    Surface::Runs => self.run_view.clone().into_any_element(),
                    Surface::Query => self.render_query(cx),
                    Surface::Remote => self.render_remote(cx),
                };
                v_flex()
                    .size_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(content))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElRunsPanel")
            .bg(colors.panel_background)
            .child(body)
    }
}
