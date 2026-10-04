//! The deploy popup: the developer clicks Deploy, the popup asks which
//! remote and — fetched live from that server — which PROFILE the
//! pipeline should run under there, and states what the deploy changes
//! on that server (new vs replacing, a run in flight, the schedule that
//! starts firing) before anything ships. A stale spec (unsaved edits,
//! parse errors) is refused with the reason.
//!
//! A pipeline names connections; a server that does not have them can
//! only fail the run. So the deploy also carries the project's
//! connections file and the variables it names — sealed to the key that
//! server publishes, which no one else can open. The popup names the
//! variables and shows the key's fingerprint; it never shows a value.
//!
//! A project of the Ellode platform keeps its connections itself, and
//! keeps drafts: there the file is saved as the project's draft of the
//! pipeline and deployed from it — the console then shows who deployed
//! which draft — or, by choice, deployed directly under a pinned profile.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Task,
    WeakEntity, Window,
};
use project::Project;
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use super::dialog;
use super::platform::{self, PlatformRemote};

use el_engine::server::{RemoteDraft, RemotePipeline, RemoteRefusal, Sealing};
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
        /// How the server takes the project's connections: sealed to a
        /// key whose fingerprint a person compares, never (the platform
        /// keeps its own), or not at all (a daemon older than /pubkey).
        sealing: Sealing,
        /// On the platform, the pipeline's draft in the project's
        /// workspace — None when it has none — or why the workspace can't
        /// be used. None for a daemon, which has no workspace.
        workspace: Option<Result<Option<RemoteDraft>, SharedString>>,
    },
    Failed(SharedString),
}

