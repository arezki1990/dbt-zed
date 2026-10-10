use editor::Editor;
use gpui::AnyElement;
use ui::{Tooltip, prelude::*};

use super::QuickActionBar;

impl QuickActionBar {
    /// The way from an xDI mapping's file to its lineage: the file is the
    /// mapping's XML, the lineage what it reads and writes. Only for the
    /// mappings of the workspace's xDI project.
    pub fn render_xdi_lineage_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        let workspace = self.workspace.upgrade()?;
        if !dbt_ui::xdi::is_project_mapping(workspace.read(cx), &path, cx) {
            return None;
        }

        let button = IconButton::new("xdi-show-lineage", IconName::GitGraph)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::for_action("Show xDI Lineage", &dbt_ui::xdi::ShowLineage, cx)
            })
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(dbt_ui::xdi::ShowLineage), cx);
            });

        Some(button.into_any_element())
    }
}
