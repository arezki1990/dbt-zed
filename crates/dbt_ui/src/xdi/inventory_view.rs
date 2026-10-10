//! The inventory of an xDI project, as a tab: what it holds counted, the
//! tables read and written most, what each process runs, the mappings no
//! process runs, the copies, the mappings' complexity and what could not be
//! read. Every row opens its lineage; the whole copies as Markdown.

use std::collections::HashSet;

use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle,
    SharedString, Subscription, WeakEntity, Window, px,
};
use ui::{Tooltip, prelude::*};
use workspace::{
    Workspace,
    item::{Item, TabContentParams},
};

use super::Focus;
use super::store::{Snapshot, XdiStore};

/// What the complexity list is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortBy {
    Mapping,
    Score,
    Sources,
    Stages,
    Joins,
    Filters,
    Columns,
    Transforms,
    Tokens,
}

impl SortBy {
    const ALL: [SortBy; 9] = [
        SortBy::Mapping,
        SortBy::Score,
        SortBy::Sources,
        SortBy::Stages,
        SortBy::Joins,
        SortBy::Filters,
        SortBy::Columns,
        SortBy::Transforms,
        SortBy::Tokens,
    ];

    fn label(self) -> &'static str {
        match self {
            SortBy::Mapping => "Mapping",
            SortBy::Score => "Score",
            SortBy::Sources => "Sources",
            SortBy::Stages => "Stages",
            SortBy::Joins => "Joins",
            SortBy::Filters => "Filters",
            SortBy::Columns => "Columns",
            SortBy::Transforms => "Transforms",
            SortBy::Tokens => "Tokens",
        }
    }

    fn value(self, job: &el_import::inventory::Complexity) -> usize {
        match self {
            SortBy::Mapping => 0,
            SortBy::Score => job.score,
            SortBy::Sources => job.sources,
            SortBy::Stages => job.stages,
            SortBy::Joins => job.joins,
            SortBy::Filters => job.filters,
            SortBy::Columns => job.columns,
            SortBy::Transforms => job.transforms,
            SortBy::Tokens => job.tokens,
        }
    }
}

/// So many rows a list shows until asked for all.
const SHOWN: usize = 50;

pub struct XdiInventoryView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    store: Entity<XdiStore>,
    sort: (SortBy, bool),
    /// The lists shown whole.
    whole: HashSet<&'static str>,
    scroll: ScrollHandle,
    _observe: Subscription,
}

impl XdiInventoryView {
    /// Opens the inventory tab, or brings it forward.
    pub fn deploy(
        workspace: &mut Workspace,
        store: Entity<XdiStore>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let existing = workspace.items_of_type::<XdiInventoryView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let view = cx.new(|cx| Self::new(workspace_handle, store, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        store: Entity<XdiStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            store,
            sort: (SortBy::Score, true),
            whole: HashSet::new(),
            scroll: ScrollHandle::new(),
            _observe: observe,
        }
    }

    fn show_lineage(&mut self, focus: Focus, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| {
                super::show_lineage(workspace, focus, window, cx)
            })
            .ok();
    }

