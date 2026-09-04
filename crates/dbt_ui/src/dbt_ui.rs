//! Native dbt UI for Zed: a bottom-dock results panel that runs
//! `dbt show --select <model>` and renders the returned rows as a data table,
//! plus a left-dock database explorer built from the dbt artifacts.

pub mod connection;
pub mod database;
pub mod database_panel;
pub mod dbt_install;
pub mod dbt_settings;
pub mod lineage;
pub mod lineage_sql;
pub mod mcp;
pub mod results_panel;

use gpui::{App, actions};
use workspace::Workspace;

pub use database_panel::DbtDatabasePanel;
pub use results_panel::DbtResultsPanel;

actions!(
    dbt,
    [
        /// Runs `dbt show` for the model in the active editor and displays the rows in the dbt results panel.
        ShowModelData,
        /// Toggles focus on the dbt results panel.
        ToggleFocus,
        /// Toggles focus on the dbt database explorer panel.
        ToggleDatabaseFocus,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|workspace, _: &ShowModelData, window, cx| {
            results_panel::show_model_data(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<DbtResultsPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &ToggleDatabaseFocus, window, cx| {
            workspace.toggle_panel_focus::<DbtDatabasePanel>(window, cx);
        });
    })
    .detach();
}
