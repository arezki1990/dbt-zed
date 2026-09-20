use editor::Editor;
use gpui::AnyElement;
use ui::{Tooltip, prelude::*};

use super::QuickActionBar;

impl QuickActionBar {
    /// Deploy, for a pipeline spec open in the editor. The canvas has had
    /// this button since there was a canvas; someone editing the YAML
    /// directly had to go looking for it.
    pub fn render_el_deploy_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

        let button = IconButton::new("el-deploy-pipeline", IconName::Server)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::for_action("Deploy Pipeline", &dbt_ui::DeployPipeline, cx)
            })
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(dbt_ui::DeployPipeline), cx);
            });

        Some(button.into_any_element())
    }
}
