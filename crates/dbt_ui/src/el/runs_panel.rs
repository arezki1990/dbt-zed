//! The EL console — the plugin's bottom-dock surface. Two views: Runs
//! (hosting [`super::run_view::ElRunView`]) and Query (ad-hoc SQL against
//! any EL connection, through the on-demand worker). Mapping-editor
//! previews overlay either view and return with Back.

use std::path::PathBuf;
use std::time::Duration;

use editor::Editor;
use gpui::{
    App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use ui::prelude::*;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use super::run_view::ElRunView;
use crate::ToggleElRunsFocus;

const QUERY_ROW_CAP: usize = 500;

#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Runs,
    Query,
}

pub struct ElRunsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    run_view: Entity<ElRunView>,
    surface: Surface,
    /// A preview table (stream sample or failed casts) — overlays the
    /// current surface until Back.
    preview: Option<PreviewTable>,
    preview_scroll: UniformListScrollHandle,
    // Query surface state.
    sql: Entity<Editor>,
    /// (name, kind) — names only, credential posture as everywhere.
    connections: Vec<(SharedString, SharedString)>,
    selected: Option<usize>,
    root: Option<PathBuf>,
    running: bool,
    result: Option<PreviewTable>,
    result_scroll: UniformListScrollHandle,
    elapsed: Option<Duration>,
    query_error: Option<SharedString>,
    _query: Task<()>,
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
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        let sql = cx.new(|cx| {
            let mut editor = Editor::auto_height(3, 8, window, cx);
            editor.set_placeholder_text("SELECT …", window, cx);
            editor
        });
        cx.new(|cx| {
            let run_view = cx.new(|cx| ElRunView::new(workspace_handle.clone(), cx));
            Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                run_view,
                surface: Surface::Runs,
                preview: None,
                preview_scroll: UniformListScrollHandle::new(),
                sql,
                connections: Vec::new(),
                selected: None,
                root: None,
                running: false,
                result: None,
                result_scroll: UniformListScrollHandle::new(),
                elapsed: None,
                query_error: None,
                _query: Task::ready(()),
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

    /// The explorer's click-a-table entry point: switches to Query,
    /// selects the connection, seeds the SQL, and runs it.
    pub fn show_query_for_table(
        &mut self,
        connection: SharedString,
        schema: &str,
        table: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = None;
        self.surface = Surface::Query;
        self.refresh_connections(cx);
        self.selected = self
            .connections
            .iter()
            .position(|(name, _)| *name == connection)
            .or(self.selected);
        let quote = |ident: &str| format!("\"{}\"", ident.replace('"', "\"\""));
        let sql = format!(
            "SELECT * FROM {}.{} LIMIT 200",
            quote(schema),
            quote(table)
        );
        self.sql.update(cx, |editor, cx| {
            editor.set_text(sql, window, cx);
        });
        self.run_query(cx);
    }

    fn refresh_connections(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone().or_else(|| {
            self.workspace
                .upgrade()
                .and_then(|workspace| super::discover_el_root(workspace.read(cx), cx))
        });
        self.root = root.clone();
        let Some(root) = root else { return };
        self.connections =
            el_engine::spec::load_connections(&super::el_dir(&root).join("connections.yml"))
                .map(|connections| {
                    connections
                        .connections
                        .iter()
                        .map(|(name, connection)| {
                            (name.clone().into(), connection.kind().to_owned().into())
                        })
                        .collect()
                })
                .unwrap_or_default();
        if self.selected.map_or(true, |ix| ix >= self.connections.len()) {
            self.selected = (!self.connections.is_empty()).then_some(0);
        }
    }

    fn run_query(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.query_error = None;
        let Some(root) = self.root.clone() else {
            self.query_error = Some("No EL project in this workspace — open one or run el: initialize workspace.".into());
            cx.notify();
            return;
        };
        let Some((name, _)) = self.selected.and_then(|ix| self.connections.get(ix)) else {
            self.query_error = Some("Pick a connection first.".into());
            cx.notify();
            return;
        };
        let sql = self.sql.read(cx).text(cx).trim().to_owned();
        if sql.is_empty() {
            self.query_error =
                Some("Write a query first — or click a table in the EL panel.".into());
            cx.notify();
            return;
        }
        let Some(worker) = super::find_worker() else {
            self.query_error = Some(
                "Connector worker not found — build zdbt-el-worker or set ZDBT_EL_WORKER.".into(),
            );
            cx.notify();
            return;
        };
        let connection_name = name.to_string();
        self.running = true;
        self.result = None;
        self.elapsed = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            let started = std::time::Instant::now();
            let connections = el_engine::spec::load_connections(
                &super::el_dir(&root).join("connections.yml"),
            )?;
            let connection = connections
                .connections
                .get(&connection_name)
                .ok_or_else(|| anyhow::anyhow!("connection {connection_name:?} is gone from connections.yml"))?;
            let env = el_engine::env::EnvMap::load(&root, None);
            let result = el_engine::explore::run_query(
                &worker,
                &root,
                connection,
                &env,
                &sql,
                QUERY_ROW_CAP,
            )?;
            anyhow::Ok((result, started.elapsed()))
        });
        self._query = cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.running = false;
                match outcome {
                    Ok((result, elapsed)) => {
                        this.elapsed = Some(elapsed);
                        this.result = Some(PreviewTable {
                            title: "query".into(),
                            columns: std::sync::Arc::new(
                                result.columns.into_iter().map(Into::into).collect(),
                            ),
                            rows: std::sync::Arc::new(
                                result
                                    .rows
                                    .into_iter()
                                    .map(|row| row.into_iter().map(Into::into).collect())
                                    .collect(),
                            ),
                        });
                    }
                    Err(error) => this.query_error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn render_query(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let chips = h_flex().gap_1().flex_wrap().children(
            self.connections
                .iter()
                .enumerate()
                .map(|(ix, (name, kind))| {
                    let selected = self.selected == Some(ix);
                    Button::new(("el-query-conn", ix), name.clone())
                        .label_size(LabelSize::Small)
                        .toggle_state(selected)
                        .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                        .tooltip(ui::Tooltip::text(format!("{kind} connection")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(ix);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>(),
        );

        let status: Option<SharedString> = if self.running {
            Some("Running…".into())
        } else if let (Some(result), Some(elapsed)) = (&self.result, self.elapsed) {
            let count = result.rows.len();
            let capped = if count >= QUERY_ROW_CAP {
                format!(" (first {QUERY_ROW_CAP})")
            } else {
                String::new()
            };
            Some(format!("{count} rows{capped} in {:.2}s", elapsed.as_secs_f64()).into())
        } else {
            None
        };

        let toolbar = h_flex()
            .w_full()
            .p_1()
            .gap_2()
            .items_center()
            .child(chips)
            .child(div().flex_1())
            .children(status.map(|status| {
                Label::new(status).size(LabelSize::XSmall).color(Color::Muted)
            }))
            .child(
                Button::new("el-query-run", "Run query")
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Filled)
                    .disabled(self.running)
                    .on_click(cx.listener(|this, _, _, cx| this.run_query(cx))),
            );

        let editor = div()
            .w_full()
            .px_1()
            .pb_1()
            .child(
                div()
                    .w_full()
                    .p_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.editor_background)
                    .child(self.sql.clone()),
            );

        let body: gpui::AnyElement = if let Some(error) = &self.query_error {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(error.clone())
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .into_any_element()
        } else if let Some(result) = &self.result {
            render_grid(result, &self.result_scroll, "el-query-grid")
        } else if self.connections.is_empty() {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "No connections yet — run el: initialize workspace to create el/connections.yml.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            v_flex()
                .flex_1()
                .p_2()
                .child(
                    Label::new(
                        "Pick a connection and run a query — or click a table in the EL panel.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(toolbar)
            .child(editor)
            .child(body)
            .into_any_element()
    }
}

/// The shared results grid: fixed-width columns, accent header, uniform
/// rows. Used by mapping previews and query results alike.
fn render_grid(
    table: &PreviewTable,
    scroll: &UniformListScrollHandle,
    list_id: &'static str,
) -> gpui::AnyElement {
    const COL_WIDTH: f32 = 170.;
    let columns = table.columns.clone();
    let rows = table.rows.clone();
    let total = px(columns.len() as f32 * COL_WIDTH);
    let header = h_flex().w(total).flex_shrink_0().children(columns.iter().map(|column| {
        div().w(px(COL_WIDTH)).px_1().flex_shrink_0().child(
            Label::new(column.clone())
                .size(LabelSize::XSmall)
                .color(Color::Accent)
                .truncate(),
        )
    }));
    let count = rows.len();
    let list = gpui::uniform_list(list_id, count, {
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
                                Label::new(cell.clone()).size(LabelSize::XSmall).truncate(),
                            )
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        }
    })
    .flex_1()
    .track_scroll(scroll);

    div()
        .id(gpui::SharedString::from(format!("{list_id}-scroll")))
        .flex_1()
        .min_h_0()
        .overflow_x_scroll()
        .child(v_flex().w(total).h_full().child(header).child(list))
        .into_any_element()
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
        Some("EL Console")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleElRunsFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |this, _, cx| this.refresh_connections(cx));
        }
    }
}

impl Render for ElRunsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let body: gpui::AnyElement = match &self.preview {
            Some(preview) => {
                let count = preview.rows.len();
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
                                    .tooltip(ui::Tooltip::text("Back"))
                                    .on_click(cx.listener(|this, _, _, cx| this.show_runs(cx))),
                            )
                            .child(Label::new(preview.title.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(format!("{count} rows"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(render_grid(preview, &self.preview_scroll, "el-preview-rows"))
                    .into_any_element()
            }
            None => {
                let tab = |id: &'static str,
                           label: &'static str,
                           surface: Surface,
                           this: &Self,
                           cx: &mut Context<Self>| {
                    Button::new(id, label)
                        .label_size(LabelSize::Small)
                        .toggle_state(this.surface == surface)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.surface = surface;
                            if surface == Surface::Query {
                                this.refresh_connections(cx);
                            }
                            cx.notify();
                        }))
                };
                let header = h_flex()
                    .w_full()
                    .p_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(tab("el-console-runs", "Runs", Surface::Runs, self, cx))
                    .child(tab("el-console-query", "Query", Surface::Query, self, cx));
                let content: gpui::AnyElement = match self.surface {
                    Surface::Runs => self.run_view.clone().into_any_element(),
                    Surface::Query => self.render_query(cx),
                };
                v_flex()
                    .size_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(content))
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