    fn project_name(&self, cx: &App) -> String {
        self.store
            .read(cx)
            .active()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn card(label: &str, value: String, cx: &App) -> gpui::AnyElement {
        v_flex()
            .min_w(px(120.))
            .px_3()
            .py_2()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                Label::new(value)
                    .size(LabelSize::Large)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(label.to_owned())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .into_any_element()
    }

    fn section_title(title: &str, count: usize) -> gpui::AnyElement {
        h_flex()
            .pt_4()
            .pb_1()
            .gap_2()
            .child(
                Label::new(title.to_owned())
                    .size(LabelSize::Default)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(count.to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .into_any_element()
    }

    /// A row of cells; clickable when `focus` is given.
    fn row(
        &self,
        id: SharedString,
        cells: Vec<(String, f32)>,
        focus: Option<Focus>,
        muted: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut row = h_flex()
            .id(id)
            .w_full()
            .h(px(22.))
            .gap_2()
            .px_1()
            .rounded_sm()
            .hover(|style| style.bg(cx.theme().colors().element_hover));
        for (ix, (text, width)) in cells.into_iter().enumerate() {
            let cell = div().min_w_0().overflow_hidden().child(
                Label::new(text)
                    .size(LabelSize::Small)
                    .color(if muted || ix > 0 {
                        Color::Muted
                    } else {
                        Color::Default
                    })
                    .truncate(),
            );
            row = row.child(if width > 0. {
                cell.w(px(width)).flex_shrink_0()
            } else {
                cell.flex_1()
            });
        }
        if let Some(focus) = focus {
            row = row
                .cursor_pointer()
                .tooltip(Tooltip::text("Show the lineage"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_lineage(focus.clone(), window, cx)
                }));
        }
        row.into_any_element()
    }

    /// The first rows of a list, and a button for the rest.
    fn capped(
        &self,
        key: &'static str,
        rows: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let total = rows.len();
        if total <= SHOWN || self.whole.contains(key) {
            return rows;
        }
        let mut rows: Vec<gpui::AnyElement> = rows.into_iter().take(SHOWN).collect();
        rows.push(
            Button::new(
                SharedString::from(format!("xdi-all-{key}")),
                format!("Show all {total}"),
            )
            .label_size(LabelSize::Small)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.whole.insert(key);
                cx.notify();
            }))
            .into_any_element(),
        );
        rows
    }

    fn render_inventory(
        &self,
        snapshot: &Snapshot,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let inventory = &snapshot.inventory;
        let totals = &inventory.totals;
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        let node_of = |dataset: &str| -> Option<String> {
            snapshot
                .project
                .datasets
                .get(dataset)
                .map(el_import::lineage::dataset_node_id)
        };

        out.push(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(Self::card("mappings", super::grouped(totals.mappings), cx))
                .child(Self::card(
                    "processes",
                    super::grouped(totals.processes),
                    cx,
                ))
                .child(Self::card(
                    "named like copies",
                    totals.copies.to_string(),
                    cx,
                ))
                .child(Self::card(
                    "datasets without metadata",
                    format!(
                        "{} of {}",
                        super::grouped(totals.unresolved_datasets),
                        super::grouped(totals.datasets)
                    ),
                    cx,
                ))
                .child(Self::card(
                    "datastores described",
                    format!(
                        "{} of {}",
                        super::grouped(totals.resolved),
                        super::grouped(totals.datastores)
                    ),
                    cx,
                ))
                .child(Self::card(
                    "expressions read as a guess",
                    format!(
                        "{} of {}",
                        super::grouped(totals.approximate),
                        super::grouped(totals.expressions)
                    ),
                    cx,
                ))
                .child(Self::card(
                    "errors · warnings",
                    format!("{} · {}", totals.errors, totals.warnings),
                    cx,
                ))
                .into_any_element(),
        );

        out.push(Self::section_title("Folders", inventory.folders.len()));
        for folder in &inventory.folders {
            out.push(self.row(
                format!("xdi-folder-{}", folder.folder).into(),
                vec![
                    (folder.folder.clone(), 0.),
                    (format!("{} mappings", folder.mappings), 110.),
                    (format!("{} processes", folder.processes), 110.),
                ],
                None,
                false,
                cx,
            ));
        }

        for (title, pairs) in [
            ("Technologies", &inventory.technologies),
            ("Templates", &inventory.templates),
        ] {
            out.push(Self::section_title(title, pairs.len()));
            for (name, count) in pairs {
                out.push(self.row(
                    format!("xdi-{title}-{name}").into(),
                    vec![(name.clone(), 0.), (count.to_string(), 60.)],
                    None,
                    false,
                    cx,
                ));
            }
        }

        for (title, uses) in [
            ("Most read", &inventory.most_read),
            ("Most written", &inventory.most_written),
        ] {
            out.push(Self::section_title(title, uses.len()));
            for table in uses {
                out.push(self.row(
                    format!("xdi-{title}-{}", table.dataset).into(),
                    vec![
                        (table.label.clone(), 0.),
                        (table.technology.clone(), 110.),
                        (table.server.clone().unwrap_or_default(), 160.),
                        (format!("{} mappings", table.mappings), 100.),
                    ],
                    node_of(&table.dataset).map(Focus::Node),
                    false,
                    cx,
                ));
            }
        }

        out.push(Self::section_title("Processes", inventory.processes.len()));
        let rows = inventory
            .processes
            .iter()
            .map(|calls| {
                let mut runs = calls.mappings.clone();
                runs.extend(calls.processes.iter().map(|process| format!("▸ {process}")));
                self.row(
                    format!("xdi-process-{}", calls.process.id).into(),
                    vec![
                        (calls.process.path.clone(), 0.),
                        (format!("{} steps", calls.steps), 70.),
                        (runs.join(", "), 360.),
                        (
                            if calls.unresolved > 0 {
                                format!("{} not found", calls.unresolved)
                            } else {
                                String::new()
                            },
                            90.,
                        ),
                    ],
                    Some(Focus::Process(calls.process.id.clone())),
                    false,
                    cx,
                )
            })
            .collect();
        out.extend(self.capped("processes", rows, cx));

        for (key, title, items) in [
            (
                "not-called",
                "Not run by a process of this project",
                &inventory.not_called,
            ),
            ("copies", "Copies", &inventory.copies),
        ] {
            out.push(Self::section_title(title, items.len()));
            let rows = items
                .iter()
                .map(|item| {
                    let focus = if item.id.starts_with("process:") {
                        Focus::Process(item.id.clone())
                    } else {
                        Focus::Node(item.id.clone())
                    };
                    self.row(
                        format!("xdi-{key}-{}", item.id).into(),
                        vec![(item.path.clone(), 0.)],
                        Some(focus),
                        key == "copies",
                        cx,
                    )
                })
                .collect();
            out.extend(self.capped(key, rows, cx));
        }

        out.push(Self::section_title(
            "Complexity",
            inventory.complexity.len(),
        ));
        let (sort, descending) = self.sort;
        let mut header = h_flex().w_full().gap_2().px_1();
        for column in SortBy::ALL {
            let mark = if column == sort {
                if descending { " ↓" } else { " ↑" }
            } else {
                ""
            };
            let button = Button::new(
                SharedString::from(format!("xdi-sort-{}", column.label())),
                format!("{}{mark}", column.label()),
            )
            .label_size(LabelSize::XSmall)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sort = if this.sort.0 == column {
                    (column, !this.sort.1)
                } else {
                    (column, column != SortBy::Mapping)
                };
                cx.notify();
            }));
            header = header.child(if column == SortBy::Mapping {
                div().flex_1().child(button)
            } else {
                div().w(px(72.)).flex_shrink_0().child(button)
            });
        }
        out.push(header.into_any_element());
        let mut jobs: Vec<&el_import::inventory::Complexity> =
            inventory.complexity.iter().collect();
        jobs.sort_by(|a, b| {
            let order = match sort {
                SortBy::Mapping => a.job.path.cmp(&b.job.path),
                other => other.value(a).cmp(&other.value(b)),
            };
            let order = if descending { order.reverse() } else { order };
            order.then(a.job.path.cmp(&b.job.path))
        });
        let rows = jobs
            .into_iter()
            .map(|job| {
                let mut cells = vec![(job.job.path.clone(), 0.)];
                cells.extend(
                    SortBy::ALL
                        .iter()
                        .skip(1)
                        .map(|column| (column.value(job).to_string(), 72.)),
                );
                self.row(
                    format!("xdi-complexity-{}", job.job.id).into(),
                    cells,
                    Some(Focus::Node(job.job.id.clone())),
                    false,
                    cx,
                )
            })
            .collect();
        out.extend(self.capped("complexity", rows, cx));

        out.push(Self::section_title("Problems", inventory.problems.len()));
        let rows = inventory
            .problems
            .iter()
            .enumerate()
            .map(|(ix, problem)| {
                let focus = problem
                    .path
                    .as_ref()
                    .filter(|path| path.ends_with(".map"))
                    .map(|path| Focus::Node(format!("job:xdi:{path}")));
                self.row(
                    format!("xdi-problem-{ix}").into(),
                    vec![
                        (problem.kind.label().to_owned(), 150.),
                        (problem.path.clone().unwrap_or_default(), 260.),
                        (problem.message.clone(), 0.),
                    ],
                    focus,
                    false,
                    cx,
                )
            })
            .collect();
        out.extend(self.capped("problems", rows, cx));
        out
    }
}

