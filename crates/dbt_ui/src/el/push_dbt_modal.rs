//! Pushes the project's dbt code to an Ellode platform project as a
//! bundle: the files git knows under the project, less what dbt makes and
//! what holds a credential (`el_engine::dbt_bundle`), through the platform
//! remote's access token. The platform checks it again, keeps it and makes
//! it current: the project's dbt runs take it from then on.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, SharedString, Task, Window,
};
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use super::dialog;
use super::platform::{self, PlatformRemote};
use el_engine::dbt_bundle::Packed;

enum Packing {
    Packing,
    Ready(Arc<Packed>),
    Failed(SharedString),
}

pub struct ElPushDbtModal {
    focus_handle: FocusHandle,
    root: PathBuf,
    /// The project's platform remotes: their names in remotes.yml, and where
    /// each points.
    remotes: Vec<(SharedString, PlatformRemote)>,
    selected: usize,
    packing: Packing,
    pushing: bool,
    outcome: Option<Result<SharedString, SharedString>>,
    _pack: Task<()>,
    _push: Task<()>,
}

/// The platform remotes `root` declares, by name.
fn platform_remotes(root: &std::path::Path) -> Vec<(SharedString, PlatformRemote)> {
    el_engine::spec::load_remotes(&super::el_dir(root).join("remotes.yml"))
        .map(|remotes| remotes.remotes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, remote)| {
            platform::platform_remote(&remote.url).map(|platform| (SharedString::from(name), platform))
        })
        .collect()
}

/// A size as people read it.
fn size(bytes: u64) -> String {
    match bytes {
        bytes if bytes < 1024 => format!("{bytes} B"),
        bytes if bytes < 1024 * 1024 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        bytes => format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0)),
    }
}

impl ElPushDbtModal {
    pub fn deploy(
        workspace: &mut Workspace,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let remotes = platform_remotes(&root);
        if remotes.is_empty() {
            super::toast(
                workspace,
                "No Ellode platform remote yet: add one (Servers, then Add server, Ellode platform) to push the dbt project to.",
                cx,
            );
            return;
        }
        workspace.toggle_modal(window, cx, move |_, cx| {
            let mut this = Self {
                focus_handle: cx.focus_handle(),
                root,
                remotes,
                selected: 0,
                packing: Packing::Packing,
                pushing: false,
                outcome: None,
                _pack: Task::ready(()),
                _push: Task::ready(()),
            };
            this.pack(cx);
            this
        });
    }

