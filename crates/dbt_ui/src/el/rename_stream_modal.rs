//! The rename popup for a stream on the canvas. A stream's name is also
//! its incremental cursor's identity and, unless `target_table` pins
//! one, its warehouse table name — the popup says so before anything is
//! written, and the canvas does the write.

use std::sync::Arc;

use editor::{Editor, EditorEvent};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use super::canvas_item::ElPipelineCanvas;
use super::dialog;
use el_engine::spec::{Mode, Pipeline};

pub struct ElRenameStreamModal {
    canvas: WeakEntity<ElPipelineCanvas>,
    /// The spec as the canvas held it when the menu opened.
    pipeline: Arc<Pipeline>,
    original: String,
    editor: Entity<Editor>,
    _edits: Subscription,
}

impl ElRenameStreamModal {
    pub fn deploy(
        workspace: &mut Workspace,
        canvas: WeakEntity<ElPipelineCanvas>,
        pipeline: Arc<Pipeline>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        workspace.toggle_modal(window, cx, move |window, cx| {
            let editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("stream name", window, cx);
                editor.set_text(name.clone(), window, cx);
                editor.select_all(&Default::default(), window, cx);
                editor
            });
            // Typing re-checks the name and refreshes the table note.
            let edits = cx.subscribe(&editor, |_: &mut Self, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    cx.notify();
                }
            });
            Self {
                canvas,
                pipeline,
                original: name,
                editor,
                _edits: edits,
            }
        });
    }

    fn stream(&self) -> Option<&el_engine::spec::StreamSpec> {
        self.pipeline
            .streams
            .iter()
            .find(|stream| stream.name == self.original)
    }

    fn typed(&self, cx: &App) -> String {
        self.editor.read(cx).text(cx).trim().to_owned()
    }

    /// Why the typed name can't be used, or None. Checked as you type, so
    /// the button is never pressable for a name that would be refused.
    fn problem(&self, cx: &App) -> Option<SharedString> {
        let to = self.typed(cx);
        if to.is_empty() {
            return Some("Enter a name.".into());
        }
        if to == self.original {
            return None;
        }
        if self.pipeline.streams.iter().any(|stream| stream.name == to) {
            return Some(format!("{to} already exists in this pipeline.").into());
        }
        // The derived table follows the name, so a new name can land on a
        // table another stream already loads — they'd overwrite each other.
        let stream = self.stream()?;
        let mut renamed = stream.clone();
        renamed.name = to.clone();
        let table = renamed.target_table(&self.pipeline.target).to_uppercase();
        self.pipeline
            .streams
            .iter()
            .find(|other| {
                other.name != self.original
                    && other.target_table(&self.pipeline.target).to_uppercase() == table
            })
            .map(|other| {
                format!(
                    "{to} would load the same table as {} — pick another name.",
                    other.name
                )
                .into()
            })
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.typed(cx);
        if to == self.original {
            cx.emit(DismissEvent);
            return;
        }
        if self.problem(cx).is_some() {
            return;
        }
        let from = self.original.clone();
        self.canvas
            .update(cx, |canvas, cx| canvas.rename_stream(from, to, window, cx))
            .ok();
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for ElRenameStreamModal {}
impl ModalView for ElRenameStreamModal {}

impl Focusable for ElRenameStreamModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl Render for ElRenameStreamModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typed = self.typed(cx);
        let incremental = self.stream().is_some_and(|stream| {
            stream.mode(self.pipeline.defaults.as_ref()) == Mode::Incremental
        });
        // With no pinned target_table the warehouse table is derived from
        // the name: show the old and the would-be new one.
        let table_note: Option<SharedString> = self
            .stream()
            .filter(|stream| stream.target_table.is_none())
            .map(|stream| {
                let old = stream.target_table(&self.pipeline.target);
                let mut renamed = stream.clone();
                renamed.name = if typed.is_empty() {
                    self.original.clone()
                } else {
                    typed.clone()
                };
                let new = renamed.target_table(&self.pipeline.target);
                if new == old {
                    format!("The target table follows the name: {old}.").into()
                } else {
                    format!("The target table follows the name: {old} becomes {new}.").into()
                }
            });
        let problem = self.problem(cx);
        let focus = self.focus_handle(cx);

        dialog::frame(dialog::NARROW, cx)
            .key_context("ElRenameStreamModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.confirm(window, cx)))
            .child(dialog::header(
                "Rename stream",
                Some(self.original.clone().into()),
                cx.listener(|_, _, _, cx| cx.emit(DismissEvent)),
                cx,
            ))
            .child(
                dialog::body()
                    .child(dialog::field("New name", &self.editor, None, window, cx))
                    .when(incremental, |body| {
                        body.child(dialog::notice(
                            Severity::Warning,
                            "Renaming resets this stream's incremental cursor — the next run \
                             re-extracts every row.",
                        ))
                    })
                    .children(table_note.map(|note| dialog::notice(Severity::Info, note)))
                    // An empty field is the one problem not worth shouting about.
                    .children(
                        problem
                            .clone()
                            .filter(|_| !typed.is_empty())
                            .map(|problem| dialog::notice(Severity::Error, problem)),
                    ),
            )
            .child(dialog::footer(
                [],
                [
                    dialog::cancel_button("el-rename-cancel", &focus, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
                        .into_any_element(),
                    dialog::primary_button("el-rename-confirm", "Rename stream", &focus, cx)
                        .disabled(problem.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx)))
                        .into_any_element(),
                ],
                cx,
            ))
    }
}
