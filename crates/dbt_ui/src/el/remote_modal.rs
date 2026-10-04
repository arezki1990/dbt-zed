//! The remote editor: add, edit, or delete a server in el/remotes.yml
//! from the sidebar. A remote is a name, an https URL, and the NAME of
//! the environment variable holding its token — the token value itself
//! never enters the form or the file. A project of the Ellode platform is
//! one too, reached through its console with a personal access token,
//! which is taken from the clipboard straight into `.env`.

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
use super::platform;

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
    /// A project of the Ellode platform, through its web console, with a
    /// personal access token.
    Platform,
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
    /// Env var NAME (e.g. ELLODE_TOKEN); stored as "${NAME}".
    token_var: Entity<Editor>,
    /// Install over SSH: user@host, the project dir, the profile to run.
    ssh_host: Entity<Editor>,
    ssh_project: Entity<Editor>,
    ssh_profile: Entity<Editor>,
    /// A platform remote: the console's address and the project.
    console: Entity<Editor>,
    platform_project: Entity<Editor>,
    /// An access token taken from the clipboard, written to .env on save.
    /// Only its prefix is ever displayed.
    pasted: Option<String>,
    /// Whether the remote being edited has a live socket (`live`), kept
    /// unless its URL turns out to be a console's.
    live: bool,
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

/// "${ELLODE_TOKEN}" → "ELLODE_TOKEN" for display.
fn var_name_of(template: &str) -> String {
    template
        .trim()
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(template.trim())
        .to_owned()
}

