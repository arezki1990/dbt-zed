//! The deploy popup: the developer clicks Deploy, the popup asks which
//! remote and — fetched live from that server — which PROFILE the
//! pipeline should run under there. Nothing ships until "Deploy" is
//! pressed with both choices visible.

use std::path::PathBuf;

use anyhow::Context as _;
use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, SharedString, Task,
    WeakEntity, Window,
};
use ui::prelude::*;
use workspace::{ModalView, Workspace};

/// What the selected remote reports about itself.
enum RemoteFacts {
    Loading,
    Loaded {
        /// Profiles declared on the server.
        profiles: Vec<String>,
        /// The daemon's own active profile.
        active: Option<String>,
    },
    Failed(SharedString),
}

pub struct ElDeployModal {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: PathBuf,
    pipeline: String,
    remotes: Vec<SharedString>,
    selected_remote: usize,
    facts: RemoteFacts,
    /// None = run under the server's own profile.
    selected_profile: Option<String>,
    deploying: bool,
    error: Option<SharedString>,
    epoch: u64,
    _fetch: Task<()>,
}

impl ElDeployModal {
    pub fn deploy(
        workspace: &mut Workspace,
        root: PathBuf,
        pipeline: String,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let remotes: Vec<SharedString> = el_engine::spec::load_remotes(
            &super::el_dir(&root).join("remotes.yml"),
        )
        .map(|remotes| remotes.remotes.keys().map(|name| name.clone().into()).collect())
        .unwrap_or_default();
        if remotes.is_empty() {
            super::toast(
                workspace,
                "No remotes declared — add one to el/remotes.yml first.",
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
                remotes,
                selected_remote: 0,
                facts: RemoteFacts::Loading,
                selected_profile: None,
                deploying: false,
                error: None,
                epoch: 0,
                _fetch: Task::ready(()),
            };
            this.fetch_facts(cx);
            this
        });
    }

    /// Asks the selected remote which profiles it declares and which one
    /// it runs by default.
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
            client.health()
        });
        self._fetch = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.facts = match result {
                    Ok(health) => {
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
                        // Preselect the server's own profile when declared.
                        this.selected_profile = active
                            .clone()
                            .filter(|name| profiles.iter().any(|p| p == name));
                        RemoteFacts::Loaded { profiles, active }
                    }
                    Err(error) => RemoteFacts::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        });
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.deploying {
            return;
        }
        let Some(remote) = self.remotes.get(self.selected_remote).cloned() else {
            return;
        };
        self.deploying = true;
        self.error = None;
        cx.notify();
        let root = self.root.clone();
        let name = self.pipeline.clone();
        let profile = self.selected_profile.clone();
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let task_root = root.clone();
            let task_name = name.clone();
            let task_remote = remote.to_string();
            let task_profile = profile.clone();
            let result = cx
                .background_spawn(async move {
                    // Resolve the pipeline's file by its spec name.
                    let mut yaml = None;
                    for path in el_engine::spec::list_pipelines(&super::el_dir(&task_root)) {
                        if el_engine::spec::load_pipeline(&path)
                            .map(|pipeline| pipeline.pipeline == task_name)
                            .unwrap_or(false)
                        {
                            yaml = Some(
                                std::fs::read_to_string(&path)
                                    .context("reading the pipeline")?,
                            );
                            break;
                        }
                    }
                    let yaml = yaml.with_context(|| {
                        format!("no local pipeline named {task_name:?}")
                    })?;
                    let client = el_engine::server::RemoteClient::connect(
                        &task_root,
                        &task_remote,
                    )?;
                    client.deploy(&[(task_name, yaml, task_profile)])?;
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |this, cx| {
                this.deploying = false;
                match result {
                    Ok(()) => {
                        let profile_text = profile
                            .clone()
                            .unwrap_or_else(|| "the server's profile".to_owned());
                        workspace
                            .update(cx, |workspace, cx| {
                                super::toast(
                                    workspace,
                                    &format!(
                                        "Deployed {name} to {remote} — runs under \
                                         {profile_text}."
                                    ),
                                    cx,
                                );
                                if let Some(panel) =
                                    workspace.panel::<super::ElRunsPanel>(cx)
                                {
                                    panel.update(cx, |panel, cx| {
                                        panel.profile_changed(cx)
                                    });
                                }
                            })
                            .ok();
                        cx.emit(DismissEvent);
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
            .w(px(440.))
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
                Label::new(format!("Asking {remote} about its profiles…"))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            ),
            RemoteFacts::Failed(error) => div().p_2().child(
                Label::new(error.clone()).size(LabelSize::XSmall).color(Color::Error),
            ),
            RemoteFacts::Loaded { profiles, active } => {
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
                        .disabled(
                            self.deploying
                                || !matches!(self.facts, RemoteFacts::Loaded { .. }),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                ),
        )
    }
}
