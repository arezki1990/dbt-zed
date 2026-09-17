//! The remote editor: add, edit, or delete a server in el/remotes.yml
//! from the sidebar. A remote is a name, an https URL, and the NAME of
//! the environment variable holding its token — the token value itself
//! never enters the form or the file.

use std::path::PathBuf;

use anyhow::{Result, bail};
use editor::Editor;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    WeakEntity, Window,
};
use indexmap::IndexMap;
use project::Project;
use ui::{Tooltip, prelude::*};
use workspace::{ModalView, Workspace};

use el_engine::spec::{RemoteSpec, Remotes, SpecError};

use super::dialog;
use super::panel::ElPanel;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Details,
    Install,
    Review,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// A daemon is already running somewhere: declare it.
    Existing,
    /// Install one on a Linux server over ssh, then declare it.
    Install,
}

pub struct ElRemoteModal {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<ElPanel>,
    project: Entity<Project>,
    root: PathBuf,
    /// The original name when editing; None while adding.
    editing: Option<String>,
    name: Entity<Editor>,
    url: Entity<Editor>,
    /// Env var NAME (e.g. ZDBT_EL_TOKEN); stored as "${NAME}".
    token_var: Entity<Editor>,
    /// Install over SSH: user@host, the project dir, the profile to run.
    ssh_host: Entity<Editor>,
    ssh_project: Entity<Editor>,
    ssh_profile: Entity<Editor>,
    /// Wizard position (add mode only; edit is a single page).
    step: Step,
    mode: Mode,
    /// A token generated here for an existing daemon: (variable, value).
    /// Written to .env; the value is only ever copied, never displayed.
    generated: Option<(String, String)>,
    /// The generated token went to the clipboard; the button says so.
    copied: bool,
    delete_armed: bool,
    writing: bool,
    error: Option<SharedString>,
}

fn load_remotes_strict(root: &std::path::Path) -> Result<Remotes> {
    let path = super::el_dir(root).join("remotes.yml");
    match el_engine::spec::load_remotes(&path) {
        Ok(remotes) => Ok(remotes),
        Err(SpecError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(Remotes {
                version: 1,
                remotes: IndexMap::new(),
                extra: IndexMap::new(),
            })
        }
        Err(error) => bail!("remotes.yml could not be read: {error} — fix the file first"),
    }
}

/// "${ZDBT_EL_TOKEN}" → "ZDBT_EL_TOKEN" for display.
fn var_name_of(template: &str) -> String {
    template
        .trim()
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(template.trim())
        .to_owned()
}

