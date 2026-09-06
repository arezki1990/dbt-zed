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
        let field_row = |label: &'static str, editor: Entity<Editor>| {
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(div().w(px(120.)).flex_shrink_0().child(
                    Label::new(label).size(LabelSize::XSmall).color(Color::Muted),
                ))
                .child(div().flex_1().child(editor))
        };
        let mut card = v_flex()
            .key_context("ElRemoteModal")
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
                        Label::new(if editing { "Edit server" } else { "Add server" })
                            .size(LabelSize::Small),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-remote-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .child(field_row("name", self.name.clone()))
                    .child(field_row("url", self.url.clone()))
                    .child(field_row("token variable", self.token_var.clone()))
                    .child(
                        Label::new(
                            "https is required beyond localhost. The token itself lives in \
                             .env under that variable — only its name is stored here.",
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    ),
            );
        if let Some(error) = &self.error {
            card = card.child(div().px_2().pb_1().child(
                Label::new(error.clone()).size(LabelSize::XSmall).color(Color::Error),
            ));
        }
        let mut footer = h_flex().w_full().p_2().gap_1().border_t_1().border_color(colors.border);
        if editing {
            footer = footer.child(
                Button::new(
                    "el-remote-delete",
                    if self.delete_armed { "Confirm remove" } else { "Remove" },
                )
                .label_size(LabelSize::Small)
                .color(Color::Error)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            );
        }
        card.child(
            footer
                .child(div().flex_1())
                .child(
                    Button::new("el-remote-cancel", "Cancel")
                        .label_size(LabelSize::Small)
                        .disabled(self.writing)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                )
                .child(
                    Button::new(
                        "el-remote-save",
                        if self.writing {
                            "Saving…"
                        } else if editing {
                            "Save changes"
                        } else {
                            "Add server"
                        },
                    )
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .disabled(self.writing)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                ),
        )
    }
}
