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
use el_engine::spec::{Mode, Pipeline};

pub struct ElRenameStreamModal {
    canvas: WeakEntity<ElPipelineCanvas>,
    /// The spec as the canvas held it when the menu opened.
    pipeline: Arc<Pipeline>,
    original: String,
    editor: Entity<Editor>,
    error: Option<SharedString>,
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
            // Typing clears a stale error and refreshes the table note.
            let edits = cx.subscribe(&editor, |this: &mut Self, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    this.error = None;
                    cx.notify();
                }
            });
            Self {
                canvas,
                pipeline,
                original: name,
                editor,
                error: None,
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

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.typed(cx);
        if to.is_empty() {
            self.error = Some("Enter a name.".into());
            cx.notify();
            return;
        }
        if to == self.original {
            cx.emit(DismissEvent);
            return;
        }
        if self.pipeline.streams.iter().any(|stream| stream.name == to) {
            self.error = Some(format!("{to} already exists in this pipeline.").into());
            cx.notify();
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
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

        let mut card = v_flex()
            .key_context("ElRenameStreamModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.confirm(window, cx)))
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
                    .child(Label::new(format!("Rename {}", self.original)).size(LabelSize::Small))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("el-rename-close", IconName::Close)
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
            .child(
                div().w_full().p_2().child(
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.editor_background)
                        .child(self.editor.clone()),
                ),
            );

        let mut notes = v_flex().w_full().px_2().pb_2().gap_1();
        if incremental {
            notes = notes.child(
                Label::new(
                    "Renaming resets this stream's incremental cursor — the next run \
                     re-extracts every row.",
                )
                .size(LabelSize::XSmall)
                .color(Color::Warning),
            );
        }
        if let Some(note) = table_note {
            notes = notes.child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted));
        }
        if let Some(error) = &self.error {
            notes = notes.child(
                Label::new(error.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Error),
            );
        }
        card = card.child(notes);

        card.child(
            h_flex()
                .w_full()
                .p_2()
                .gap_1()
                .border_t_1()
                .border_color(colors.border)
                .child(div().flex_1())
                .child(
                    Button::new("el-rename-cancel", "Cancel")
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                )
                .child(
                    Button::new("el-rename-confirm", "Rename stream")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                ),
        )
    }
}
