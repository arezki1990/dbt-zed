//! The deploy popup: the developer clicks Deploy, the popup asks which
//! remote and — fetched live from that server — which PROFILE the
//! pipeline should run under there, and states what the deploy changes
//! on that server (new vs replacing, a run in flight, the schedule that
//! starts firing) before anything ships. A stale spec (unsaved edits,
//! parse errors) is refused with the reason.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Task, WeakEntity, Window,
};
use project::Project;
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use el_engine::server::RemotePipeline;
use el_engine::spec::Pipeline;

/// What the selected remote reports about itself.
enum RemoteFacts {
    Loading,
    Loaded {
        /// Profiles declared on the server.
        profiles: Vec<String>,
        /// The daemon's own active profile.
        active: Option<String>,
        /// What the server holds today (GET /pipelines).
        pipelines: Vec<RemotePipeline>,
        /// The daemon tracks a git checkout and refuses /deploy.
        tracks_checkout: bool,
    },
    Failed(SharedString),
}

pub struct ElDeployModal {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: PathBuf,
    pipeline: String,
    spec_path: PathBuf,
    project: Entity<Project>,
    /// The spec as parsed from disk when the popup opened.
    local: Option<Arc<Pipeline>>,
    /// Why the local spec can't ship (unsaved edits, parse error).
    refusal: Option<SharedString>,
    remotes: Vec<SharedString>,
    selected_remote: usize,
    facts: RemoteFacts,
    /// None = run under the server's own profile.
    selected_profile: Option<String>,
    deploying: bool,
    error: Option<SharedString>,
    epoch: u64,
    _fetch: Task<()>,
    _check: Task<()>,
}

impl ElDeployModal {
    pub fn deploy(
        workspace: &mut Workspace,
        root: PathBuf,
        pipeline: String,
        spec_path: PathBuf,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let remotes: Vec<SharedString> = el_engine::spec::load_remotes(
            &super::el_dir(&root).join("remotes.yml"),
        )
        .map(|remotes| remotes.remotes.keys().map(|name| name.clone().into()).collect())
        .unwrap_or_default();
        if remotes.is_empty() {
            super::toast_error(
                workspace,
                "No remotes declared — add one to el/remotes.yml first.",
                Some(super::el_dir(&root).join("remotes.yml")),
                cx,
            );
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        workspace.toggle_modal(window, cx, move |_, cx| {
            let mut this = Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                root,
                pipeline,
                spec_path,
                project,
                local: None,
                refusal: None,
                remotes,
                selected_remote: 0,
                facts: RemoteFacts::Loading,
                selected_profile: None,
                deploying: false,
                error: None,
                epoch: 0,
                _fetch: Task::ready(()),
                _check: Task::ready(()),
            };
            this.check_local(cx);
            this.fetch_facts(cx);
            this
        });
    }

    fn file_name(&self) -> String {
        self.spec_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("the spec file")
            .to_owned()
    }

    /// Why the on-disk spec must not ship right now, or None.
    fn unsaved_refusal(&self, cx: &App) -> Option<SharedString> {
        super::spec_io::has_unsaved_edits(&self.project, &self.spec_path, cx).then(|| {
            format!("{} has unsaved edits — save it, then deploy again.", self.file_name())
                .into()
        })
    }