impl ElRemoteModal {
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
        let mut error = None;
        let existing: Option<RemoteSpec> =
            editing
                .as_ref()
                .and_then(|name| match load_remotes_strict(&root) {
                    Ok(remotes) => {
                        let found = remotes.remotes.get(name).cloned();
                        if found.is_none() {
                            error = Some(format!("{name:?} is gone from remotes.yml").into());
                        }
                        found
                    }
                    Err(load_error) => {
                        error = Some(format!("{load_error:#}").into());
                        None
                    }
                });
        let mut make = |placeholder: &str, initial: &str| {
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
        };
        let name = make("prod_vm", editing.as_deref().unwrap_or(""));
        let url = make(
            "https://el.example.com:7431",
            existing
                .as_ref()
                .map(|remote| remote.url.as_str())
                .unwrap_or(""),
        );
        let token_var = make(
            "ZDBT_EL_TOKEN",
            &existing
                .as_ref()
                .and_then(|remote| remote.token.as_deref())
                .map(var_name_of)
                .unwrap_or_default(),
        );
        let ssh_host = make("deploy@el.example.com", "");
        let ssh_project = make("/srv/el-project", "");
        let ssh_profile = make("prod", "");
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            panel,
            project,
            root,
            editing,
            name,
            url,
            token_var,
            ssh_host,
            ssh_project,
            ssh_profile,
            step: Step::Details,
            mode: Mode::Existing,
            generated: None,
            copied: false,
            delete_armed: false,
            writing: false,
            error,
        }
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn write(
        &mut self,
        remotes: Remotes,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.writing = true;
        cx.notify();
        let workspace = self.workspace.clone();
        let panel = self.panel.clone();
        let project = self.project.clone();
        let path = super::el_dir(&self.root).join("remotes.yml");
        cx.spawn_in(window, async move |this, cx| {
            let result = super::spec_io::write_text(
                workspace.clone(),
                project,
                path,
                el_engine::spec::to_canonical_remotes_yaml(&remotes),
                cx,
            )
            .await;
            if result.is_ok() {
                panel.update(cx, |panel, cx| panel.remotes_changed(cx)).ok();
                workspace
                    .update(cx, |workspace, cx| super::toast(workspace, &done, cx))
                    .ok();
            }
            this.update_in(cx, |this, _, cx| {
                this.writing = false;
                match &result {
                    Ok(()) => cx.emit(DismissEvent),
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The details step's checks, without writing anything.
    fn details_valid(&mut self, cx: &mut Context<Self>) -> bool {
        let name = self.name.read(cx).text(cx).trim().to_owned();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            self.fail(
                "the server needs a plain name (letters, digits, _ -)".into(),
                cx,
            );
            return false;
        }
        let url = self.url.read(cx).text(cx).trim().to_owned();
        if let Err(error) = el_engine::server::check_remote_url(&url) {
            self.fail(format!("{error:#}"), cx);
            return false;
        }
        if self.mode == Mode::Existing {
            let token_var = var_name_of(&self.token_var.read(cx).text(cx));
            if !token_var.is_empty()
                && !(token_var.len() <= 64
                    && token_var
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'))
            {
                self.fail(
                    "give the NAME of the environment variable holding the token (e.g. \
                     ZDBT_EL_TOKEN) — put the value itself in .env"
                        .into(),
                    cx,
                );
                return false;
            }
            if token_var.is_empty() && !url.starts_with("http://") {
                self.fail(
                    "a server beyond localhost needs a token variable".into(),
                    cx,
                );
                return false;
            }
        }
        self.error = None;
        true
    }

    /// Generates a token for an existing daemon: random 48 hex chars,
    /// appended to the project's .env under the token variable (defaulting
    /// to ZDBT_EL_TOKEN_<NAME>), then offered for copying to the server.
    fn generate_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut var = var_name_of(&self.token_var.read(cx).text(cx));
        if var.is_empty() {
            var = self.generated_token_var(cx);
        }
        if !(var.len() <= 64 && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return self.fail(
                "the token variable must be a plain NAME (e.g. ZDBT_EL_TOKEN)".into(),
                cx,
            );
        }
        let mut bytes = [0u8; 24];
        if let Err(error) = std::fs::File::open("/dev/urandom")
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        {
            return self.fail(format!("could not generate a token: {error}"), cx);
        }
        let value: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let env_path = self.root.join(".env");
        let write = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
            if existing
                .lines()
                .any(|line| line.starts_with(&format!("{var}=")))
            {
                return Err(std::io::Error::other(format!(
                    "{var} already exists in .env — pick another variable name"
                )));
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&env_path)?;
            if !existing.is_empty() && !existing.ends_with('\n') {
                writeln!(file)?;
            }
            writeln!(file, "{var}={value}")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        })();
        if let Err(error) = write {
            return self.fail(format!("could not write .env: {error}"), cx);
        }
        self.error = None;
        // Reflect the variable actually used in the field.
        self.token_var
            .update(cx, |editor, cx| editor.set_text(var.clone(), window, cx));
        self.generated = Some((var, value));
        self.copied = false;
        cx.notify();
    }

    /// The generated token variable for an SSH install.
    fn generated_token_var(&self, cx: &App) -> String {
        let name = self.name.read(cx).text(cx).trim().to_ascii_uppercase();
        format!(
            "ZDBT_EL_TOKEN_{}",
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        )
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let name = self.name.read(cx).text(cx).trim().to_owned();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return self.fail(
                "the server needs a plain name (letters, digits, _ -)".into(),
                cx,
            );
        }
        let url = self.url.read(cx).text(cx).trim().to_owned();
        if let Err(error) = el_engine::server::check_remote_url(&url) {
            return self.fail(format!("{error:#}"), cx);
        }
        let token_var = var_name_of(&self.token_var.read(cx).text(cx));
        let token = if token_var.is_empty() {
            None
        } else {
            // A variable name, never a value: no spaces, no long random
            // strings, no quotes.
            let looks_like_var = token_var.len() <= 64
                && token_var
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !looks_like_var {
                return self.fail(
                    "give the NAME of the environment variable holding the token \
                     (e.g. ZDBT_EL_TOKEN) — put the value itself in .env"
                        .into(),
                    cx,
                );
            }
            Some(format!("${{{token_var}}}"))
        };
        if token.is_none() && !url.starts_with("http://") {
            return self.fail(
                "a server beyond localhost needs a token variable".into(),
                cx,
            );
        }

        let mut remotes = match load_remotes_strict(&self.root) {
            Ok(remotes) => remotes,
            Err(error) => return self.fail(format!("{error:#}"), cx),
        };
        let extra = self
            .editing
            .as_ref()
            .and_then(|original| remotes.remotes.get(original))
            .map(|remote| remote.extra.clone())
            .unwrap_or_default();
        let value = RemoteSpec { url, token, extra };
        match &self.editing {
            None => {
                if remotes.remotes.contains_key(&name) {
                    return self.fail(format!("a server named {name:?} already exists"), cx);
                }
                remotes.remotes.insert(name.clone(), value);
            }
            Some(original) => {
                if name != *original && remotes.remotes.contains_key(&name) {
                    return self.fail(format!("a server named {name:?} already exists"), cx);
                }
                if !remotes.remotes.contains_key(original) {
                    return self.fail("the server is gone from remotes.yml".into(), cx);
                }
                remotes.remotes = remotes
                    .remotes
                    .iter()
                    .map(|(key, existing)| {
                        if key == original {
                            (name.clone(), value.clone())
                        } else {
                            (key.clone(), existing.clone())
                        }
                    })
                    .collect();
            }
        }
        let done = match &self.editing {
            None => format!("Server {name} added."),
            Some(_) => format!("Server {name} saved."),
        };
        self.write(remotes, done, window, cx);
    }

    /// Runs `zdbt el install-remote` in a terminal tab: the daemon gets
    /// installed on the server over the user's ssh, and the server is
    /// declared locally when it finishes.
    fn install_over_ssh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.ssh_host.read(cx).text(cx).trim().to_owned();
        if host.is_empty() || !host.contains('@') {
            return self.fail("give the server as user@host".into(), cx);
        }
        let remote_project = {
            let text = self.ssh_project.read(cx).text(cx).trim().to_owned();
            if text.is_empty() {
                "/srv/el-project".to_owned()
            } else {
                text
            }
        };
        let name = self.name.read(cx).text(cx).trim().to_owned();
        let url = self.url.read(cx).text(cx).trim().to_owned();
        let profile = {
            let text = self.ssh_profile.read(cx).text(cx).trim().to_owned();
            if text.is_empty() {
                "prod".to_owned()
            } else {
                text
            }
        };
        let Ok(exe) = std::env::current_exe() else {
            return self.fail("could not locate the zdbt binary".into(), cx);
        };
        let mut args = vec![
            "el".to_owned(),
            "install-remote".to_owned(),
            host.clone(),
            "--remote-project".to_owned(),
            remote_project,
            "--project".to_owned(),
            self.root.to_string_lossy().into_owned(),
            "--profile".to_owned(),
            profile,
        ];
        if !name.is_empty() {
            args.extend(["--name".to_owned(), name]);
        }
        if !url.is_empty() {
            args.extend(["--url".to_owned(), url]);
        }
        let label = format!("Install EL daemon on {host}");
        let spawn = task::SpawnInTerminal {
            id: task::TaskId(format!("el-install-{host}")),
            full_label: label.clone(),
            label: label.clone(),
            command: Some(exe.to_string_lossy().into_owned()),
            args,
            command_label: label,
            cwd: Some(self.root.clone()),
            env: Default::default(),
            use_new_terminal: true,
            allow_concurrent_runs: false,
            reveal: task::RevealStrategy::Always,
            reveal_target: task::RevealTarget::Dock,
            hide: task::HideStrategy::Never,
            shell: task::Shell::System,
            show_summary: true,
            show_command: true,
            show_rerun: true,
            save: Default::default(),
        };
        let panel = self.panel.clone();
        self.workspace
            .update(cx, |workspace, cx| {
                let status = workspace.spawn_in_terminal(spawn, window, cx);
                cx.spawn(async move |_, cx| {
                    let _ = status.await;
                    // The CLI wrote remotes.yml + .env — refresh the sidebar.
                    panel.update(cx, |panel, cx| panel.remotes_changed(cx)).ok();
                })
                .detach();
            })
            .ok();
        cx.emit(DismissEvent);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let Some(original) = self.editing.clone() else {
            return;
        };
        if !self.delete_armed {
            self.delete_armed = true;
            cx.notify();
            return;
        }
        let mut remotes = match load_remotes_strict(&self.root) {
            Ok(remotes) => remotes,
            Err(error) => return self.fail(format!("{error:#}"), cx),
        };
        if remotes.remotes.shift_remove(&original).is_none() {
            return self.fail("the server is gone from remotes.yml".into(), cx);
        }
        self.write(remotes, format!("Server {original} removed."), window, cx);
    }
}

