//! What the EL dialogs are built from, so they look and behave alike: a
//! header with a close button, labeled fields in bordered boxes, choice
//! chips whose selection is filled rather than merely tinted text, notices
//! that lead with an icon, a spinner line, and a footer whose buttons show
//! their keys — ⏎ confirms and esc cancels, as on every macOS sheet. Tab and
//! shift-tab walk a dialog's fields.

use editor::Editor;
use gpui::{ClickEvent, Entity, FocusHandle, Focusable as _, FontWeight};
use ui::{CommonAnimationExt as _, KeyBinding, Tooltip, prelude::*};

/// Rename, deploy, and the canvas's add-stream and target forms.
pub const NARROW: Pixels = px(480.);
/// The connection and server forms, which carry more fields.
pub const WIDE: Pixels = px(560.);

const LABEL_WIDTH: Pixels = px(120.);

/// The dialog surface; children stack top to bottom.
pub fn frame(width: Pixels, cx: &App) -> Div {
    v_flex().w(width).elevation_3(cx).overflow_hidden()
}

pub fn header(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .px_3()
        .py_2()
        .gap_2()
        .items_start()
        .border_b_1()
        .border_color(cx.theme().colors().border_variant)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    Label::new(title)
                        .size(LabelSize::Default)
                        .weight(FontWeight::SEMIBOLD),
                )
                .children(detail.map(|detail| {
                    Label::new(detail)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate()
                })),
        )
        .child(
            IconButton::new("el-dialog-close", IconName::Close)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
                .tooltip(Tooltip::text("Close"))
                .on_click(on_close),
        )
}

/// The scrolling middle of a dialog.
pub fn body() -> Div {
    v_flex().w_full().px_3().py_3().gap_3()
}

/// A label beside its input box, with an optional hint under the box. The
/// border lights up while the field has focus, so it is always clear where
/// typing lands.
pub fn field(
    label: impl Into<SharedString>,
    editor: &Entity<Editor>,
    hint: Option<SharedString>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let focused = editor.read(cx).focus_handle(cx).is_focused(window);
    labeled(
        label,
        v_flex()
            .w_full()
            .gap_0p5()
            .child(
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if focused {
                        colors.border_focused
                    } else {
                        colors.border_variant
                    })
                    .bg(colors.editor_background)
                    .child(editor.clone()),
            )
            .children(
                hint.map(|hint| Label::new(hint).size(LabelSize::XSmall).color(Color::Muted)),
            ),
    )
}

/// A label beside any control.
pub fn labeled(label: impl Into<SharedString>, control: impl IntoElement) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(LABEL_WIDTH)
                .flex_shrink_0()
                .pt_1()
                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted)),
        )
        .child(div().flex_1().min_w_0().child(control))
        .into_any_element()
}

/// One option of a single-choice row.
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Button {
    Button::new(id, label)
        .label_size(LabelSize::Small)
        .style(ButtonStyle::Outlined)
        .toggle_state(selected)
        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
}

/// Chips that wrap onto more lines rather than overflow the dialog.
pub fn chips(chips: impl IntoIterator<Item = Button>) -> Div {
    h_flex().w_full().gap_1().flex_wrap().children(chips)
}

pub fn notice(severity: Severity, text: impl Into<SharedString>) -> impl IntoElement {
    let (icon, color) = match severity {
        Severity::Info => (IconName::Info, Color::Muted),
        Severity::Success => (IconName::Check, Color::Success),
        Severity::Warning => (IconName::Warning, Color::Warning),
        Severity::Error => (IconName::XCircle, Color::Error),
    };
    h_flex()
        .w_full()
        .gap_1p5()
        .items_start()
        .child(
            div()
                .pt(px(2.))
                .child(Icon::new(icon).size(IconSize::Small).color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(match severity {
                            Severity::Info => Color::Muted,
                            _ => color,
                        }),
                ),
        )
}

pub fn loading(text: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .items_center()
        .child(
            Icon::new(IconName::LoadCircle)
                .size(IconSize::Small)
                .color(Color::Muted)
                .with_rotate_animation(2),
        )
        .child(Label::new(text).size(LabelSize::Small).color(Color::Muted))
}

/// Secondary actions on the left, then Cancel and the primary action.
pub fn footer(
    start: impl IntoIterator<Item = AnyElement>,
    end: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .px_3()
        .py_2()
        .gap_1()
        .items_center()
        .border_t_1()
        .border_color(cx.theme().colors().border_variant)
        .children(start)
        .child(div().flex_1())
        .children(end)
}

pub fn cancel_button(id: &'static str, focus: &FocusHandle, cx: &App) -> Button {
    Button::new(id, "Cancel")
        .label_size(LabelSize::Small)
        .key_binding(KeyBinding::for_action_in(&menu::Cancel, focus, cx).size(rems_from_px(12_f32)))
}

/// The dialog's default button: filled, and the one ⏎ presses.
pub fn primary_button(
    id: &'static str,
    label: impl Into<SharedString>,
    focus: &FocusHandle,
    cx: &App,
) -> Button {
    Button::new(id, label)
        .label_size(LabelSize::Small)
        .style(ButtonStyle::Filled)
        .key_binding(
            KeyBinding::for_action_in(&menu::Confirm, focus, cx).size(rems_from_px(12_f32)),
        )
}

/// Removal is never the default button, and reads as what it is.
pub fn danger_button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .label_size(LabelSize::Small)
        .style(ButtonStyle::Tinted(ui::TintColor::Error))
}

/// Tab and shift-tab: focus the next or previous of `fields`, wrapping;
/// from anywhere else, the first or the last.
pub fn cycle_focus(fields: &[FocusHandle], forward: bool, window: &mut Window, cx: &mut App) {
    let count = fields.len();
    if count == 0 {
        return;
    }
    let next = match (
        fields.iter().position(|field| field.is_focused(window)),
        forward,
    ) {
        (Some(index), true) => (index + 1) % count,
        (Some(index), false) => (index + count - 1) % count,
        (None, true) => 0,
        (None, false) => count - 1,
    };
    if let Some(field) = fields.get(next) {
        window.focus(field, cx);
    }
}

pub fn focus_handle_of(editor: &Entity<Editor>, cx: &App) -> FocusHandle {
    editor.read(cx).focus_handle(cx)
}