impl EventEmitter<()> for XdiInventoryView {}

impl Focusable for XdiInventoryView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for XdiInventoryView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        format!("Inventory: {}", self.project_name(cx)).into()
    }

    fn tab_content(
        &self,
        params: TabContentParams,
        _window: &Window,
        cx: &App,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_1()
            .child(Icon::new(IconName::ListTree).color(Color::Muted))
            .child(Label::new(self.tab_content_text(0, cx)).color(params.text_color()))
            .into_any_element()
    }
}

impl Render for XdiInventoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let snapshot = store.snapshot().cloned();
        let loading = store.loading();
        let name = self.project_name(cx);
        let mut body = v_flex()
            .id("xdi-inventory")
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("XdiInventoryView")
            .bg(cx.theme().colors().editor_background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_4()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Label::new(format!("{name} — inventory"))
                                .size(LabelSize::Large)
                                .weight(gpui::FontWeight::SEMIBOLD),
                        ),
                    )
                    .when(loading, |row| {
                        row.child(
                            Label::new("Reading…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(snapshot.clone(), |row, snapshot| {
                        row.child(
                            Button::new("xdi-copy-markdown", "Copy as Markdown")
                                .start_icon(Icon::new(IconName::Copy).size(IconSize::Small))
                                .label_size(LabelSize::Small)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        snapshot.inventory.to_markdown(),
                                    ));
                                }),
                        )
                    }),
            );
        match snapshot {
            Some(snapshot) => {
                let rows = self.render_inventory(&snapshot, cx);
                body = body.children(rows);
            }
            None => {
                body = body.child(
                    Label::new(if loading {
                        "Reading the project…"
                    } else {
                        "No xDI project read yet."
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                );
            }
        }
        body
    }
}

