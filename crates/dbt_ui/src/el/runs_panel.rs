//! The EL Runs bottom-dock panel — the plugin's own run surface, hosting
//! [`super::run_view::ElRunView`]. Independent of the dbt results panel.

use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity,
    Window,
};
use ui::prelude::*;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::run_view::ElRunView;
use crate::ToggleElRunsFocus;

pub struct ElRunsPanel {
    focus_handle: FocusHandle,
    run_view: Entity<ElRunView>,
    /// A preview table (stream sample or failed casts) — the plugin's own
    /// grid, independent of the dbt results panel.
    preview: Option<PreviewTable>,
    scroll: gpui::UniformListScrollHandle,
}

pub struct PreviewTable {
    pub title: SharedString,
    pub columns: std::sync::Arc<Vec<SharedString>>,
    pub rows: std::sync::Arc<Vec<Vec<SharedString>>>,
}

impl ElRunsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            Self::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        cx.new(|cx| {
            let run_view = cx.new(|cx| ElRunView::new(workspace_handle.clone(), cx));
            Self {
                focus_handle: cx.focus_handle(),
                run_view,
                preview: None,
                scroll: gpui::UniformListScrollHandle::new(),
            }
        })
    }

    pub fn run_view(&self) -> Entity<ElRunView> {
        self.run_view.clone()
    }

    pub fn show_preview(
        &mut self,
        title: SharedString,
        columns: Vec<SharedString>,
        rows: Vec<Vec<SharedString>>,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(PreviewTable {
            title,
            columns: std::sync::Arc::new(columns),
            rows: std::sync::Arc::new(rows),
        });
        cx.notify();
    }

    pub fn show_runs(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for ElRunsPanel {}

impl Focusable for ElRunsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ElRunsPanel {
    fn persistent_name() -> &'static str {
        "EL Runs Panel"
    }

    fn panel_key() -> &'static str {
        "ElRunsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayFilled)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("EL Runs")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElRunsFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}

impl Render for ElRunsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let body: gpui::AnyElement = match &self.preview {
            None => self.run_view.clone().into_any_element(),
            Some(preview) => {
                const COL_WIDTH: f32 = 170.;
                let columns = preview.columns.clone();
                let rows = preview.rows.clone();
                let total = px(columns.len() as f32 * COL_WIDTH);
                let header = h_flex().w(total).flex_shrink_0().children(
                    columns.iter().map(|column| {
                        div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
                            Label::new(column.clone())
                                .size(LabelSize::XSmall)
                                .color(Color::Accent)
                                .truncate(),
                        )
                    }),
                );
                let count = rows.len();
                let list = gpui::uniform_list("el-preview-rows", count, {
                    move |range, _, _| {
                        range
                            .filter_map(|ix| rows.get(ix))
                            .map(|row| {
                                h_flex()
                                    .w(total)
                                    .h(px(24.))
                                    .flex_shrink_0()
                                    .children(row.iter().map(|cell| {
                                        div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
                                            Label::new(cell.clone())
                                                .size(LabelSize::XSmall)
                                                .truncate(),
                                        )
                                    }))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }
                })
                .flex_1()
                .track_scroll(&self.scroll);

                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .w_full()
                            .p_1()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                IconButton::new("el-preview-back", IconName::ArrowLeft)
                                    .icon_size(IconSize::Small)
                                    .tooltip(ui::Tooltip::text("Back to runs"))
                                    .on_click(cx.listener(|this, _, _, cx| this.show_runs(cx))),
                            )
                            .child(
                                Label::new(preview.title.clone()).size(LabelSize::Small),
                            )
                            .child(
                                Label::new(format!("{count} rows"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        div()
                            .id("el-preview-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_x_scroll()
                            .child(v_flex().w(total).h_full().child(header).child(list)),
                    )
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("ElRunsPanel")
            .bg(colors.panel_background)
            .child(body)
    }
}