    /// The local preflight: refuses a dirty buffer outright, then parses
    /// the file on the background executor. Reads only; never writes.
    fn check_local(&mut self, cx: &mut Context<Self>) {
        self.local = None;
        self.refusal = self.unsaved_refusal(cx);
        if self.refusal.is_some() {
            cx.notify();
            return;
        }
        let spec_path = self.spec_path.clone();
        let parse = cx.background_spawn(async move { el_engine::spec::load_pipeline(&spec_path) });
        self._check = cx.spawn(async move |this, cx| {
            let result = parse.await;
            this.update(cx, |this, cx| {
                let file = this.file_name();
                match result {
                    Ok(pipeline) if pipeline.pipeline != this.pipeline => {
                        this.refusal = Some(
                            format!(
                                "{file} names pipeline {} — deploy it from its own canvas.",
                                pipeline.pipeline
                            )
                            .into(),
                        );
                    }
                    Ok(pipeline) => this.local = Some(Arc::new(pipeline)),
                    Err(error) => {
                        this.refusal = Some(
                            format!("Can't deploy: {error} — fix {file}, then deploy again.")
                                .into(),
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Asks the selected remote which profiles it declares, which one it
    /// runs by default, and which pipelines it already holds.
    fn fetch_facts(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        let epoch = self.epoch;
        self.facts = RemoteFacts::Loading;
        self.selected_profile = None;
        let root = self.root.clone();
        let Some(remote) = self.remotes.get(self.selected_remote).cloned() else {
            return;
        };
        let task = cx.background_spawn(async move {
            let client = el_engine::server::RemoteClient::connect(&root, remote.as_ref())?;
            let health = client.health()?;
            let pipelines = client.pipelines()?;
            anyhow::Ok((health, pipelines))
        });
        self._fetch = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.facts = match result {
                    Ok((health, pipelines)) => {
                        let profiles: Vec<String> = health
                            .get("profiles")
                            .and_then(|names| names.as_array())
                            .map(|names| {
                                names
                                    .iter()
                                    .filter_map(|name| name.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let active = health
                            .get("profile")
                            .and_then(|name| name.as_str())
                            .map(str::to_owned);
                        // Older daemons don't say; /deploy still 409s then.
                        let tracks_checkout = health
                            .get("tracks_checkout")
                            .and_then(|flag| flag.as_bool())
                            .unwrap_or(false);
                        // Preselect the server's own profile when declared.
                        this.selected_profile = active
                            .clone()
                            .filter(|name| profiles.iter().any(|p| p == name));
                        RemoteFacts::Loaded { profiles, active, pipelines, tracks_checkout }
                    }
                    Err(error) => RemoteFacts::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        });
    }

    fn can_send(&self) -> bool {
        !self.deploying
            && self.refusal.is_none()
            && self.local.is_some()
            && matches!(
                self.facts,
                RemoteFacts::Loaded { tracks_checkout: false, .. }
            )
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send() {
            return;
        }
        let Some(remote) = self.remotes.get(self.selected_remote).cloned() else {
            return;
        };
        // The buffer may have changed since the popup opened.
        if let Some(refusal) = self.unsaved_refusal(cx) {
            self.local = None;
            self.refusal = Some(refusal);
            cx.notify();
            return;
        }
        self.deploying = true;
        self.error = None;
        cx.notify();
        let root = self.root.clone();
        let name = self.pipeline.clone();
        let spec_path = self.spec_path.clone();
        let profile = self.selected_profile.clone();
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task_root = root.clone();
            let task_name = name.clone();
            let task_remote = remote.to_string();
            let task_profile = profile.clone();
            let result = cx
                .background_spawn(async move {
                    // Ship the file as it is on disk, re-validated now.
                    let yaml = std::fs::read_to_string(&spec_path)
                        .context("reading the pipeline")?;
                    let parsed = el_engine::spec::load_pipeline(&spec_path)?;
                    if parsed.pipeline != task_name {
                        anyhow::bail!(
                            "{} names pipeline {:?}, not {task_name:?} — deploy it from \
                             its own canvas",
                            spec_path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("the spec file"),
                            parsed.pipeline
                        );
                    }
                    let client = el_engine::server::RemoteClient::connect(
                        &task_root,
                        &task_remote,
                    )?;
                    client.deploy(&[(task_name, yaml, task_profile)])?;
                    anyhow::Ok(())
                })
                .await;
            match result {
                Ok(()) => {
                    // Dismiss first: hiding the modal restores the focus it
                    // took, which would otherwise steal the console back.
                    this.update_in(cx, |this, _, cx| {
                        this.deploying = false;
                        cx.emit(DismissEvent);
                        cx.notify();
                    })
                    .ok();
                    let profile_text = profile
                        .clone()
                        .unwrap_or_else(|| "the server's profile".to_owned());
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            super::toast(
                                workspace,
                                &format!(
                                    "Deployed {name} to {remote} — runs under {profile_text}."
                                ),
                                cx,
                            );
                            workspace.focus_panel::<super::ElRunsPanel>(window, cx);
                        })
                        .ok();
                    // The console refreshes by reading the workspace —
                    // update it only after the lease above is released.
                    let console = workspace
                        .read_with(cx, |workspace, cx| workspace.panel::<super::ElRunsPanel>(cx))
                        .ok()
                        .flatten();
                    if let Some(console) = console {
                        console.update(cx, |panel, cx| {
                            panel.show_remote_pipeline(remote.clone(), name.clone().into(), cx)
                        });
                    }
                }
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        this.deploying = false;
                        this.error = Some(format!("{error:#}").into());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// The consequence lines: what pressing Deploy would do on `remote`.
    fn render_consequences(
        &self,
        remote: &SharedString,
        pipelines: &[RemotePipeline],
        tracks_checkout: bool,
    ) -> Div {
        let mut rows = v_flex().w_full().px_2().pb_2().gap_0p5();
        let line = |text: String, color: Color| {
            Label::new(text).size(LabelSize::XSmall).color(color)
        };
        if let Some(refusal) = &self.refusal {
            return rows.child(line(refusal.to_string(), Color::Error));
        }
        if tracks_checkout {
            rows = rows.child(line(
                format!("{remote} tracks its git checkout — deploy by pushing to it, not from here."),
                Color::Error,
            ));
        }
        let Some(local) = &self.local else {
            return rows.child(line(format!("Reading {}…", self.file_name()), Color::Muted));
        };
        let pf = el_engine::server::deploy_preflight(
            local,
            pipelines,
            self.selected_profile.as_deref(),
        );
        let profile_text = match &self.selected_profile {
            Some(profile) => format!("profile {profile}"),
            None => "the server default profile".to_owned(),
        };
        match &pf.replacing {
            None => {
                rows = rows.child(line(
                    format!("New on {remote} — nothing runs there until you deploy."),
                    Color::Muted,
                ));
            }
            Some(replaced) => {
                let when = replaced
                    .deployed_unix
                    .map(|unix| super::runs_panel::relative_time(unix, false))
                    .unwrap_or_else(|| "earlier".to_owned());
                let pinned = replaced
                    .previous_profile
                    .as_deref()
                    .map(|profile| format!("profile {profile}"))
                    .unwrap_or_else(|| "the server default profile".to_owned());
                rows = rows.child(line(
                    format!("Replaces the copy deployed {when}, pinned to {pinned}."),
                    Color::Muted,
                ));
                if replaced.profile_changes {
                    let previous = replaced
                        .previous_profile
                        .as_deref()
                        .unwrap_or("server default");
                    let next = self.selected_profile.as_deref().unwrap_or("server default");
                    rows = rows.child(line(
                        format!("Profile changes: {previous} → {next}."),
                        Color::Warning,
                    ));
                }
                if let (Some(previous), None) = (&replaced.previous_schedule, &pf.schedule) {
                    rows = rows.child(line(
                        format!("The current schedule {previous} stops — manual runs only afterwards."),
                        Color::Warning,
                    ));
                }
            }
        }
        if pf.running {
            rows = rows.child(line(
                format!(
                    "A run of {} is in progress on {remote} — it finishes on the old spec; \
                     the new spec applies from the next run.",
                    self.pipeline
                ),
                Color::Warning,
            ));
        }
        match &pf.schedule {
            Some((cron, timezone)) => {
                let first = match pf.first_fire_unix {
                    Some(unix) => format!(
                        "first run {}",
                        super::runs_panel::relative_time(unix, true)
                    ),
                    None => "the server can't parse it and will skip it".to_owned(),
                };
                rows = rows.child(line(
                    format!(
                        "Schedule {cron} ({timezone}) starts firing on {remote} under \
                         {profile_text} — {first}."
                    ),
                    if pf.first_fire_unix.is_some() { Color::Muted } else { Color::Warning },
                ));
            }
            None => {
                rows = rows.child(line(
                    format!("No schedule — manual runs only on {remote}."),
                    Color::Muted,
                ));
            }
        }
        rows
    }
}

impl EventEmitter<DismissEvent> for ElDeployModal {}
impl ModalView for ElDeployModal {}

impl Focusable for ElDeployModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ElDeployModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let remote = self
            .remotes
            .get(self.selected_remote)
            .cloned()
            .unwrap_or_else(|| "remote".into());

        let mut card = v_flex()
            .key_context("ElDeployModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(460.))
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
                    .child(
                        Label::new(format!("Deploy {}", self.pipeline))
                            .size(LabelSize::Small),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-deploy-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            );

        if self.remotes.len() > 1 {
            let mut remote_row = h_flex().w_full().px_2().pt_2().gap_1().flex_wrap().child(
                Label::new("server").size(LabelSize::XSmall).color(Color::Muted),
            );
            for (ix, name) in self.remotes.iter().enumerate() {
                let selected = ix == self.selected_remote;
                remote_row = remote_row.child(
                    Button::new(("el-deploy-remote", ix), name.clone())
                        .label_size(LabelSize::XSmall)
                        .toggle_state(selected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_remote = ix;
                            this.fetch_facts(cx);
                            cx.notify();
                        })),
                );
            }
            card = card.child(remote_row);
        }

        card = card.child(match &self.facts {
            RemoteFacts::Loading => div().p_2().child(
                Label::new(format!("Checking {remote}…"))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            ),
            RemoteFacts::Failed(error) => div().p_2().child(
                Label::new(error.clone()).size(LabelSize::XSmall).color(Color::Error),
            ),
            RemoteFacts::Loaded { profiles, active, .. } => {
                let mut rows = v_flex().w_full().p_2().gap_1();
                rows = rows.child(
                    Label::new("Run this pipeline under:")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
                let mut chips = h_flex().w_full().gap_1().flex_wrap();
                let server_default_label: SharedString = match active {
                    Some(active) => format!("server default ({active})").into(),
                    None => "server default".into(),
                };
                chips = chips.child(
                    Button::new("el-deploy-profile-default", server_default_label)
                        .label_size(LabelSize::XSmall)
                        .toggle_state(self.selected_profile.is_none())
                        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_profile = None;
                            cx.notify();
                        })),
                );
                for (ix, profile) in profiles.iter().enumerate() {
                    let selected = self.selected_profile.as_deref() == Some(profile);
                    let profile = profile.clone();
                    chips = chips.child(
                        Button::new(("el-deploy-profile", ix), SharedString::from(profile.clone()))
                            .label_size(LabelSize::XSmall)
                            .toggle_state(selected)
                            .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_profile = Some(profile.clone());
                                cx.notify();
                            })),
                    );
                }
                rows = rows.child(chips);
                if profiles.is_empty() {
                    rows = rows.child(
                        Label::new(
                            "This server declares no profiles — it runs its base \
                             connections.",
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    );
                }
                rows
            }
        });

        // What the deploy changes on the server, or why it can't ship.
        match &self.facts {
            RemoteFacts::Loaded { pipelines, tracks_checkout, .. } => {
                card = card.child(self.render_consequences(&remote, pipelines, *tracks_checkout));
            }
            _ => {
                if let Some(refusal) = &self.refusal {
                    card = card.child(div().px_2().pb_2().child(
                        Label::new(refusal.clone()).size(LabelSize::XSmall).color(Color::Error),
                    ));
                }
            }
        }

        if let Some(error) = &self.error {
            card = card.child(div().px_2().pb_1().child(
                Label::new(error.clone()).size(LabelSize::XSmall).color(Color::Error),
            ));
        }

        let deploy_label: SharedString = if self.deploying {
            "Deploying…".into()
        } else {
            match &self.selected_profile {
                Some(profile) => format!("Deploy to {remote} ({profile})").into(),
                None => format!("Deploy to {remote}").into(),
            }
        };
        card.child(
            h_flex()
                .w_full()
                .p_2()
                .gap_1()
                .border_t_1()
                .border_color(colors.border)
                .child(div().flex_1())
                .child(
                    Button::new("el-deploy-cancel", "Cancel")
                        .label_size(LabelSize::Small)
                        .disabled(self.deploying)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                )
                .child(
                    Button::new("el-deploy-send", deploy_label)
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Filled)
                        .disabled(!self.can_send())
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                ),
        )
    }
}
