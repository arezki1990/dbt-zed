use editor::Editor;
use gpui::AnyElement;
use ui::{Tooltip, prelude::*};

use super::QuickActionBar;

impl QuickActionBar {
    pub fn render_dbt_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_item = self.active_item.as_ref()?;
        let editor = active_item.act_as::<Editor>(cx)?;

        let is_dbt_model = editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|buffer| Some(buffer.read(cx).language()?.name()))
            .is_some_and(|name| name.as_ref() == "dbt SQL");
        if !is_dbt_model {
            return None;
        }

        let button = IconButton::new("dbt-show-model-data", IconName::DatabaseZap)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::for_action("Show Model Data", &dbt_ui::ShowModelData, cx)
            })
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(dbt_ui::ShowModelData), cx);
            });

        Some(button.into_any_element())
    }
}