    /// Packs the project off the UI thread: git and a few thousand reads.
    fn pack(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let packing = cx.background_spawn(async move { el_engine::dbt_bundle::pack(&root) });
        self._pack = cx.spawn(async move |this, cx| {
            let packed = packing.await;
            this.update(cx, |this, cx| {
                this.packing = match packed {
                    Ok(packed) => Packing::Ready(Arc::new(packed)),
                    Err(error) => Packing::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        });
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let Packing::Ready(packed) = &self.packing else {
            return;
        };
        let Some((name, remote)) = self.remotes.get(self.selected).cloned() else {
            return;
        };
        if self.pushing {
            return;
        }
        let packed = Arc::clone(packed);
        let root = self.root.clone();
        self.pushing = true;
        self.outcome = None;
        cx.notify();
        let pushing = cx.background_spawn(async move {
            let client = el_engine::server::RemoteClient::connect(&root, &name)?;
            client.push_dbt_bundle(&packed.gz, packed.commit.as_deref())
        });
        self._push = cx.spawn(async move |this, cx| {
            let result = pushing.await;
            this.update(cx, |this, cx| {
                this.pushing = false;
                this.outcome = Some(match result {
                    Ok(pushed) if pushed.unchanged => Ok(format!(
                        "{} already had this project: bundle {} is current again.",
                        remote.project, pushed.version
                    )
                    .into()),
                    Ok(pushed) => Ok(format!(
                        "Bundle {} ({}, {} files) is current on {}: its dbt runs take it.",
                        pushed.version, pushed.dbt_project, pushed.files, remote.project
                    )
                    .into()),
                    Err(error) => Err(platform::refusal_text(&remote, &error)
                        .unwrap_or_else(|| format!("{error:#}"))
                        .into()),
                });
                cx.notify();
            })
            .ok();
        });
    }
}

impl EventEmitter<DismissEvent> for ElPushDbtModal {}
impl ModalView for ElPushDbtModal {}

impl Focusable for ElPushDbtModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ElPushDbtModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_handle.clone();
        let target = self.remotes.get(self.selected).map(|(_, remote)| remote.clone());
        let mut body = dialog::body();
        if self.remotes.len() > 1 {
            let chips = self.remotes.iter().enumerate().map(|(ix, (name, remote))| {
                dialog::chip(
                    SharedString::from(format!("el-push-remote-{name}")),
                    format!("{} · {}", remote.project, remote.host),
                    ix == self.selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = ix;
                    this.outcome = None;
                    cx.notify();
                }))
            });
            body = body.child(dialog::labeled("To", dialog::chips(chips)));
        }
        body = match &self.packing {
            Packing::Packing => body.child(dialog::loading("Reading the dbt project…")),
            Packing::Failed(error) => body.child(dialog::notice(Severity::Error, error.clone())),
            Packing::Ready(packed) => {
                let commit = match &packed.commit {
                    Some(commit) => format!("at commit {}", &commit[..7]),
                    None => "with changes not committed".to_owned(),
                };
                let mut body = body.child(dialog::notice(
                    Severity::Info,
                    format!(
                        "{} files, {} ({} packed), {commit}. Left out: target/, logs/, el/, dotenv files, keys and profiles.yml — the platform writes the profile.",
                        packed.files,
                        size(packed.bytes),
                        size(packed.gz.len() as u64)
                    ),
                ));
                if !packed.skipped.is_empty() {
                    let listed: Vec<String> = packed
                        .skipped
                        .iter()
                        .take(5)
                        .map(|(path, why)| format!("{path} ({why})"))
                        .collect();
                    let more = packed.skipped.len().saturating_sub(listed.len());
                    body = body.child(dialog::notice(
                        Severity::Warning,
                        format!(
                            "Not in the bundle: {}{}",
                            listed.join(", "),
                            if more > 0 { format!(" and {more} more") } else { String::new() }
                        ),
                    ));
                }
                body
            }
        };
        if self.pushing {
            body = body.child(dialog::loading("Pushing…"));
        }
        match &self.outcome {
            Some(Ok(text)) => body = body.child(dialog::notice(Severity::Success, text.clone())),
            Some(Err(text)) => body = body.child(dialog::notice(Severity::Error, text.clone())),
            None => {}
        }
        let ready = matches!(self.packing, Packing::Ready(_)) && !self.pushing;
        let pushed = matches!(self.outcome, Some(Ok(_)));
        let label = match &target {
            Some(remote) => format!("Push to {}", remote.project),
            None => "Push".to_owned(),
        };

        dialog::frame(dialog::NARROW, cx)
            .track_focus(&self.focus_handle)
            .key_context("ElPushDbtModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.push(cx)))
            .child(dialog::header(
                "Push dbt project",
                target.map(|remote| SharedString::from(format!("{} on {}", remote.project, remote.host))),
                cx.listener(|_, _, _, cx| cx.emit(DismissEvent)),
                cx,
            ))
            .child(body)
            .child(dialog::footer(
                [],
                [
                    dialog::cancel_button("el-push-dbt-cancel", &focus, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
                        .into_any_element(),
                    dialog::primary_button(
                        "el-push-dbt-confirm",
                        if pushed { "Push again".to_owned() } else { label },
                        &focus,
                        cx,
                    )
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, _, cx| this.push(cx)))
                    .into_any_element(),
                ],
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_platform_remotes_are_pushed_to() {
        let dir = tempfile::tempdir().unwrap();
        let el = dir.path().join("el");
        std::fs::create_dir_all(&el).unwrap();
        std::fs::write(
            el.join("remotes.yml"),
            "version: 1\nremotes:\n  vm: { url: 'https://el.example.com:7431', token: '${ELLODE_TOKEN}' }\n  \
             shop: { url: 'https://console.example.com/api/platform/projects/shop', token: '${ELLODE_PAT_SHOP}', live: false }\n",
        )
        .unwrap();
        let remotes = platform_remotes(dir.path());
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].0.as_ref(), "shop");
        assert_eq!(remotes[0].1.project, "shop");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3 * 1024 * 1024), "3.0 MiB");
    }
}