impl EventEmitter<DismissEvent> for ElRemoteModal {}

impl ModalView for ElRemoteModal {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        workspace::DismissDecision::Dismiss(!self.writing)
    }
}

impl Focusable for ElRemoteModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

impl ElRemoteModal {
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.writing {
            cx.emit(DismissEvent);
        }
    }

    /// The editors the current step shows, in the order Tab walks them.
    fn step_editors(&self) -> Vec<Entity<Editor>> {
        match (self.editing.is_some(), self.step, self.mode) {
            (true, _, _) | (false, Step::Details, Mode::Existing) => {
                vec![self.name.clone(), self.url.clone(), self.token_var.clone()]
            }
            (false, Step::Details, Mode::Install) => vec![self.name.clone(), self.url.clone()],
            (false, Step::Install, _) => vec![
                self.ssh_host.clone(),
                self.ssh_project.clone(),
                self.ssh_profile.clone(),
            ],
            (false, Step::Review, _) => Vec::new(),
        }
    }

    fn step_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let fields: Vec<FocusHandle> = self
            .step_editors()
            .iter()
            .map(|editor| editor.focus_handle(cx))
            .collect();
        dialog::cycle_focus(&fields, forward, window, cx);
    }

    /// Lands focus on a step's first field — or, on Review, on the dialog
    /// itself, so ⏎ and esc still reach it.
    fn focus_step(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step_editors().first() {
            Some(editor) => window.focus(&editor.focus_handle(cx), cx),
            None => window.focus(&self.focus_handle, cx),
        }
    }

    /// Next, Add, Install, or Save — whatever the footer's primary button
    /// says right now; ⏎ presses it.
    fn primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        if self.editing.is_some() {
            return self.save(window, cx);
        }
        match self.step {
            Step::Details => {
                if self.details_valid(cx) {
                    self.step = if self.mode == Mode::Install {
                        Step::Install
                    } else {
                        Step::Review
                    };
                    self.focus_step(window, cx);
                }
            }
            Step::Install => {
                let host = self.ssh_host.read(cx).text(cx).trim().to_owned();
                if host.is_empty() || !host.contains('@') {
                    self.fail("give the server as user@host".into(), cx);
                    return;
                }
                self.error = None;
                self.step = Step::Review;
                self.focus_step(window, cx);
            }
            Step::Review => match self.mode {
                Mode::Existing => self.save(window, cx),
                Mode::Install => self.install_over_ssh(window, cx),
            },
        }
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = match (self.step, self.mode) {
            (Step::Review, Mode::Install) => Step::Install,
            _ => Step::Details,
        };
        self.error = None;
        self.focus_step(window, cx);
        cx.notify();
    }
}