/// A variable's NAME, never a value: no spaces, no quotes, not long.
fn is_var_name(var: &str) -> bool {
    !var.is_empty() && var.len() <= 64 && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Puts an access token in the project's `.env` under `var` — its line
/// rewritten when it has one — readable by its owner only, once git is
/// known never to carry the file.
async fn store_token(root: &std::path::Path, var: &str, value: &str) -> Result<()> {
    use std::io::Write as _;
    let exclude = super::local_runner::dotenv_exclusion(root).await?;
    if let Some(exclude) = exclude {
        super::local_runner::append_line(&exclude, ".env")
            .map_err(|error| anyhow::anyhow!("could not exclude .env from git: {error:#}"))?;
    }
    let path = root.join(".env");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => bail!("could not read .env: {error}"),
    };
    let write = || -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
            options.mode(0o600);
            let mut file = options.open(&path)?;
            // An existing file keeps its mode through a truncate: tighten it.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(platform::with_env_line(&existing, var, value).as_bytes())
        }
        #[cfg(not(unix))]
        {
            let mut file = options.open(&path)?;
            file.write_all(platform::with_env_line(&existing, var, value).as_bytes())
        }
    };
    write().map_err(|error| anyhow::anyhow!("could not write .env: {error}"))
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
            "ELLODE_TOKEN",
            &existing
                .as_ref()
                .and_then(|remote| remote.token.as_deref())
                .map(var_name_of)
                .unwrap_or_default(),
        );
        let ssh_host = make("deploy@el.example.com", "");
        let ssh_project = make("/srv/el-project", "");
        let ssh_profile = make("prod", "");
        // A console remote is edited as one: its console and project, not
        // the URL they make.
        let console_remote = existing
            .as_ref()
            .filter(|remote| platform::platform_remote(&remote.url).is_some())
            .and_then(|remote| platform::console_address(&remote.url).ok());
        let console = make(
            "https://el-console.example.com",
            console_remote
                .as_ref()
                .map(|(console, _)| console.as_str())
                .unwrap_or(""),
        );
        let platform_project = make(
            "shop",
            console_remote
                .as_ref()
                .and_then(|(_, project)| project.as_deref())
                .unwrap_or(""),
        );
        let mode = if console_remote.is_some() {
            Mode::Platform
        } else {
            Mode::Existing
        };
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
            console,
            platform_project,
            pasted: None,
            live: existing.as_ref().is_none_or(|remote| remote.live),
            step: Step::Details,
            mode,
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

    /// Writes remotes.yml — after `token` (variable, value), when there is
    /// one, has gone into `.env`: never into a `.env` git tracks, and one
    /// git does not ignore yet is excluded from this clone first.
    fn write(
        &mut self,
        remotes: Remotes,
        done: String,
        token: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.writing = true;
        cx.notify();
        let workspace = self.workspace.clone();
        let panel = self.panel.clone();
        let project = self.project.clone();
        let root = self.root.clone();
        let path = super::el_dir(&self.root).join("remotes.yml");
        cx.spawn_in(window, async move |this, cx| {
            let stored = match token {
                Some((var, value)) => store_token(&root, &var, &value).await,
                None => Ok(()),
            };
            let result = match stored {
                Ok(()) => {
                    super::spec_io::write_text(
                        workspace.clone(),
                        project,
                        path,
                        el_engine::spec::to_canonical_remotes_yaml(&remotes),
                        cx,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
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

    /// A platform remote's URL and project, from the console and project
    /// fields — the project read from the console's address when that
    /// names one and the field is empty.
    fn platform_target(&self, cx: &App) -> std::result::Result<(String, String), String> {
        let (console, named) = platform::console_address(&self.console.read(cx).text(cx))
            .map_err(|error| format!("{error:#}"))?;
        let typed = self.platform_project.read(cx).text(cx).trim().to_owned();
        let project = if typed.is_empty() {
            named.unwrap_or_default()
        } else {
            typed
        };
        if !platform::is_project_name(&project) {
            return Err(
                "give the project's name in the console: lower-case letters, digits and -".into(),
            );
        }
        let url = platform::remote_url(&console, &project);
        el_engine::server::check_remote_url(&url).map_err(|error| format!("{error:#}"))?;
        Ok((url, project))
    }

    /// The variable a platform remote's token goes in: the one typed, or
    /// `ELLODE_PAT_<PROJECT>`.
    fn platform_token_var(&self, project: &str, cx: &App) -> String {
        let typed = var_name_of(&self.token_var.read(cx).text(cx));
        if typed.is_empty() {
            platform::token_var_for(project)
        } else {
            typed
        }
    }

    /// Takes an access token from the clipboard, where the console's dialog
    /// put it, to write to .env on save. It is never displayed.
    fn paste_token(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        match text.as_deref().and_then(platform::access_token) {
            Some(token) => {
                self.pasted = Some(token.to_owned());
                self.error = None;
            }
            None => {
                self.error = Some(
                    "The clipboard holds no access token: copy it in the console's dialog \
                 (ellode_pat_…), then paste again."
                        .into(),
                )
            }
        }
        cx.notify();
    }

    /// The details step's checks, without writing anything.
    fn details_valid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
        if self.mode == Mode::Platform {
            let project = match self.platform_target(cx) {
                Ok((_, project)) => project,
                Err(message) => {
                    self.fail(message, cx);
                    return false;
                }
            };
            // What was read from a pasted address shows in the fields.
            if self.platform_project.read(cx).text(cx).trim().is_empty() {
                self.platform_project.update(cx, |editor, cx| {
                    editor.set_text(project.clone(), window, cx)
                });
            }
            let token_var = self.platform_token_var(&project, cx);
            if !is_var_name(&token_var) {
                self.fail(
                    "give the NAME of the environment variable for the token (e.g. \
                     ELLODE_PAT_SHOP) — the token itself is pasted, never typed"
                        .into(),
                    cx,
                );
                return false;
            }
            self.token_var
                .update(cx, |editor, cx| editor.set_text(token_var, window, cx));
            self.error = None;
            return true;
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
                     ELLODE_TOKEN) — put the value itself in .env"
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
    /// to ELLODE_TOKEN_<NAME>), then offered for copying to the server.
    fn generate_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut var = var_name_of(&self.token_var.read(cx).text(cx));
        if var.is_empty() {
            var = self.generated_token_var(cx);
        }
        if !(var.len() <= 64 && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return self.fail(
                "the token variable must be a plain NAME (e.g. ELLODE_TOKEN)".into(),
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
            "ELLODE_TOKEN_{}",
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
        let (url, live, project) = if self.mode == Mode::Platform {
            match self.platform_target(cx) {
                Ok((url, project)) => (url, false, Some(project)),
                Err(message) => return self.fail(message, cx),
            }
        } else {
            let url = self.url.read(cx).text(cx).trim().to_owned();
            if let Err(error) = el_engine::server::check_remote_url(&url) {
                return self.fail(format!("{error:#}"), cx);
            }
            // A console's URL typed in by hand is a console remote all the
            // same: it has no socket to attach to.
            let live = self.live && platform::platform_remote(&url).is_none();
            (url, live, None)
        };
        let token_var = match &project {
            Some(project) => self.platform_token_var(project, cx),
            None => var_name_of(&self.token_var.read(cx).text(cx)),
        };
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
                     (e.g. ELLODE_TOKEN) — put the value itself in .env"
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
        let value = RemoteSpec {
            url,
            token,
            live,
            extra,
        };
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
        // A token pasted for a console goes to .env; one left over from
        // before switching to another kind of server does not.
        let pasted = self
            .pasted
            .clone()
            .filter(|_| project.is_some())
            .map(|value| (token_var, value));
        self.write(remotes, done, pasted, window, cx);
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
        self.write(
            remotes,
            format!("Server {original} removed."),
            None,
            window,
            cx,
        );
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
            (true, _, Mode::Platform) | (false, Step::Details, Mode::Platform) => vec![
                self.name.clone(),
                self.console.clone(),
                self.platform_project.clone(),
                self.token_var.clone(),
            ],
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
                if self.details_valid(window, cx) {
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
                Mode::Existing | Mode::Platform => self.save(window, cx),
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

impl ElRemoteModal {
    /// A platform remote's fields: the console, the project, and the
    /// token's variable — the token itself taken from the clipboard.
    fn render_platform_details(
        &self,
        body: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let token_var = match self.platform_target(cx) {
            Ok((_, project)) => self.platform_token_var(&project, cx),
            Err(_) => var_name_of(&self.token_var.read(cx).text(cx)),
        };
        let body = body
            .child(dialog::field(
                "Console",
                &self.console,
                Some("Its address, as you open it in a browser.".into()),
                window,
                cx,
            ))
            .child(dialog::field(
                "Project",
                &self.platform_project,
                Some("Its name in the console: a token reaches that project only.".into()),
                window,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(dialog::field(
                        "Token variable",
                        &self.token_var,
                        Some(
                            "ELLODE_PAT_<PROJECT> when left empty. The token goes in .env.".into(),
                        ),
                        window,
                        cx,
                    )))
                    .child(
                        Button::new(
                            "el-remote-paste-token",
                            if self.pasted.is_some() {
                                "Paste again"
                            } else {
                                "Paste token"
                            },
                        )
                        .label_size(LabelSize::Small)
                        .start_icon(Icon::new(IconName::Lock).size(IconSize::Small))
                        .tooltip(Tooltip::text(
                            "Take the access token from the clipboard, where the console's \
                             dialog copies it. It goes to this project's .env, never into \
                             remotes.yml, and is never shown.",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.paste_token(cx))),
                    ),
            );
        body.child(match &self.pasted {
            Some(token) => dialog::notice(
                Severity::Success,
                format!(
                    "Token {}… taken from the clipboard: it goes to .env as {} when you save.",
                    platform::token_prefix(token),
                    if token_var.is_empty() {
                        "the variable above"
                    } else {
                        &token_var
                    },
                ),
            )
            .into_any_element(),
            None if self.editing.is_some() => dialog::notice(
                Severity::Info,
                "The token stays as .env has it. To replace one that expired or was revoked, \
                 make a new one on the console's Access tokens page, copy it, then Paste token.",
            )
            .into_any_element(),
            None => dialog::notice(
                Severity::Info,
                "Make a token on the console's Access tokens page, for this project — Operator \
                 to run pipelines, Admin to deploy them too — copy it, then Paste token.",
            )
            .into_any_element(),
        })
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
                                dialog::chip(
                                    "el-remote-mode-platform",
                                    "Ellode platform",
                                    self.mode == Mode::Platform,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.mode = Mode::Platform;
                                        cx.notify();
                                    },
                                )),
                            ]))
                            .child(
                                Label::new(match self.mode {
                                    Mode::Existing => {
                                        "Point at an ellode-serve daemon that is already running."
                                    }
                                    Mode::Install => {
                                        "Install the daemon on a Linux server through your own \
                                         ssh."
                                    }
                                    Mode::Platform => {
                                        "A project of the Ellode platform, through its web \
                                         console, with an access token of your own."
                                    }
                                })
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            ),
                    ));
                }
                body = body.child(dialog::field(
                    "Name",
                    &self.name,
                    Some("Letters, digits, _ and -.".into()),
                    window,
                    cx,
                ));
                if self.mode == Mode::Platform {
                    body = self.render_platform_details(body, window, cx);
                } else {
                    body = body.child(dialog::field(
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
                                         the server's /etc/ellode-serve/env."
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
                                                        gpui::ClipboardItem::new_string(
                                                            value.clone(),
                                                        ),
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
                    "Runs the Ellode installer on the server through your own ssh (sudo \
                     needed): builds ellode-serve, creates the service and a systemd unit. \
                     The token is generated here and sent over stdin.",
                )),
            (false, Step::Review) => {
                let name = self.name.read(cx).text(cx).trim().to_owned();
                let platform_target = (self.mode == Mode::Platform)
                    .then(|| self.platform_target(cx).ok())
                    .flatten();
                let url = match &platform_target {
                    Some((url, _)) => url.clone(),
                    None => self.url.read(cx).text(cx).trim().to_owned(),
                };
                let token_var = match (&platform_target, self.mode) {
                    (Some((_, project)), _) => self.platform_token_var(project, cx),
                    (None, Mode::Install) => self.generated_token_var(cx),
                    (None, _) => var_name_of(&self.token_var.read(cx).text(cx)),
                };
                let mut yaml = format!("# el/remotes.yml\nremotes:\n  {name}:\n    url: {url}\n");
                if !token_var.is_empty() {
                    yaml.push_str(&format!("    token: \"${{{token_var}}}\"\n"));
                }
                if platform_target.is_some() {
                    yaml.push_str("    live: false\n");
                }
                let env_line = if token_var.is_empty() {
                    "# .env — no token (loopback only)".to_owned()
                } else if self.mode == Mode::Install {
                    format!("# .env\n{token_var}=<generated on install>")
                } else if let Some(token) =
                    self.pasted.as_deref().filter(|_| platform_target.is_some())
                {
                    format!(
                        "# .env, readable by you only\n{token_var}=<the pasted token, {}…>",
                        platform::token_prefix(token)
                    )
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
                if let Some((_, project)) = &platform_target {
                    body = body.child(dialog::notice(
                        Severity::Info,
                        format!(
                            "Runs, deploys and explore calls go through the console as you, in \
                             project {project}, with at most the token's role. The console has \
                             no live socket, so the panel asks it every two seconds."
                        ),
                    ));
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
            (false, Step::Review, Mode::Existing | Mode::Platform) => if self.writing {
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