#[cfg(test)]
mod render_tests {
    use gpui::{TestAppContext, VisualTestContext, size};
    use project::Project;
    use workspace::MultiWorkspace;

    use super::*;
    use crate::xdi::store::fixture_snapshot;

    #[gpui::test]
    async fn the_inventory_draws_in_every_state(cx: &mut TestAppContext) {
        let app_state = cx.update(|cx| {
            let app_state = workspace::AppState::test(cx);
            editor::init(cx);
            app_state
        });
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let window =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window
            .read_with(cx, |multi, _| multi.workspace().clone())
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let store = cx.new(|cx| super::super::store::XdiStore::new(project.clone(), cx));
        let view = workspace.update_in(cx, |_, _, cx| {
            let workspace = cx.entity().downgrade();
            cx.new(|cx| XdiInventoryView::new(workspace, store.clone(), cx))
        });
        let draw = |label: &str, cx: &mut VisualTestContext| {
            cx.draw(gpui::Point::default(), size(px(1200.), px(900.)), |_, _| {
                view.clone().into_any_element()
            });
            cx.run_until_parked();
            log::info!("drew: {label}");
        };
        draw("nothing read", cx);
        let dir = tempfile::tempdir().unwrap();
        let (root, snapshot) = fixture_snapshot(dir.path());
        store.update(cx, |store, cx| store.set_snapshot(root, Some(snapshot), cx));
        draw("read", cx);
        view.update(cx, |view, cx| {
            view.sort = (SortBy::Mapping, false);
            view.whole.extend([
                "processes",
                "not-called",
                "copies",
                "complexity",
                "problems",
            ]);
            cx.notify();
        });
        draw("sorted by name, every list whole", cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.tab_content_text(0, cx)),
            "Inventory: Sales"
        );
    }
}