impl Render for ElRemoteModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let editing = self.editing.is_some();
        let focus = self.focus_handle(cx);
        let steps: &[(Step, &str)] = if self.mode == Mode::Install {
            &[
                (Step::Details, "Server"),
                (Step::Install, "Install"),
                (Step::Review, "Review"),
            ]
        } else {
            &[(Step::Details, "Server"), (Step::Review, "Review")]
        };
        let position = steps
            .iter()
            .position(|(step, _)| *step == self.step)
            .unwrap_or(0);
        let (title, detail): (&str, SharedString) = if editing {
            (
                "Edit server",
                self.editing.clone().unwrap_or_default().into(),
            )
        } else {
            (
                "Add server",
                format!(
                    "Step {} of {} · {}",
                    position + 1,
                    steps.len(),
                    steps.get(position).map_or("", |(_, label)| *label)
                )
                .into(),
            )
        };

        let mut body = dialog::body();

        // Step trail (add mode): done steps carry a check, the current one
        // is filled.
        if !editing {
            let mut trail = h_flex().w_full().gap_2().items_center();
            for (ix, (_, label)) in steps.iter().enumerate() {
                let (marker, color): (AnyElement, Color) = if ix < position {
                    (
                        Icon::new(IconName::Check)
                            .size(IconSize::XSmall)
                            .color(Color::Success)
                            .into_any_element(),
                        Color::Muted,
                    )
                } else {
                    (
                        Label::new(format!("{}", ix + 1))
                            .size(LabelSize::XSmall)
                            .color(if ix == position {
                                Color::Accent
                            } else {
                                Color::Muted
                            })
                            .into_any_element(),
                        if ix == position {
                            Color::Default
                        } else {
                            Color::Muted
                        },
                    )
                };
                trail = trail.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .size_5()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(if ix == position {
                                    colors.border_focused
                                } else {
                                    colors.border_variant
                                })
                                .child(marker),
                        )
                        .child(Label::new(*label).size(LabelSize::Small).color(color)),
                );
                if ix + 1 < steps.len() {
                    trail = trail.child(div().w(px(24.)).h(px(1.)).bg(colors.border_variant));
                }
            }
            body = body.child(trail);
        }

        body = match (editing, self.step) {
            (true, _) | (false, Step::Details) => {
                if !editing {
                    body = body.child(dialog::labeled(
                        "Setup",
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(dialog::chips([
                                dialog::chip(
                                    "el-remote-mode-existing",
                                    "Existing daemon",
                                    self.mode == Mode::Existing,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.mode = Mode::Existing;
                                        cx.notify();
                                    },
                                )),
                                dialog::chip(
                                    "el-remote-mode-install",
                                    "New server over SSH",
                                    self.mode == Mode::Install,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.mode = Mode::Install;
                                        cx.notify();
                                    },
                                )),
                            ]))
                            .child(
                                Label::new(if self.mode == Mode::Existing {
                                    "Point at an el serve daemon that is already running."
                                } else {
                                    "Install the daemon on a Linux server through your own ssh."
                                })
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            ),
                    ));
                }
                body = body
                    .child(dialog::field(
                        "Name",
                        &self.name,
                        Some("Letters, digits, _ and -.".into()),
                        window,
                        cx,
                    ))
                    .child(dialog::field(
                        "URL",
                        &self.url,
                        Some("https is required beyond localhost.".into()),
                        window,
                        cx,
                    ));
                if editing || self.mode == Mode::Existing {
                    body = body.child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_start()
                            .child(div().flex_1().min_w_0().child(dialog::field(
                                "Token variable",
                                &self.token_var,
                                Some(
                                    "Only the variable's NAME goes in YAML; the token lives in \
                                     .env."
                                        .into(),
                                ),
                                window,
                                cx,
                            )))
                            .child(
                                Button::new("el-remote-gen-token", "Generate")
                                    .label_size(LabelSize::Small)
                                    .start_icon(Icon::new(IconName::Sparkle).size(IconSize::Small))
                                    .tooltip(Tooltip::text(
                                        "Make a random token, store it in this project's .env \
                                         under the variable, and offer it for copying to the \
                                         server",
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.generate_token(window, cx)
                                    })),
                            ),
                    );
                    if let Some((var, value)) = self.generated.clone() {
                        body = body.child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(dialog::notice(
                                    Severity::Success,
                                    format!(
                                        "Token written to .env as {var}. Put the same value in \
                                         the server's /etc/zdbt-el-serve/env."
                                    ),
                                ))
                                .child(
                                    h_flex().pl_5().child(
                                        Button::new(
                                            "el-remote-copy-token",
                                            if self.copied { "Copied" } else { "Copy token" },
                                        )
                                        .label_size(LabelSize::Small)
                                        .start_icon(
                                            Icon::new(if self.copied {
                                                IconName::Check
                                            } else {
                                                IconName::Copy
                                            })
                                            .size(IconSize::Small),
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(value.clone()),
                                                );
                                                this.copied = true;
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                                ),
                        );
                    }
                } else {
                    body = body.child(dialog::labeled(
                        "Token variable",
                        Label::new(format!(
                            "{} — generated during the install",
                            self.generated_token_var(cx)
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ));
                }
                body
            }
            (false, Step::Install) => body
                .child(dialog::field(
                    "SSH",
                    &self.ssh_host,
                    Some("user@host, as you would type it after ssh.".into()),
                    window,
                    cx,
                ))
                .child(dialog::field(
                    "Project directory",
                    &self.ssh_project,
                    Some("Where the project lives on the server.".into()),
                    window,
                    cx,
                ))
                .child(dialog::field(
                    "Profile",
                    &self.ssh_profile,
                    Some("The connections profile the daemon runs.".into()),
                    window,
                    cx,
                ))
                .child(dialog::notice(
                    Severity::Info,
                    "Runs the zdbt-el installer on the server through your own ssh (sudo \
                     needed): builds zdbt-el-serve, creates the service and a systemd unit. \
                     The token is generated here and sent over stdin.",
                )),
            (false, Step::Review) => {
                let name = self.name.read(cx).text(cx).trim().to_owned();
                let url = self.url.read(cx).text(cx).trim().to_owned();
                let token_var = if self.mode == Mode::Install {
                    self.generated_token_var(cx)
                } else {
                    var_name_of(&self.token_var.read(cx).text(cx))
                };
                let mut yaml = format!("# el/remotes.yml\nremotes:\n  {name}:\n    url: {url}\n");
                if !token_var.is_empty() {
                    yaml.push_str(&format!("    token: \"${{{token_var}}}\"\n"));
                }
                let env_line = if token_var.is_empty() {
                    "# .env — no token (loopback only)".to_owned()
                } else if self.mode == Mode::Install {
                    format!("# .env\n{token_var}=<generated on install>")
                } else {
                    format!("# .env\n{token_var}=<your token — set it yourself>")
                };
                body = body.child(
                    Label::new("This is what gets written:")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
                for block in [yaml, env_line] {
                    let mut pre = v_flex()
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(colors.editor_background)
                        .border_1()
                        .border_color(colors.border_variant);
                    for line in block.lines() {
                        pre = pre.child(
                            Label::new(line.to_owned())
                                .size(LabelSize::Small)
                                .buffer_font(cx),
                        );
                    }
                    body = body.child(pre);
                }
                if self.mode == Mode::Install {
                    let host = self.ssh_host.read(cx).text(cx).trim().to_owned();
                    let dir = self.ssh_project.read(cx).text(cx).trim().to_owned();
                    let profile = self.ssh_profile.read(cx).text(cx).trim().to_owned();
                    body = body.child(dialog::notice(
                        Severity::Info,
                        format!(
                            "Then installs the daemon on {host} under {} running profile {} \
                             (a terminal tab shows progress).",
                            if dir.is_empty() {
                                "/srv/el-project"
                            } else {
                                &dir
                            },
                            if profile.is_empty() { "prod" } else { &profile }
                        ),
                    ));
                }
                body
            }
        };

        if let Some(error) = &self.error {
            body = body.child(dialog::notice(Severity::Error, error.clone()));
        }

        let remove = editing.then(|| {
            dialog::danger_button(
                "el-remote-delete",
                if self.delete_armed {
                    "Confirm remove"
                } else {
                    "Remove…"
                },
            )
            .disabled(self.writing)
            .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx)))
            .into_any_element()
        });
        let back = (!editing && self.step != Step::Details).then(|| {
            Button::new("el-remote-back", "Back")
                .label_size(LabelSize::Small)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.back(window, cx)))
                .into_any_element()
        });
        let primary: SharedString = match (editing, self.step, self.mode) {
            (true, _, _) => if self.writing {
                "Saving…"
            } else {
                "Save changes"
            }
            .into(),
            (false, Step::Review, Mode::Install) => "Install & add server".into(),
            (false, Step::Review, Mode::Existing) => if self.writing {
                "Saving…"
            } else {
                "Add server"
            }
            .into(),
            _ => "Next".into(),
        };

        dialog::frame(dialog::WIDE, cx)
            .key_context("ElRemoteModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.primary(window, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectNext, window, cx| {
                this.step_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                this.step_focus(false, window, cx)
            }))
            .child(dialog::header(
                title,
                Some(detail),
                cx.listener(|this, _, _, cx| this.cancel(cx)),
                cx,
            ))
            .child(body)
            .child(dialog::footer(
                remove,
                back.into_iter().chain([
                    dialog::cancel_button("el-remote-cancel", &focus, cx)
                        .disabled(self.writing)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                        .into_any_element(),
                    dialog::primary_button("el-remote-primary", primary, &focus, cx)
                        .disabled(self.writing)
                        .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx)))
                        .into_any_element(),
                ]),
                cx,
            ))
    }
}