/// How a platform deploy ships the file.
#[derive(Clone, Copy, PartialEq)]
enum Route {
    /// Saved as the project's draft, then deployed from it.
    Draft,
    /// As it is, to `/deploy`, under the profile chosen.
    Direct,
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
    /// Its text, to tell whether the project's draft already is this file.
    local_text: Option<String>,
    /// Why the local spec can't ship (unsaved edits, parse error).
    refusal: Option<SharedString>,
    remotes: Vec<SharedString>,
    /// The remotes that are platform projects reached through a console.
    platforms: HashMap<SharedString, PlatformRemote>,
    selected_remote: usize,
    facts: RemoteFacts,
    /// On the platform: from the project's draft, or directly.
    route: Route,
    /// None = run under the server's own profile.
    selected_profile: Option<String>,
    /// The connections file and the variables it names, read from the
    /// project when the popup opened. Values stay inside it.
    connections: Option<Arc<el_engine::secrets::ConnectionsBundle>>,
    connections_error: Option<SharedString>,
    /// Whether this deploy carries them. Off leaves whatever the server
    /// already has.
    send_connections: bool,
    deploying: bool,
    error: Option<SharedString>,
    epoch: u64,
    _fetch: Task<()>,
    _check: Task<()>,
    _connections: Task<()>,
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
        let declared = el_engine::spec::load_remotes(&super::el_dir(&root).join("remotes.yml"))
            .map(|remotes| remotes.remotes)
            .unwrap_or_default();
        let remotes: Vec<SharedString> = declared.keys().map(|name| name.clone().into()).collect();
        let platforms: HashMap<SharedString, PlatformRemote> = declared
            .iter()
            .filter_map(|(name, remote)| {
                platform::platform_remote(&remote.url)
                    .map(|platform| (SharedString::from(name.clone()), platform))
            })
            .collect();
        if remotes.is_empty() {
            // Same action name as the panel's "+": open the Add server
            // wizard right away instead of pointing at a file to hand-edit.
            super::toast(workspace, "No servers yet — add one to deploy to.", cx);
            if let Some(panel) = workspace.panel::<super::ElPanel>(cx) {
                super::remote_modal::ElRemoteModal::deploy(
                    workspace,
                    panel.downgrade(),
                    root,
                    None,
                    window,
                    cx,
                );
            }
            return;
        }
        // A console project's connections are its admins': sent only when asked.
        let send_connections = remotes
            .first()
            .is_none_or(|first| !platforms.contains_key(first));
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
                local_text: None,
                refusal: None,
                remotes,
                platforms,
                selected_remote: 0,
                facts: RemoteFacts::Loading,
                route: Route::Direct,
                selected_profile: None,
                connections: None,
                connections_error: None,
                send_connections,
                deploying: false,
                error: None,
                epoch: 0,
                _fetch: Task::ready(()),
                _check: Task::ready(()),
                _connections: Task::ready(()),
            };
            this.check_local(cx);
            this.read_connections(cx);
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
            format!(
                "{} has unsaved edits — save it, then deploy again.",
                self.file_name()
            )
            .into()
        })
    }

    /// The local preflight: refuses a dirty buffer outright, then parses
    /// the file on the background executor. Reads only; never writes.
    fn check_local(&mut self, cx: &mut Context<Self>) {
        self.local = None;
        self.local_text = None;
        self.refusal = self.unsaved_refusal(cx);
        if self.refusal.is_some() {
            cx.notify();
            return;
        }
        let spec_path = self.spec_path.clone();
        let parse = cx.background_spawn(async move {
            let text = std::fs::read_to_string(&spec_path).ok();
            (el_engine::spec::load_pipeline(&spec_path), text)
        });
        self._check = cx.spawn(async move |this, cx| {
            let (result, text) = parse.await;
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
                    Ok(pipeline) => {
                        this.local = Some(Arc::new(pipeline));
                        this.local_text = text;
                    }
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

    /// Reads the project's connections and resolves the variables they
    /// name, off the main thread — dotenv files are on disk. Values never
    /// leave the bundle.
    fn read_connections(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let read = cx
            .background_spawn(async move { el_engine::secrets::ConnectionsBundle::read(&root) });
        self._connections = cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(bundle) => this.connections = Some(Arc::new(bundle)),
                    Err(error) => this.connections_error = Some(format!("{error:#}").into()),
                }
                // The profiles on offer may have just changed under it.
                this.settle_profile();
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
        let pipeline = self.pipeline.clone();
        let Some(remote) = self.remotes.get(self.selected_remote).cloned() else {
            return;
        };
        let is_platform = self.selected_platform().is_some();
        let task = cx.background_spawn(async move {
            let client = el_engine::server::RemoteClient::connect(&root, remote.as_ref())?;
            let health = client.health()?;
            let pipelines = client.pipelines()?;
            // A daemon older than /pubkey, or the platform keeping its own
            // connections, is an answer; anything else is a failure to say.
            let sealing = client.sealing()?;
            // Only the platform keeps drafts: a console's project, or a
            // platform reached directly that keeps its connections itself.
            let workspace = (is_platform || matches!(sealing, Sealing::KeepsItsOwn(_))).then(|| {
                client
                    .draft(&pipeline)
                    .map_err(|error| SharedString::from(format!("{error:#}")))
            });
            anyhow::Ok((health, pipelines, sealing, workspace))
        });
        self._fetch = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.facts = match result {
                    Ok((health, pipelines, sealing, workspace)) => {
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
                        // On the platform, the project's draft is the way
                        // when its workspace answers.
                        this.route = if matches!(workspace, Some(Ok(_))) {
                            Route::Draft
                        } else {
                            Route::Direct
                        };
                        // A deploy that carries connections leaves the
                        // pin empty: "the server's own" then names the
                        // environment and keeps naming it if the server's
                        // choice moves. So does one from a draft, which
                        // pins nothing. Otherwise pin what it runs today.
                        let sends = this.send_connections && matches!(sealing, Sealing::Key { .. });
                        this.selected_profile = if sends || this.route == Route::Draft {
                            None
                        } else {
                            active
                                .clone()
                                .filter(|name| profiles.iter().any(|p| p == name))
                        };
                        RemoteFacts::Loaded {
                            profiles,
                            active,
                            pipelines,
                            tracks_checkout,
                            sealing,
                            workspace,
                        }
                    }
                    Err(error) => RemoteFacts::Failed(this.failure_text(&error).into()),
                };
                cx.notify();
            })
            .ok();
        });
    }

    /// The selected remote's console and project, when it is a platform
    /// project reached through a console.
    fn selected_platform(&self) -> Option<&PlatformRemote> {
        self.remotes
            .get(self.selected_remote)
            .and_then(|name| self.platforms.get(name))
    }

    /// A failure as the popup says it: a console's refusal names the token
    /// or the role, after the step it stopped — "saved as the project's
    /// draft, but not deployed"; anything else reads as it is.
    fn failure_text(&self, error: &anyhow::Error) -> String {
        let refusal = self
            .selected_platform()
            .and_then(|platform| platform::refusal_text(platform, error));
        match refusal {
            Some(text) if error.chain().count() > 1 => format!("{error}: {text}"),
            Some(text) => text,
            None => format!("{error:#}"),
        }
    }

    /// Whether this deploy carries the project's connections: chosen, not
    /// through the project's draft, and the server takes them sealed.
    fn sends_connections(&self) -> bool {
        self.send_connections
            && !self.through_draft()
            && matches!(
                self.facts,
                RemoteFacts::Loaded {
                    sealing: Sealing::Key { .. },
                    ..
                }
            )
    }

    /// Whether this deploy goes through the project's draft: chosen, and
    /// the platform's workspace answered.
    fn through_draft(&self) -> bool {
        self.route == Route::Draft
            && matches!(
                self.facts,
                RemoteFacts::Loaded {
                    workspace: Some(Ok(_)),
                    ..
                }
            )
    }

    /// Which profiles this deploy can pin, and which one the server falls
    /// back to for a pipeline that pins none.
    ///
    /// When the deploy carries connections, that is the file the server
    /// will be running — so its profiles are the ones to offer, not the
    /// ones the server declares this second. Otherwise the server's own.
    fn profile_choices(&self) -> (Vec<String>, Option<String>, bool) {
        if self.sends_connections() {
            if let Some(bundle) = &self.connections {
                return (
                    bundle.profiles.clone(),
                    bundle.default_profile.clone(),
                    true,
                );
            }
        }
        match &self.facts {
            RemoteFacts::Loaded {
                profiles, active, ..
            } => (profiles.clone(), active.clone(), false),
            _ => (Vec::new(), None, false),
        }
    }

    /// Keeps the pinned profile honest when the choice of source moves —
    /// a name the server will not have must not stay selected.
    fn settle_profile(&mut self) {
        let (profiles, _, _) = self.profile_choices();
        if let Some(selected) = &self.selected_profile {
            if !profiles.iter().any(|profile| profile == selected) {
                self.selected_profile = None;
            }
        }
    }

    fn can_send(&self) -> bool {
        !self.deploying
            && self.refusal.is_none()
            && self.local.is_some()
            && matches!(
                self.facts,
                RemoteFacts::Loaded {
                    tracks_checkout: false,
                    ..
                }
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
        // What the credentials travel sealed to, and what travels — None
        // for either means this deploy carries the pipeline alone.
        let sealing = self
            .sends_connections()
            .then(|| match (&self.facts, &self.connections) {
                (
                    RemoteFacts::Loaded {
                        sealing: Sealing::Key { public_key, .. },
                        ..
                    },
                    Some(bundle),
                ) => Some((public_key.clone(), bundle.clone())),
                _ => None,
            })
            .flatten();
        // Through the project's draft: the one there when the popup looked,
        // which a save replaces only if it is still that one.
        let draft = match (&self.facts, self.through_draft()) {
            (
                RemoteFacts::Loaded {
                    workspace: Some(Ok(draft)),
                    ..
                },
                true,
            ) => Some(draft.clone()),
            _ => None,
        };
        let through_draft = draft.is_some();
        cx.spawn_in(window, async move |this, cx| {
            let task_root = root.clone();
            let task_name = name.clone();
            let task_remote = remote.to_string();
            let task_profile = profile.clone();
            let task_sealing = sealing.clone();
            let result = cx
                .background_spawn(async move {
                    // Ship the file as it is on disk, re-validated now.
                    let yaml =
                        std::fs::read_to_string(&spec_path).context("reading the pipeline")?;
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
                    let client =
                        el_engine::server::RemoteClient::connect(&task_root, &task_remote)?;
                    if let Some(base) = draft {
                        return deploy_through_draft(&client, &task_name, &yaml, base)
                            .map(Shipped::Draft);
                    }
                    // Credentials first: a pipeline deployed against
                    // connections the server does not have can only fail
                    // its next run.
                    let sent = match task_sealing {
                        Some((key, bundle)) => {
                            let sealed = bundle.seal(&key).context("sealing the connections")?;
                            Some(
                                client
                                    .send_connections(&sealed)
                                    .context("sending the connections")?,
                            )
                        }
                        None => None,
                    };
                    client.deploy(&[(task_name, yaml, task_profile)])?;
                    anyhow::Ok(Shipped::Direct(sent))
                })
                .await;
            match result {
                Ok(Shipped::Draft(saved)) => {
                    this.update_in(cx, |this, _, cx| {
                        this.deploying = false;
                        cx.emit(DismissEvent);
                        cx.notify();
                    })
                    .ok();
                    let how = if saved {
                        "saved as the project's draft and deployed from it"
                    } else {
                        "deployed from the project's draft, which already was this file"
                    };
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            super::toast(
                                workspace,
                                &format!(
                                    "{name} on {remote}: {how} — it runs under the project's \
                                     profile."
                                ),
                                cx,
                            );
                            workspace.focus_panel::<super::ElRunsPanel>(window, cx);
                        })
                        .ok();
                    let console = workspace
                        .read_with(cx, |workspace, cx| {
                            workspace.panel::<super::ElRunsPanel>(cx)
                        })
                        .ok()
                        .flatten();
                    if let Some(console) = console {
                        console.update(cx, |panel, cx| {
                            panel.show_remote_pipeline(remote.clone(), name.clone().into(), cx)
                        });
                    }
                }
                Ok(Shipped::Direct(sent)) => {
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
                    let credentials = match sent {
                        Some((connections, variables, files)) => {
                            let carried = if files > 0 {
                                format!(
                                    " and {files} credential file{}",
                                    if files == 1 { "" } else { "s" }
                                )
                            } else {
                                String::new()
                            };
                            format!(
                                " Sent {connections} connection{}, {variables} variable{}{carried}, \
                                 sealed.",
                                if connections == 1 { "" } else { "s" },
                                if variables == 1 { "" } else { "s" },
                            )
                        }
                        None => String::new(),
                    };
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            super::toast(
                                workspace,
                                &format!(
                                    "Deployed {name} to {remote} — runs under                                      {profile_text}.{credentials}"
                                ),
                                cx,
                            );
                            workspace.focus_panel::<super::ElRunsPanel>(window, cx);
                        })
                        .ok();
                    // The console refreshes by reading the workspace —
                    // update it only after the lease above is released.
                    let console = workspace
                        .read_with(cx, |workspace, cx| {
                            workspace.panel::<super::ElRunsPanel>(cx)
                        })
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
                        this.error = Some(this.failure_text(&error).into());
                        // The project's draft may have moved, or been saved
                        // before the deploy was refused: look again.
                        if through_draft {
                            this.fetch_facts(cx);
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// The credentials half: what travels, sealed to which key. Names
    /// and counts only — a value never reaches this popup's surface.
    fn render_connections(
        &self,
        remote: &SharedString,
        sealing: &Sealing,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fingerprint = match sealing {
            Sealing::Key { fingerprint, .. } => fingerprint,
            Sealing::KeepsItsOwn(_) => {
                return dialog::labeled(
                    "Connections",
                    dialog::notice(
                        Severity::Info,
                        format!(
                            "{remote} keeps the project's connections and secrets itself: they \
                             are set in the console, never sent from here, and the pipeline \
                             runs against them."
                        ),
                    ),
                );
            }
            Sealing::Unsupported => {
                return dialog::labeled(
                    "Connections",
                    dialog::notice(
                        Severity::Warning,
                        format!(
                            "{remote} is older than sealed connections — this deploy carries \
                             the pipeline only, and the server runs it against whatever \
                             connections it already has."
                        ),
                    ),
                );
            }
        };
        let platform = self.selected_platform().is_some();
        if self.through_draft() {
            return dialog::labeled(
                "Connections",
                dialog::notice(
                    Severity::Info,
                    format!(
                        "A deploy from the project's draft sends no connections: the pipeline \
                         runs against the ones {remote}'s project has. Directly sends them, and \
                         so does Send connections on the Remote tab."
                    ),
                ),
            );
        }
        if let Some(error) = &self.connections_error {
            return dialog::labeled(
                "Connections",
                dialog::notice(
                    Severity::Error,
                    format!("Can't read this project's connections: {error}"),
                ),
            );
        }
        let Some(bundle) = &self.connections else {
            return dialog::labeled("Connections", dialog::loading("Reading el/connections.yml…"));
        };
        let send_label: SharedString = format!(
            "Send {} connection{} · {} variable{}",
            bundle.connections,
            if bundle.connections == 1 { "" } else { "s" },
            bundle.variables.len(),
            if bundle.variables.len() == 1 { "" } else { "s" },
        )
        .into();
        let chips = dialog::chips([
            dialog::chip("el-deploy-conn-send", send_label, self.send_connections)
                .disabled(self.deploying)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.send_connections = true;
                    this.settle_profile();
                    cx.notify();
                })),
            dialog::chip(
                "el-deploy-conn-keep",
                if platform { "Leave the project's" } else { "Leave the server's" },
                !self.send_connections,
            )
            .disabled(self.deploying)
            .on_click(cx.listener(|this, _, _, cx| {
                this.send_connections = false;
                this.settle_profile();
                cx.notify();
            })),
        ]);
        let mut control = v_flex().w_full().gap_1().child(chips);
        if self.send_connections && platform {
            control = control.child(dialog::notice(
                Severity::Info,
                format!(
                    "Encrypted to the key of {remote}'s project alone — {fingerprint}; the \
                     console passes it on unread. The platform checks connections.yml as its \
                     settings do, and keeps the variables as the project's secrets. Sending \
                     takes a project admin."
                ),
            ));
        } else if self.send_connections {
            control = control.child(dialog::notice(
                Severity::Info,
                format!(
                    "Encrypted for {remote} alone — its key is {fingerprint}. Passwords are \
                     unreadable in transit and to anything between here and the daemon."
                ),
            ));
        }
        if self.send_connections {
            if !bundle.variables.is_empty() {
                control = control.child(dialog::notice(
                    Severity::Info,
                    format!("Variables sent: {}", bundle.variables.join(", ")),
                ));
            }
            if !bundle.file_valued.is_empty() && platform {
                control = control.child(dialog::notice(
                    Severity::Info,
                    format!(
                        "{} name{} a file here — a key pair or a wallet. The platform's jobs \
                         read their files from the volumes its chart mounts, so {} the value \
                         the project has there (the path), and the file stays here.",
                        bundle.file_valued.join(", "),
                        if bundle.file_valued.len() == 1 { "s" } else { "" },
                        if bundle.file_valued.len() == 1 { "it keeps" } else { "they keep" },
                    ),
                ));
            } else if !bundle.file_valued.is_empty() {
                control = control.child(dialog::notice(
                    Severity::Info,
                    format!(
                        "{} name{} a file here — a key pair or a wallet. The file travels \
                         sealed too, and {remote} keeps its own copy, readable only by the \
                         daemon.",
                        bundle.file_valued.join(", "),
                        if bundle.file_valued.len() == 1 { "s" } else { "" },
                    ),
                ));
            }
            if !bundle.too_large.is_empty() {
                control = control.child(dialog::notice(
                    Severity::Warning,
                    format!(
                        "{} name{} a file too large to send — it has to be on {remote} \
                         already, at the path this variable holds.",
                        bundle.too_large.join(", "),
                        if bundle.too_large.len() == 1 { "s" } else { "" },
                    ),
                ));
            }
            if !bundle.missing.is_empty() {
                control = control.child(dialog::notice(
                    Severity::Warning,
                    format!(
                        "Not set here, so they can't travel: {}. Connections that use them \
                         won't work on {remote}.",
                        bundle.missing.join(", ")
                    ),
                ));
            }
        } else {
            control = control.child(dialog::notice(
                Severity::Warning,
                format!(
                    "{remote} keeps the connections it has — a name this pipeline uses that \
                     is missing there fails the run."
                ),
            ));
        }
        dialog::labeled("Connections", control)
    }

    /// The platform's two ways to ship the file, and what the chosen one
    /// does to the project's draft.
    fn render_route(
        &self,
        remote: &SharedString,
        workspace: &Result<Option<RemoteDraft>, SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = &self.pipeline;
        let file = self.file_name();
        let through_draft = self.through_draft();
        let chips = dialog::chips([
            dialog::chip(
                "el-deploy-route-draft",
                "From the project's draft",
                through_draft,
            )
            .disabled(self.deploying || workspace.is_err())
            .on_click(cx.listener(|this, _, _, cx| {
                this.route = Route::Draft;
                // A draft deploys unpinned.
                this.selected_profile = None;
                cx.notify();
            })),
            dialog::chip("el-deploy-route-direct", "Directly", !through_draft)
                .disabled(self.deploying)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.route = Route::Direct;
                    cx.notify();
                })),
        ]);
        let (severity, text) = match (workspace, through_draft) {
            (Err(reason), _) => (
                Severity::Warning,
                format!(
                    "{remote}'s drafts can't be used ({reason}): this deploy ships {file} \
                     directly."
                ),
            ),
            (Ok(_), false) => (
                Severity::Info,
                format!(
                    "Ships {file} as it is, under the profile chosen below. The project's draft \
                     of {name}, if any, stays as it is."
                ),
            ),
            (Ok(None), true) => (
                Severity::Info,
                format!(
                    "Saves {file} as the project's draft of {name}, then deploys it: the console \
                     shows who deployed which draft."
                ),
            ),
            (Ok(Some(draft)), true)
                if self.local_text.as_deref() == Some(draft.content.as_str()) =>
            {
                (
                    Severity::Info,
                    format!(
                        "The project's draft of {name} is this file already: deployed from it."
                    ),
                )
            }
            (Ok(Some(draft)), true) => {
                let by = draft
                    .updated_by
                    .as_deref()
                    .map(|who| format!(" by {who}"))
                    .unwrap_or_default();
                let when = draft
                    .updated_unix
                    .map(|unix| {
                        format!(
                            " {}",
                            super::runs_panel::relative_time(unix.max(0) as u64, false)
                        )
                    })
                    .unwrap_or_default();
                (
                    Severity::Warning,
                    format!(
                        "Replaces the project's draft of {name}, saved{by}{when}, with {file}, \
                         then deploys it."
                    ),
                )
            }
        };
        dialog::labeled(
            "Deploy",
            v_flex()
                .w_full()
                .gap_1()
                .child(chips)
                .child(dialog::notice(severity, text)),
        )
    }

    /// The consequence lines: what pressing Deploy would do on `remote`.
    fn render_consequences(
        &self,
        remote: &SharedString,
        pipelines: &[RemotePipeline],
        tracks_checkout: bool,
    ) -> Div {
        let mut rows = v_flex().w_full().gap_1p5();
        let line = |text: String, color: Color| {
            dialog::notice(
                match color {
                    Color::Error => Severity::Error,
                    Color::Warning => Severity::Warning,
                    _ => Severity::Info,
                },
                text,
            )
        };
        if let Some(refusal) = &self.refusal {
            return rows.child(line(refusal.to_string(), Color::Error));
        }
        if tracks_checkout {
            rows = rows.child(line(
                format!(
                    "{remote} tracks its git checkout — deploy by pushing to it, not from here."
                ),
                Color::Error,
            ));
        }
        let Some(local) = &self.local else {
            return rows.child(line(format!("Reading {}…", self.file_name()), Color::Muted));
        };
        let pf =
            el_engine::server::deploy_preflight(local, pipelines, self.selected_profile.as_deref());
        let profile_text = match (&self.selected_profile, self.profile_choices().1) {
            (Some(profile), _) => format!("profile {profile}"),
            // Unpinned follows the server's own choice — which is worth
            // naming, since it is what the run will actually use.
            (None, Some(fallback)) => format!("{remote}'s own profile ({fallback})"),
            (None, None) => format!("{remote}'s base connections"),
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
                        format!(
                            "The current schedule {previous} stops — manual runs only afterwards."
                        ),
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
                // What the deploy does to the schedule: arms a new one,
                // swaps the one already firing, or leaves it running.
                let previous = pf.replacing.as_ref().and_then(|replaced| {
                    replaced.previous_schedule.as_ref().map(|previous| {
                        // An older server doesn't report its timezone, so
                        // an equal cron alone can't prove nothing changed.
                        let same = previous == cron
                            && replaced.previous_timezone.as_deref() == Some(timezone.as_str());
                        (previous.clone(), same)
                    })
                });
                let unchanged = matches!(&previous, Some((_, true)));
                let run = match pf.first_fire_unix {
                    Some(unix) => format!(
                        "{} run {}",
                        if unchanged { "next" } else { "first" },
                        super::runs_panel::relative_time(unix, true)
                    ),
                    None => "the server can't parse it and will skip it".to_owned(),
                };
                let text = match previous {
                    Some((_, true)) => format!(
                        "Schedule {cron} ({timezone}) keeps firing on {remote} under \
                         {profile_text} — {run}."
                    ),
                    Some((previous, false)) => format!(
                        "Schedule changes: {previous} → {cron} ({timezone}) on {remote} under \
                         {profile_text} — {run}."
                    ),
                    None => format!(
                        "Schedule {cron} ({timezone}) starts firing on {remote} under \
                         {profile_text} — {run}."
                    ),
                };
                rows = rows.child(line(
                    text,
                    if pf.first_fire_unix.is_some() {
                        Color::Muted
                    } else {
                        Color::Warning
                    },
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

/// What a deploy shipped: through the project's draft (and whether the file
/// had to be saved there first), or directly, with the connections it
/// carried — how many connections, variables and files — if any.
enum Shipped {
    Draft(bool),
    Direct(Option<(usize, usize, usize)>),
}

/// Saves `yaml` as the project's draft of `name` — unless the draft is
/// already it — while the draft is still `base` (or there is none, when it
/// is None), then deploys the pipeline from it. Returns whether it saved.
fn deploy_through_draft(
    client: &el_engine::server::RemoteClient,
    name: &str,
    yaml: &str,
    base: Option<RemoteDraft>,
) -> anyhow::Result<bool> {
    let saved = match &base {
        Some(draft) if draft.content == yaml => false,
        _ => {
            client
                .save_draft(name, yaml, base.as_ref().map(|draft| draft.sha.as_str()))
                .map_err(|error| match RemoteRefusal::of(&error) {
                    // Saved, or made, by someone else since the popup looked.
                    Some(refusal) if refusal.status == 409 => anyhow::anyhow!(
                        "the project's draft of {name} changed since this opened — look at it \
                         again before deploying over it"
                    ),
                    _ => error.context("saving the project's draft"),
                })?;
            true
        }
    };
    client.deploy_drafts(&[name.to_owned()]).map_err(|error| {
        if saved {
            error.context("saved as the project's draft, but not deployed")
        } else {
            error
        }
    })?;
    Ok(saved)
}

impl EventEmitter<DismissEvent> for ElDeployModal {}
impl ModalView for ElDeployModal {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        // A deploy in flight reports how it ended, so the popup stays up.
        workspace::DismissDecision::Dismiss(!self.deploying)
    }
}

impl Focusable for ElDeployModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ElDeployModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let remote = self
            .remotes
            .get(self.selected_remote)
            .cloned()
            .unwrap_or_else(|| "remote".into());
        let focus = self.focus_handle.clone();
        let mut body = dialog::body();

        if self.remotes.len() > 1 {
            let chips = self.remotes.iter().enumerate().map(|(ix, name)| {
                dialog::chip(
                    ("el-deploy-remote", ix),
                    name.clone(),
                    ix == self.selected_remote,
                )
                .disabled(self.deploying)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_remote = ix;
                    this.send_connections = this.selected_platform().is_none();
                    this.fetch_facts(cx);
                    cx.notify();
                }))
            });
            body = body.child(dialog::labeled("Server", dialog::chips(chips)));
        }

        // On the platform: through the project's draft, or directly.
        if let RemoteFacts::Loaded {
            workspace: Some(workspace),
            ..
        } = &self.facts
        {
            body = body.child(self.render_route(&remote, workspace, cx));
        }

        body = body.child(match &self.facts {
            RemoteFacts::Loading => {
                dialog::labeled("Profile", dialog::loading(format!("Checking {remote}…")))
            }
            RemoteFacts::Failed(error) => v_flex()
                .w_full()
                .gap_1()
                .child(dialog::notice(
                    Severity::Error,
                    format!("Couldn't reach {remote}: {error}"),
                ))
                .child(
                    Button::new("el-deploy-retry", "Try again")
                        .label_size(LabelSize::Small)
                        .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fetch_facts(cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
            RemoteFacts::Loaded { .. } if self.through_draft() => {
                // A draft deploys unpinned, like the console's builder.
                let (_, fallback, _) = self.profile_choices();
                let own = match fallback {
                    Some(fallback) => format!("{remote}'s own ({fallback})"),
                    None => format!("{remote}'s own"),
                };
                dialog::labeled(
                    "Profile",
                    dialog::notice(
                        Severity::Info,
                        format!(
                            "{own}: a deploy from the project's draft pins none. Directly pins \
                             one."
                        ),
                    ),
                )
            }
            RemoteFacts::Loaded { .. } => {
                let (profiles, fallback, from_local) = self.profile_choices();
                // A pipeline that pins nothing runs under the server's own
                // choice, so the chip says which one that is rather than
                // leaving it to be discovered in a run.
                let unpinned_label: SharedString = match &fallback {
                    Some(fallback) => format!("server's own ({fallback})").into(),
                    None => "server's own".into(),
                };
                let mut chips = vec![
                    dialog::chip(
                        "el-deploy-profile-default",
                        unpinned_label,
                        self.selected_profile.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_profile = None;
                        cx.notify();
                    })),
                ];
                for (ix, profile) in profiles.iter().enumerate() {
                    let selected = self.selected_profile.as_deref() == Some(profile);
                    let profile = profile.clone();
                    chips.push(
                        dialog::chip(
                            ("el-deploy-profile", ix),
                            SharedString::from(profile.clone()),
                            selected,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_profile = Some(profile.clone());
                            cx.notify();
                        })),
                    );
                }
                let mut control = v_flex().w_full().gap_1().child(dialog::chips(chips));
                control = control.child(dialog::notice(
                    Severity::Info,
                    match (profiles.is_empty(), from_local) {
                        // The list comes from the file this deploy sends,
                        // which is what the server will be running — not
                        // from what it declares this second.
                        (false, true) => format!(
                            "{} — the environments in the connections this deploy sends. \
                             A pipeline pinned to one runs under it; pinned to none, it \
                             follows {remote}'s own choice.",
                            profiles.join(", ")
                        ),
                        (false, false) => format!(
                            "{} — the environments {remote} declares today.",
                            profiles.join(", ")
                        ),
                        (true, true) => {
                            "The connections this deploy sends declare no environments — \
                             the pipeline runs against their base connections."
                                .to_owned()
                        }
                        (true, false) => format!(
                            "{remote} declares no environments — the pipeline runs against \
                             its base connections."
                        ),
                    },
                ));
                dialog::labeled("Profile", control)
            }
        });

        if let RemoteFacts::Loaded { sealing, .. } = &self.facts {
            body = body.child(self.render_connections(&remote, sealing, cx));
        }

        // What the deploy changes on the server, or why it can't ship.
        match &self.facts {
            RemoteFacts::Loaded {
                pipelines,
                tracks_checkout,
                ..
            } => {
                body = body.child(self.render_consequences(&remote, pipelines, *tracks_checkout));
            }
            _ => {
                if let Some(refusal) = &self.refusal {
                    body = body.child(dialog::notice(Severity::Error, refusal.clone()));
                }
            }
        }

        if let Some(error) = &self.error {
            body = body.child(dialog::notice(Severity::Error, error.clone()));
        }

        let deploy_label: SharedString = if self.deploying {
            "Deploying…".into()
        } else {
            match &self.selected_profile {
                Some(profile) => format!("Deploy to {remote} ({profile})").into(),
                None => format!("Deploy to {remote}").into(),
            }
        };
        dialog::frame(dialog::NARROW, cx)
            .key_context("ElDeployModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| {
                if !this.deploying {
                    cx.emit(DismissEvent);
                }
            }))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.send(window, cx)))
            .child(dialog::header(
                "Deploy pipeline",
                Some(self.pipeline.clone().into()),
                cx.listener(|this, _, _, cx| {
                    if !this.deploying {
                        cx.emit(DismissEvent);
                    }
                }),
                cx,
            ))
            .child(body)
            .child(dialog::footer(
                [],
                [
                    dialog::cancel_button("el-deploy-cancel", &focus, cx)
                        .disabled(self.deploying)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
                        .into_any_element(),
                    dialog::primary_button("el-deploy-send", deploy_label, &focus, cx)
                        .disabled(!self.can_send())
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                        .into_any_element(),
                ],
                cx,
            ))
    }
}
