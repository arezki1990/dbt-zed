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
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use el_engine::spec::{RemoteSpec, Remotes, SpecError};

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
        let existing: Option<RemoteSpec> = editing.as_ref().and_then(|name| {
            match load_remotes_strict(&root) {
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
            existing.as_ref().map(|remote| remote.url.as_str()).unwrap_or(""),
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
            delete_armed: false,
            writing: false,
            error,
        }
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn write(&mut self, remotes: Remotes, done: String, window: &mut Window, cx: &mut Context<Self>) {
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
            self.fail("the server needs a plain name (letters, digits, _ -)".into(), cx);
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
                self.fail("a server beyond localhost needs a token variable".into(), cx);
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
            return self.fail("the token variable must be a plain NAME (e.g. ZDBT_EL_TOKEN)".into(), cx);
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
            if existing.lines().any(|line| line.starts_with(&format!("{var}="))) {
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
        self.token_var.update(cx, |editor, cx| editor.set_text(var.clone(), window, cx));
        self.generated = Some((var, value));
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
            return self.fail("the server needs a plain name (letters, digits, _ -)".into(), cx);
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
            return self.fail("a server beyond localhost needs a token variable".into(), cx);
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
            if text.is_empty() { "/srv/el-project".to_owned() } else { text }
        };
        let name = self.name.read(cx).text(cx).trim().to_owned();
        let url = self.url.read(cx).text(cx).trim().to_owned();
        let profile = {
            let text = self.ssh_profile.read(cx).text(cx).trim().to_owned();
            if text.is_empty() { "prod".to_owned() } else { text }
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
        let Some(original) = self.editing.clone() else { return };
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

impl Render for ElRemoteModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let editing = self.editing.is_some();
        let input_bg = colors.editor_background;
        let input_border = colors.border;
        let field_row = move |label: &'static str, editor: Entity<Editor>| {
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(div().w(px(150.)).flex_shrink_0().child(
                    Label::new(label).size(LabelSize::Default).color(Color::Muted),
                ))
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(input_border)
                        .bg(input_bg)
                        .child(editor),
                )
        };
        let steps: &[(Step, &str)] = if self.mode == Mode::Install {
            &[(Step::Details, "Server"), (Step::Install, "Install"), (Step::Review, "Review")]
        } else {
            &[(Step::Details, "Server"), (Step::Review, "Review")]
        };
        let title: SharedString = if editing {
            "Edit server".into()
        } else {
            let position = steps.iter().position(|(step, _)| *step == self.step).unwrap_or(0);
            format!("Add server — step {} of {}", position + 1, steps.len()).into()
        };

        let mut card = v_flex()
            .key_context("ElRemoteModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(680.))
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new(title).size(LabelSize::Large))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-remote-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            );

        // Step trail (add mode).
        if !editing {
            let mut trail = h_flex().w_full().px_4().pt_3().gap_2();
            for (ix, (step, label)) in steps.iter().enumerate() {
                let active = *step == self.step;
                trail = trail.child(
                    Label::new(format!("{}. {label}", ix + 1))
                        .size(LabelSize::Default)
                        .color(if active { Color::Accent } else { Color::Muted }),
                );
                if ix + 1 < steps.len() {
                    trail = trail.child(
                        Label::new("›").size(LabelSize::Default).color(Color::Muted),
                    );
                }
            }
            card = card.child(trail);
        }

        let body = match (editing, self.step) {
            (true, _) | (false, Step::Details) => {
                let mut fields = v_flex().w_full().px_4().py_3().gap_2();
                if !editing {
                    fields = fields.child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .pb_1()
                            .child(
                                Button::new("el-remote-mode-existing", "Existing daemon")
                                    .label_size(LabelSize::Default)
                                    .toggle_state(self.mode == Mode::Existing)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode = Mode::Existing;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("el-remote-mode-install", "New server over SSH")
                                    .label_size(LabelSize::Default)
                                    .toggle_state(self.mode == Mode::Install)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode = Mode::Install;
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                fields = fields
                    .child(field_row("name", self.name.clone()))
                    .child(field_row("url", self.url.clone()));
                if editing || self.mode == Mode::Existing {
                    fields = fields.child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().child(field_row("token variable", self.token_var.clone())))
                            .child(
                                Button::new("el-remote-gen-token", "Generate")
                                    .label_size(LabelSize::Default)
                                    .tooltip(ui::Tooltip::text(
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
                        // Update the field to the variable we wrote.
                        fields = fields.child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(
                                    Label::new(format!(
                                        "Token written to .env as {var}. Put the same value \
                                         in the server's /etc/zdbt-el-serve/env:"
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Success),
                                )
                                .child(
                                    Button::new("el-remote-copy-token", "Copy token")
                                        .label_size(LabelSize::Small)
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                value.clone(),
                                            ));
                                        })),
                                ),
                        );
                    }
                } else {
                    fields = fields.child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(150.)).flex_shrink_0().child(
                                Label::new("token variable")
                                    .size(LabelSize::Default)
                                    .color(Color::Muted),
                            ))
                            .child(
                                Label::new(format!(
                                    "{} — generated for you",
                                    self.generated_token_var(cx)
                                ))
                                .size(LabelSize::Default)
                                .color(Color::Muted),
                            ),
                    );
                }
                fields.child(
                    Label::new(
                        "https is required beyond localhost. The token itself lives in .env \
                         under that variable — only its name is stored in YAML.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
            }
            (false, Step::Install) => v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_2()
                .child(field_row("ssh", self.ssh_host.clone()))
                .child(field_row("project dir", self.ssh_project.clone()))
                .child(field_row("profile", self.ssh_profile.clone()))
                .child(
                    Label::new(
                        "Runs the zdbt-el installer on the server through your own ssh \
                         (sudo needed): builds zdbt-el-serve, creates the service and a \
                         systemd unit. The token is generated here and sent over stdin.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
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
                let mut review = v_flex().w_full().px_4().py_3().gap_2().child(
                    Label::new("This is what gets written:")
                        .size(LabelSize::Default)
                        .color(Color::Muted),
                );
                for block in [yaml, env_line] {
                    let mut pre = v_flex()
                        .w_full()
                        .p_3()
                        .rounded_md()
                        .bg(colors.editor_background)
                        .border_1()
                        .border_color(colors.border);
                    for line in block.lines() {
                        pre = pre.child(Label::new(line.to_owned()).size(LabelSize::Default));
                    }
                    review = review.child(pre);
                }
                if self.mode == Mode::Install {
                    let host = self.ssh_host.read(cx).text(cx).trim().to_owned();
                    let dir = self.ssh_project.read(cx).text(cx).trim().to_owned();
                    let profile = self.ssh_profile.read(cx).text(cx).trim().to_owned();
                    review = review.child(
                        Label::new(format!(
                            "Then installs the daemon on {host} under {} running profile {} \
                             (a terminal tab shows progress).",
                            if dir.is_empty() { "/srv/el-project" } else { &dir },
                            if profile.is_empty() { "prod" } else { &profile }
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    );
                }
                review
            }
        };
        card = card.child(body);

        if let Some(error) = &self.error {
            card = card.child(div().px_4().pb_2().child(
                Label::new(error.clone()).size(LabelSize::Small).color(Color::Error),
            ));
        }

        let mut footer = h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_2()
            .border_t_1()
            .border_color(colors.border);
        if editing {
            footer = footer.child(
                Button::new(
                    "el-remote-delete",
                    if self.delete_armed { "Confirm remove" } else { "Remove" },
                )
                .label_size(LabelSize::Default)
                .color(Color::Error)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            );
        }
        footer = footer.child(div().flex_1());
        if !editing && self.step != Step::Details {
            footer = footer.child(
                Button::new("el-remote-back", "Back")
                    .label_size(LabelSize::Default)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.step = match (this.step, this.mode) {
                            (Step::Review, Mode::Install) => Step::Install,
                            _ => Step::Details,
                        };
                        this.error = None;
                        cx.notify();
                    })),
            );
        }
        footer = footer.child(
            Button::new("el-remote-cancel", "Cancel")
                .label_size(LabelSize::Default)
                .disabled(self.writing)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
        );
        let primary: SharedString = match (editing, self.step, self.mode) {
            (true, _, _) => if self.writing { "Saving…" } else { "Save changes" }.into(),
            (false, Step::Review, Mode::Install) => "Install & add server".into(),
            (false, Step::Review, Mode::Existing) => {
                if self.writing { "Saving…" } else { "Add server" }.into()
            }
            _ => "Next".into(),
        };
        card.child(
            footer.child(
                Button::new("el-remote-primary", primary)
                    .label_size(LabelSize::Default)
                    .style(ButtonStyle::Filled)
                    .disabled(self.writing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.editing.is_some() {
                            return this.save(window, cx);
                        }
                        match this.step {
                            Step::Details => {
                                if this.details_valid(cx) {
                                    this.step = if this.mode == Mode::Install {
                                        Step::Install
                                    } else {
                                        Step::Review
                                    };
                                }
                            }
                            Step::Install => {
                                let host = this.ssh_host.read(cx).text(cx).trim().to_owned();
                                if host.is_empty() || !host.contains('@') {
                                    this.fail("give the server as user@host".into(), cx);
                                    return;
                                }
                                this.error = None;
                                this.step = Step::Review;
                            }
                            Step::Review => match this.mode {
                                Mode::Existing => this.save(window, cx),
                                Mode::Install => this.install_over_ssh(window, cx),
                            },
                        }
                        cx.notify();
                    })),
            ),
        )
    }
}
