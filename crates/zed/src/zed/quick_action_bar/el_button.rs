use editor::Editor;
use gpui::AnyElement;
use ui::{Tooltip, prelude::*};

use super::QuickActionBar;

impl QuickActionBar {
    /// The switch from a pipeline's YAML to its canvas, for a pipeline spec
    /// open in the editor — the counterpart of the canvas's "Open YAML".
    /// Run and Deploy stay on the canvas, which shows what they would act
    /// on; it wears the icon the canvas's tab does.
    pub fn render_el_canvas_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_item = self.active_item.as_ref()?;
        let editor = active_item.act_as::<Editor>(cx)?;
        let path = editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()?
            .read(cx)
            .file()?
            .as_local()?
            .abs_path(cx);
        if !dbt_ui::el::is_pipeline_file(&path) {
            return None;
        }

        let button = IconButton::new("el-open-canvas", IconName::ArrowRightLeft)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::for_action("Open Pipeline Canvas", &dbt_ui::OpenPipelines, cx)
            })
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(dbt_ui::OpenPipelines), cx);
            });

        Some(button.into_any_element())
    }
}
