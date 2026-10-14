//! The migration of an xDI project to Ellode, a flow at a time, as a tab.
//! The overview lists the flows in the order suggested — each after those
//! building the tables it reads — and where each is; a flow's page shows
//! its xDI steps beside what each becomes, migrates it, then lists what is
//! left to finish by hand, ticked off as it is done. Where the migration is
//! lives in the folder it goes into, `xdi-migration.yml`, which travels
//! with the files.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use el_import::migrate::{FileState, Flux, Kind, Place, Progress, StepBecomes};
use el_import::model::StepKind;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, PromptLevel, ScrollHandle,
    SharedString, Subscription, WeakEntity, Window, px, relative,
};
use ui::{Checkbox, Tooltip, prelude::*};
use workspace::{
    Workspace,
    item::{Item, TabContentParams},
};

use super::store::{Migration, MigrationOp, XdiStore};
use crate::el::dialog::notice;

/// So many rows a list shows until asked for all.
const SHOWN: usize = 50;

/// So many variables set one after another fold into a row.
const FOLDED_VARIABLES: usize = 3;

pub struct XdiMigrationView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    store: Entity<XdiStore>,
    /// The flux shown, by path; the overview when none.
    page: Option<String>,
    /// The folded parts of the overview that are open, and the lists shown
    /// whole.
    open: HashSet<&'static str>,
    /// The steps of the page whose notes are open, by place.
    notes: HashSet<usize>,
    /// The runs of variables of the page that are open, by their first
    /// step's place.
    runs: HashSet<usize>,
    scroll: ScrollHandle,
    _observe: Subscription,
}

impl XdiMigrationView {
    /// Opens the migration tab, or brings it forward, on the page of `flux`
    /// when given; and looks at the folder again.
    pub fn deploy(
        workspace: &mut Workspace,
        store: Entity<XdiStore>,
        flux: Option<String>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        store.update(cx, |store, cx| store.refresh_migration(cx));
        let existing = workspace.items_of_type::<XdiMigrationView>(cx).next();
        if let Some(existing) = existing {
            if flux.is_some() {
                existing.update(cx, |view, cx| view.show(flux, cx));
            }
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let view = cx.new(|cx| {
            let mut view = Self::new(workspace_handle, store, cx);
            view.page = flux;
            view
        });
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
            page: None,
            open: HashSet::new(),
            notes: HashSet::new(),
            runs: HashSet::new(),
            scroll: ScrollHandle::new(),
            _observe: observe,
        }
    }

    /// Shows the page of `flux`, or the overview: another flow's page
    /// without what the last migration said.
    fn show(&mut self, flux: Option<String>, cx: &mut Context<Self>) {
        if flux.is_some() && flux != self.page {
            self.store
                .update(cx, |store, cx| store.clear_last_result(cx));
        }
        self.page = flux;
        self.notes.clear();
        self.runs.clear();
        self.scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    fn project_name(&self, cx: &App) -> String {
        self.store
            .read(cx)
            .active()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn queue(&mut self, op: MigrationOp, cx: &mut Context<Self>) {
        self.store
            .update(cx, |store, cx| store.queue_migration(op, cx));
    }

    /// Asks first, then queues `op`.
    fn confirm(
        &mut self,
        message: String,
        detail: String,
        button: &'static str,
        op: MigrationOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            &[button, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.queue(op, cx)).ok();
            }
        })
        .detach();
    }

    fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.to_path_buf();
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(
                        path,
                        workspace::OpenOptions {
                            focus: Some(true),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                    .detach();
            })
            .ok();
    }

    fn title(text: String) -> gpui::AnyElement {
        Label::new(text)
            .size(LabelSize::Large)
            .weight(gpui::FontWeight::SEMIBOLD)
            .into_any_element()
    }

    fn section_title(title: &str, count: String) -> gpui::AnyElement {
        h_flex()
            .pt_4()
            .pb_1()
            .gap_2()
            .child(
                Label::new(title.to_owned())
                    .size(LabelSize::Default)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(Label::new(count).size(LabelSize::Small).color(Color::Muted))
            .into_any_element()
    }

    fn muted(text: impl Into<SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }

    /// What a flux's status reads as, and its color.
    fn status_text(flux: &Flux, migration: &Migration) -> (String, Color, IconName) {
        let status = migration.status(&flux.path);
        let progress = status.map_or(Progress::ToMigrate, |status| status.progress);
        let (mut text, color, icon) = match (progress, flux.kind) {
            (Progress::Done, _) => ("done".to_owned(), Color::Success, IconName::Check),
            (Progress::ToFinish { left }, Kind::Manual) => (
                format!("by hand · {left} to finish"),
                Color::Accent,
                IconName::CountdownTimer,
            ),
            (Progress::ToFinish { left }, _) => (
                format!("migrated · {left} to finish"),
                Color::Accent,
                IconName::CountdownTimer,
            ),
            (Progress::ToMigrate, Kind::NoFlow) => (
                "to migrate · too many steps for one flow".to_owned(),
                Color::Muted,
                IconName::Circle,
            ),
            (Progress::ToMigrate, _) => ("to migrate".to_owned(), Color::Muted, IconName::Circle),
        };
        if status.is_some_and(|status| !status.changed.is_empty()) {
            text.push_str(" · changed in xDI since");
            return (text, Color::Warning, IconName::Warning);
        }
        (text, color, icon)
    }

    fn flux_row(
        &self,
        flux: &Flux,
        migration: &Migration,
        detail: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (status, color, icon) = Self::status_text(flux, migration);
        let path = flux.path.clone();
        h_flex()
            .id(SharedString::from(format!("xdi-flux-{}", flux.path)))
            .w_full()
            .min_h(px(26.))
            .gap_2()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .tooltip(Tooltip::text(flux.path.clone()))
            .on_click(cx.listener(move |this, _, _, cx| this.show(Some(path.clone()), cx)))
            .child(Icon::new(icon).size(IconSize::Small).color(color))
            .child(
                div().w(px(220.)).flex_shrink_0().min_w_0().child(
                    Label::new(flux.name.clone())
                        .size(LabelSize::Small)
                        .truncate(),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Self::muted(detail.unwrap_or_else(|| flux.folder.clone())).truncate()),
            )
            .child(div().w(px(180.)).flex_shrink_0().min_w_0().child(
                Self::muted(flux.flow.clone().unwrap_or_else(|| "no flow".to_owned())).truncate(),
            ))
            .child(
                div().w(px(240.)).flex_shrink_0().min_w_0().child(
                    Label::new(status)
                        .size(LabelSize::Small)
                        .color(if color == Color::Success {
                            Color::Muted
                        } else {
                            color
                        })
                        .truncate(),
                ),
            )
            .into_any_element()
    }

    /// The first rows of a list, and a button for the rest.
    fn capped(
        &self,
        key: &'static str,
        rows: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let total = rows.len();
        if total <= SHOWN || self.open.contains(key) {
            return rows;
        }
        let mut rows: Vec<gpui::AnyElement> = rows.into_iter().take(SHOWN).collect();
        rows.push(
            Button::new(
                SharedString::from(format!("xdi-migration-all-{key}")),
                format!("Show all {total}"),
            )
            .label_size(LabelSize::Small)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open.insert(key);
                cx.notify();
            }))
            .into_any_element(),
        );
        rows
    }

    /// A part of the overview folded until opened.
    fn fold(
        &self,
        key: &'static str,
        title: String,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, bool) {
        let open = self.open.contains(key);
        let header = h_flex()
            .id(SharedString::from(format!("xdi-migration-fold-{key}")))
            .pt_4()
            .pb_1()
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.open.remove(key) {
                    this.open.insert(key);
                }
                cx.notify();
            }))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(IconSize::Small)
                .color(Color::Muted),
            )
            .child(
                Label::new(title)
                    .size(LabelSize::Default)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .into_any_element();
        (header, open)
    }

    /// The folder, the options, and what keeps the migration from writing.
    fn render_header(
        &self,
        migration: &Migration,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        out.push(
            h_flex()
                .gap_2()
                .child(Self::muted(format!("Into {}", migration.folder.display())))
                .child(
                    Button::new("xdi-migration-folder", "Change…")
                        .label_size(LabelSize::Small)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.store
                                .update(cx, |store, cx| store.pick_migration_folder(cx));
                        })),
                )
                .into_any_element(),
        );
        let options = &migration.plan.options;
        out.push(
            Self::muted(format!(
                "Warehouse {} · dbt target schema {} · raw landing schema {} — {}",
                options.warehouse.as_deref().unwrap_or("none"),
                options.dbt_schema,
                options.raw_schema,
                if migration.state.is_some() {
                    "kept from the first migration"
                } else {
                    "from the settings, kept from the first migration on"
                }
            ))
            .into_any_element(),
        );
        if let Some(state) = &migration.state {
            if !state.metadata.is_empty() {
                out.push(
                    Self::muted(format!("Metadata: {}", state.metadata.join(", ")))
                        .into_any_element(),
                );
            }
        }
        if let Some(result) = self.store.read(cx).last_result() {
            out.push(
                div()
                    .pt_2()
                    .child(match result {
                        Ok(summary) => notice(Severity::Success, summary.clone()),
                        Err(error) => notice(Severity::Error, error.clone()),
                    })
                    .into_any_element(),
            );
        }
        out.extend(self.blockers(migration, cx));
        out
    }

    /// What keeps the migration from writing, or from converting much, and
    /// the way out of each: on the overview and on a flow's page alike.
    fn blockers(&self, migration: &Migration, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        };
        if let Some(error) = &migration.error {
            out.push(
                div()
                    .pt_2()
                    .child(notice(Severity::Error, error.clone()))
                    .into_any_element(),
            );
        }
        if migration.one_shot {
            let mut row = h_flex().gap_2().pt_1();
            if let Some(suggested) = migration.suggested.clone() {
                row = row.child(
                    Button::new(
                        "xdi-migration-suggested",
                        format!("Migrate into {}", name(&suggested)),
                    )
                    .style(ButtonStyle::Filled)
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let folder = suggested.clone();
                        this.store
                            .update(cx, |store, cx| store.set_migration_folder(folder, cx));
                    })),
                );
            }
            row = row.child(
                Button::new("xdi-migration-choose", "Choose another folder…")
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.store
                            .update(cx, |store, cx| store.pick_migration_folder(cx));
                    })),
            );
            out.push(
                v_flex()
                    .pt_2()
                    .child(notice(
                        Severity::Warning,
                        format!(
                            "{} holds the project converted at once (CONVERSION.md): a \
                             migration goes into a folder of its own.",
                            name(&migration.folder)
                        ),
                    ))
                    .child(row)
                    .into_any_element(),
            );
        }
        let read_again = |id: &'static str, cx: &mut Context<Self>| {
            Button::new(id, "Read the project again")
                .style(ButtonStyle::Filled)
                .label_size(LabelSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.store.update(cx, |store, cx| store.reload(cx));
                }))
        };
        let choose_metadata = |id: &'static str, cx: &mut Context<Self>| {
            Button::new(id, "Choose metadata folders…")
                .label_size(LabelSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.store
                        .update(cx, |store, cx| store.pick_metadata_folders(cx));
                }))
        };
        let (described, named) = migration.described;
        if !migration.missing_metadata.is_empty() {
            let missing: Vec<String> = migration
                .missing_metadata
                .iter()
                .map(|path| name(path))
                .collect();
            out.push(
                v_flex()
                    .pt_2()
                    .child(notice(
                        Severity::Warning,
                        format!(
                            "The metadata folder {} is not there — a drive not mounted? The \
                             tables it describes are not, so the mappings reading them are \
                             not converted, and nothing is migrated until it is there: mount \
                             it, then read the project again.",
                            missing.join(", ")
                        ),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_1()
                            .child(read_again("xdi-migration-read-again", cx))
                            .child(choose_metadata("xdi-migration-metadata", cx)),
                    )
                    .into_any_element(),
            );
        } else if named > 0 && described * 2 < named {
            out.push(
                v_flex()
                    .pt_2()
                    .child(notice(
                        Severity::Warning,
                        format!(
                            "The metadata read describes {} of the {} datastores the mappings \
                             name: the mappings reading the others are not converted. If some \
                             metadata is elsewhere, choose its folders.",
                            super::grouped(described),
                            super::grouped(named)
                        ),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_1()
                            .child(choose_metadata("xdi-migration-metadata", cx))
                            .child(read_again("xdi-migration-read-again", cx)),
                    )
                    .into_any_element(),
            );
        }
        if migration.metadata_differs.is_some() {
            out.push(
                div()
                    .pt_2()
                    .child(notice(
                        Severity::Info,
                        "Reading the project again from the metadata folders this migration \
                         keeps to…",
                    ))
                    .into_any_element(),
            );
        }
        out
    }

    fn render_overview(
        &self,
        migration: &Arc<Migration>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut out = self.render_header(migration, busy, cx);
        let plan = &migration.plan;
        let (main, inside): (Vec<&Flux>, Vec<&Flux>) = plan
            .fluxes
            .iter()
            .partition(|flux| !matches!(flux.place, Place::Inside { .. }));
        let progress = |flux: &Flux| {
            migration
                .status(&flux.path)
                .map_or(Progress::ToMigrate, |status| status.progress)
        };
        let done = main
            .iter()
            .filter(|flux| progress(flux) == Progress::Done)
            .count();
        let left: usize = main
            .iter()
            .map(|flux| match progress(flux) {
                Progress::ToFinish { left } => left,
                _ => 0,
            })
            .sum();
        let remaining: Vec<String> = main
            .iter()
            .filter(|flux| progress(flux) == Progress::ToMigrate && flux.kind != Kind::Manual)
            .map(|flux| flux.path.clone())
            .collect();
        let fraction = if main.is_empty() {
            0.
        } else {
            done as f32 / main.len() as f32
        };
        let colors = cx.theme().colors();
        out.push(
            v_flex()
                .pt_4()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new(format!(
                                "{} of {} flows done",
                                super::grouped(done),
                                super::grouped(main.len())
                            ))
                            .size(LabelSize::Default)
                            .weight(gpui::FontWeight::SEMIBOLD),
                        )
                        .child(Self::muted(format!(
                            "{} things to finish in those migrated",
                            super::grouped(left)
                        ))),
                )
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(colors.element_background)
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_full()
                                .bg(colors.text_accent),
                        ),
                )
                .into_any_element(),
        );
        if !remaining.is_empty() {
            let count = remaining.len();
            out.push(
                h_flex()
                    .pt_2()
                    .child(
                        Button::new(
                            "xdi-migrate-all",
                            format!("Migrate all remaining ({})…", super::grouped(count)),
                        )
                        .label_size(LabelSize::Small)
                        .disabled(busy || !migration.writable())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.confirm(
                                    format!("Migrate the {count} flows left?"),
                                    "Their files are written into the folder at once, each as its \
                                 own page would write it; what is there already is left as it is."
                                        .to_owned(),
                                    "Migrate all",
                                    MigrationOp::Migrate {
                                        fluxes: remaining.clone(),
                                        rewrite: false,
                                    },
                                    window,
                                    cx,
                                )
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }

        // The flows, in the order suggested; the next one said.
        let next = main
            .iter()
            .find(|flux| progress(flux) == Progress::ToMigrate && flux.kind != Kind::Manual)
            .map(|flux| flux.path.clone());
        out.push(Self::section_title(
            "Flows, in the order suggested",
            super::grouped(main.len()),
        ));
        let name_of = |path: &str| {
            plan.flux(path)
                .map_or_else(|| path.to_owned(), |flux| flux.name.clone())
        };
        let rows = main
            .iter()
            .map(|flux| {
                let waiting: Vec<String> = flux
                    .before
                    .iter()
                    .filter(|before| {
                        migration
                            .status(before)
                            .is_some_and(|status| status.progress == Progress::ToMigrate)
                    })
                    .map(|before| name_of(before))
                    .collect();
                let mut detail = flux.folder.clone();
                if next.as_deref() == Some(flux.path.as_str()) {
                    detail = format!("next · {detail}");
                }
                if progress(flux) == Progress::ToMigrate && !waiting.is_empty() {
                    detail = format!("{detail} · reads what {} builds", waiting.join(", "));
                }
                self.flux_row(flux, migration, Some(detail), cx)
            })
            .collect();
        out.extend(self.capped("flows", rows, cx));

        if !inside.is_empty() {
            let (header, open) = self.fold(
                "inside",
                format!("Run inside other flows ({})", super::grouped(inside.len())),
                cx,
            );
            out.push(header);
            if open {
                out.push(
                    Self::muted(
                        "Their steps are migrated with the flows that run them; each can also \
                         become a flow of its own, for when xDI runs it on its own.",
                    )
                    .into_any_element(),
                );
                let rows = inside
                    .iter()
                    .map(|flux| {
                        let callers = match &flux.place {
                            Place::Inside { callers } => callers
                                .iter()
                                .map(|caller| name_of(caller))
                                .collect::<Vec<_>>()
                                .join(", "),
                            _ => String::new(),
                        };
                        self.flux_row(flux, migration, Some(format!("inside {callers}")), cx)
                    })
                    .collect();
                out.extend(self.capped("inside-rows", rows, cx));
            }
        }
        if !plan.copies.is_empty() {
            let (header, open) = self.fold(
                "copies",
                format!("Copies left out ({})", super::grouped(plan.copies.len())),
                cx,
            );
            out.push(header);
            if open {
                out.push(
                    Self::muted("Named like copies someone kept, and nothing runs them.")
                        .into_any_element(),
                );
                let rows = plan
                    .copies
                    .iter()
                    .map(|path| Self::muted(path.clone()).into_any_element())
                    .collect();
                out.extend(self.capped("copies-rows", rows, cx));
            }
        }
        let problems: Vec<&el_import::migrate::Problem> =
            migration.problems.iter().chain(&migration.engine).collect();
        if !problems.is_empty() {
            out.push(Self::section_title(
                "What the check finds in the folder",
                super::grouped(problems.len()),
            ));
            let rows = problems
                .into_iter()
                .map(|problem| self.problem_row(migration, problem, cx))
                .collect();
            out.extend(self.capped("problems", rows, cx));
        }
        out
    }

    fn problem_row(
        &self,
        migration: &Migration,
        problem: &el_import::migrate::Problem,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let path = migration.folder.join(&problem.path);
        h_flex()
            .id(SharedString::from(format!(
                "xdi-problem-{}-{}",
                problem.path, problem.text
            )))
            .w_full()
            .gap_2()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open_file(&path, window, cx)))
            .child(
                Icon::new(IconName::Warning)
                    .size(IconSize::Small)
                    .color(Color::Warning),
            )
            .child(
                div()
                    .w(px(300.))
                    .flex_shrink_0()
                    .min_w_0()
                    .child(Self::muted(problem.path.clone()).truncate()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(problem.text.clone()).size(LabelSize::Small)),
            )
            .into_any_element()
    }

    fn render_flux(
        &self,
        migration: &Arc<Migration>,
        flux: &Flux,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        out.push(
            h_flex()
                .child(
                    Button::new("xdi-migration-back", "All flows")
                        .start_icon(Icon::new(IconName::ArrowLeft).size(IconSize::Small))
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.show(None, cx))),
                )
                .into_any_element(),
        );
        let status = migration.status(&flux.path);
        let progress = status.map_or(Progress::ToMigrate, |status| status.progress);
        let migrated = migration
            .state
            .as_ref()
            .and_then(|state| state.flows.iter().find(|done| done.path == flux.path));
        let writable = migration.writable() && !busy;
        // Why the button is grey, on it.
        let blocked = migration
            .blocked()
            .or_else(|| busy.then(|| "Busy reading or writing: a moment…".to_owned()));
        let button = match (flux.kind, migrated) {
            (Kind::Manual, _) => None,
            (_, None) => Some(
                Button::new("xdi-migrate-flux", "Migrate this flow")
                    .style(ButtonStyle::Filled)
                    .label_size(LabelSize::Small)
                    .disabled(!writable)
                    .tooltip(Tooltip::text(blocked.clone().unwrap_or_else(|| {
                        "Write its flow, and what its mappings became, into the folder".to_owned()
                    })))
                    .on_click(cx.listener({
                        let path = flux.path.clone();
                        move |this, _, _, cx| {
                            this.queue(
                                MigrationOp::Migrate {
                                    fluxes: vec![path.clone()],
                                    rewrite: false,
                                },
                                cx,
                            )
                        }
                    })),
            ),
            (_, Some(_)) => Some(
                Button::new("xdi-write-again", "Write again…")
                    .label_size(LabelSize::Small)
                    .disabled(!writable)
                    .when_some(blocked.clone(), |button, why| {
                        button.tooltip(Tooltip::text(why))
                    })
                    .on_click(cx.listener({
                        let path = flux.path.clone();
                        let name = flux.name.clone();
                        move |this, _, window, cx| {
                            this.confirm(
                                format!("Write {name} again?"),
                                "Its files are written as the conversion makes them now, over \
                                 what was changed in them since, and those deleted come back."
                                    .to_owned(),
                                "Write again",
                                MigrationOp::Migrate {
                                    fluxes: vec![path.clone()],
                                    rewrite: true,
                                },
                                window,
                                cx,
                            )
                        }
                    })),
            ),
        };
        out.push(
            h_flex()
                .pt_2()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Self::title(flux.name.clone())),
                )
                .children(button)
                .into_any_element(),
        );
        let place = match &flux.place {
            Place::Process => "a process".to_owned(),
            Place::Mapping => "a mapping no process's flow runs".to_owned(),
            Place::Inside { callers } => format!(
                "a process {} run{}",
                callers.join(", "),
                if callers.len() == 1 { "s" } else { "" }
            ),
        };
        let flow = match (&flux.flow, flux.kind) {
            (Some(flow), _) => format!("the flow {flow}"),
            (None, Kind::NoFlow) => "no flow: too many steps for one".to_owned(),
            (None, _) => "no flow: nothing in it runs in one".to_owned(),
        };
        out.push(Self::muted(format!("{} · {place} · {flow}", flux.path)).into_any_element());
        let (status_text, color, _) = Self::status_text(flux, migration);
        let status_text = match migrated {
            Some(done) => format!("{status_text} · migrated on {}", done.migrated),
            None => status_text,
        };
        out.push(
            Label::new(status_text)
                .size(LabelSize::Small)
                .color(color)
                .into_any_element(),
        );
        // What its button does.
        if migrated.is_none() && flux.kind != Kind::Manual {
            let folder = migration
                .folder
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(
                Self::muted(format!(
                    "Migrate this flow writes into {folder} its flow, the dbt models and \
                     pipelines its converted mappings became, and the sources, connections and \
                     variables they need. Nothing in xDI changes."
                ))
                .into_any_element(),
            );
        }
        let mut notices: Vec<gpui::AnyElement> = Vec::new();
        match self.store.read(cx).last_result() {
            Some(Ok(summary)) => {
                notices.push(notice(Severity::Success, summary.clone()).into_any_element())
            }
            Some(Err(error)) => {
                notices.push(notice(Severity::Error, error.clone()).into_any_element())
            }
            None => {}
        }
        notices.extend(self.blockers(migration, cx));
        // Mappings it runs that are not converted: why it is no flow.
        let unconverted = flux
            .steps
            .iter()
            .filter(|step| {
                matches!(
                    step.becomes,
                    StepBecomes::NotConverted {
                        mapping: Some(_),
                        ..
                    }
                )
            })
            .count();
        match flux.kind {
            Kind::Manual if unconverted > 0 => notices.push(
                notice(
                    Severity::Warning,
                    if unconverted == 1 {
                        "The mapping it runs is not converted, so nothing in it runs in a \
                         flow: its step says why. Once it is — its metadata found, say — it \
                         becomes a flow."
                            .to_owned()
                    } else {
                        format!(
                            "None of the {} mappings it runs is converted, so nothing in it \
                             runs in a flow: each step says why. Once they are — their \
                             metadata found, say — it becomes a flow.",
                            super::grouped(unconverted)
                        )
                    },
                )
                .into_any_element(),
            ),
            Kind::Manual => notices.push(
                notice(
                    Severity::Info,
                    "Nothing in it runs in a flow — SQL, scripts, files…: do what its steps \
                     did by hand, as listed under To finish.",
                )
                .into_any_element(),
            ),
            Kind::NoFlow => notices.push(
                notice(
                    Severity::Warning,
                    "It would run more steps than a flow takes: its pipelines and models are \
                     written, and it is split into flows of a few by hand.",
                )
                .into_any_element(),
            ),
            Kind::Flow => {}
        }
        if flux.kind != Kind::Manual && progress != Progress::Done {
            notices.push(
                notice(
                    Severity::Warning,
                    "Switch it off in xDI before its flow runs: both would write the same \
                     tables.",
                )
                .into_any_element(),
            );
        }
        let waiting: Vec<String> = flux
            .before
            .iter()
            .filter(|before| {
                migration
                    .status(before)
                    .is_some_and(|status| status.progress == Progress::ToMigrate)
            })
            .map(|before| {
                migration
                    .plan
                    .flux(before)
                    .map_or_else(|| before.clone(), |flux| flux.name.clone())
            })
            .collect();
        if migrated.is_none() && !waiting.is_empty() {
            notices.push(
                notice(
                    Severity::Info,
                    format!(
                        "Suggested after {}: its models read tables those build. Migrated \
                         first, they read them as xDI writes them.",
                        waiting.join(", ")
                    ),
                )
                .into_any_element(),
            );
        }
        if let Some(status) = status.filter(|status| !status.changed.is_empty()) {
            notices.push(
                notice(
                    Severity::Warning,
                    format!(
                        "Changed in xDI since it was migrated: {}. Write it again to take the \
                         changes.",
                        status.changed.join(", ")
                    ),
                )
                .into_any_element(),
            );
        }
        if !notices.is_empty() {
            out.push(v_flex().pt_2().gap_1().children(notices).into_any_element());
        }

        // Its steps, as xDI ran them, beside what each becomes.
        let mappings = flux
            .steps
            .iter()
            .filter(|step| match &step.becomes {
                StepBecomes::Copy { .. } | StepBecomes::Models { .. } => true,
                StepBecomes::NotConverted { mapping, .. } => mapping.is_some(),
                _ => false,
            })
            .count();
        let mut count = super::grouped(flux.steps.len());
        if unconverted > 0 && flux.kind != Kind::Manual {
            count.push_str(&format!(
                " · {} of its {} mappings not converted: left out of the flow, as listed under \
                 To finish",
                super::grouped(unconverted),
                super::grouped(mappings)
            ));
        }
        out.push(Self::section_title("Steps", count));
        // Variables set one after another fold into a row: the steps that
        // matter stand out.
        let variable = |step: &el_import::migrate::StepPlan| {
            step.kind == StepKind::Variable.label()
                && matches!(step.becomes, StepBecomes::NotRun { .. })
        };
        let mut ix = 0;
        while let Some(step) = flux.steps.get(ix) {
            let run = flux
                .steps
                .iter()
                .skip(ix)
                .take_while(|other| other.depth == step.depth && variable(other))
                .count();
            let shown = if run >= FOLDED_VARIABLES {
                out.push(self.variables_row(ix, run, step.depth, cx));
                if self.runs.contains(&ix) { run } else { 0 }
            } else {
                1
            };
            for at in ix..ix + shown {
                out.push(self.step_row(migration, flux, at, cx));
                if self.notes.contains(&at) {
                    let Some(step) = flux.steps.get(at) else {
                        continue;
                    };
                    for note in &step.notes {
                        out.push(
                            h_flex()
                                .pl(px(28. + step.depth as f32 * 16.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Self::muted(format!("· {note}"))),
                                )
                                .into_any_element(),
                        );
                    }
                }
            }
            ix += run.max(1);
        }

        // The files it writes, then those it needs.
        let state_text = |state: Option<FileState>| match state {
            Some(FileState::Written) => ("written", Color::Muted),
            Some(FileState::Changed) => ("changed since: kept", Color::Accent),
            Some(FileState::Gone) => ("deleted or moved: not written again", Color::Warning),
            Some(FileState::Theirs) => ("there before: kept", Color::Accent),
            Some(FileState::ToWrite) | None => ("to write", Color::Muted),
        };
        out.push(Self::section_title(
            "Files",
            super::grouped(flux.files.len() + flux.sources.len()),
        ));
        let files = flux.files.iter().map(|path| (path, "")).chain(
            flux.sources
                .iter()
                .map(|path| (path, "a source it reads · ")),
        );
        let rows: Vec<gpui::AnyElement> = files
            .map(|(path, what)| {
                let state = migration.files.get(path).copied();
                let (text, color) = state_text(state);
                let there = !matches!(state, Some(FileState::ToWrite | FileState::Gone) | None);
                let abs = migration.folder.join(path);
                h_flex()
                    .id(SharedString::from(format!("xdi-file-{path}")))
                    .w_full()
                    .gap_2()
                    .px_1()
                    .rounded_sm()
                    .when(there, |row| {
                        row.cursor_pointer()
                            .hover(|style| style.bg(cx.theme().colors().element_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_file(&abs, window, cx)
                            }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(path.clone()).size(LabelSize::Small).truncate()),
                    )
                    .child(
                        div().w(px(260.)).flex_shrink_0().child(
                            Label::new(format!("{what}{text}"))
                                .size(LabelSize::Small)
                                .color(color)
                                .truncate(),
                        ),
                    )
                    .into_any_element()
            })
            .collect();
        out.extend(self.capped("files", rows, cx));

        // What the checks find in its files, once written.
        if migrated.is_some() {
            let ours: HashSet<&String> = flux.files.iter().chain(&flux.sources).collect();
            let problems: Vec<&el_import::migrate::Problem> = migration
                .problems
                .iter()
                .chain(&migration.engine)
                .filter(|problem| ours.contains(&problem.path))
                .collect();
            out.push(Self::section_title(
                "Checks",
                super::grouped(problems.len()),
            ));
            if problems.is_empty() {
                out.push(
                    notice(
                        Severity::Success,
                        "Its files read as Ellode and dbt expect them, and name nothing that \
                         is not there.",
                    )
                    .into_any_element(),
                );
            }
            for problem in problems {
                out.push(self.problem_row(migration, problem, cx));
            }
        }

        // What is left to do by hand.
        let done = migration.state.as_ref().map(|state| &state.done);
        let ticked = |key: &str| done.is_some_and(|done| done.contains(key));
        let left = flux.todo.iter().filter(|todo| !ticked(&todo.key)).count();
        let active = (migrated.is_some() || flux.kind == Kind::Manual) && writable;
        out.push(Self::section_title(
            "To finish",
            if flux.todo.is_empty() {
                "nothing".to_owned()
            } else {
                format!("{} of {} left", left, flux.todo.len())
            },
        ));
        if migrated.is_none() && flux.kind != Kind::Manual && !flux.todo.is_empty() {
            out.push(
                Self::muted("Once it is migrated, these are left to do by hand:")
                    .into_any_element(),
            );
        }
        for (ix, todo) in flux.todo.iter().enumerate() {
            let checked = ticked(&todo.key);
            let key = todo.key.clone();
            let store = self.store.clone();
            out.push(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .items_start()
                    .child(
                        div().pt(px(2.)).child(
                            Checkbox::new(("xdi-todo", ix), checked.into())
                                .disabled(!active)
                                .on_click(move |state, _, cx| {
                                    let op = MigrationOp::Tick {
                                        key: key.clone(),
                                        done: state.selected(),
                                    };
                                    store.update(cx, |store, cx| store.queue_migration(op, cx));
                                }),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(todo.text.clone()).size(LabelSize::Small).color(
                                if checked {
                                    Color::Muted
                                } else {
                                    Color::Default
                                },
                            ),
                        ),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// The row standing for `count` variables set one after another from
    /// step `ix`: opening on them.
    fn variables_row(
        &self,
        ix: usize,
        count: usize,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let open = self.runs.contains(&ix);
        h_flex()
            .id(("xdi-variables", ix))
            .w_full()
            .min_h(px(24.))
            .gap_2()
            .px_1()
            .pl(px(4. + depth as f32 * 16.))
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.runs.remove(&ix) {
                    this.runs.insert(ix);
                }
                cx.notify();
            }))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(IconSize::Small)
                .color(Color::Muted),
            )
            .child(div().w(px(200.)).flex_shrink_0().child(
                Label::new(format!("{} variables", super::grouped(count))).size(LabelSize::Small),
            ))
            .child(div().flex_1().min_w_0().child(Self::muted(
                "set in xDI: the models read them as dbt variables",
            )))
            .into_any_element()
    }

    fn step_row(
        &self,
        migration: &Migration,
        flux: &Flux,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(step) = flux.steps.get(ix) else {
            return div().into_any_element();
        };
        let (becomes, color) = match &step.becomes {
            StepBecomes::Copy { pipeline, .. } => (format!("pipeline {pipeline}"), Color::Default),
            StepBecomes::Models {
                land,
                models,
                export,
                ..
            } => {
                let mut parts = Vec::new();
                if !land.is_empty() {
                    parts.push(format!("lands with {}", land.join(", ")));
                }
                parts.push(format!("builds {}", models.join(", ")));
                if !export.is_empty() {
                    parts.push(format!("writes back with {}", export.join(", ")));
                }
                (parts.join(" · "), Color::Default)
            }
            StepBecomes::Inlined { .. } => ("its steps follow".to_owned(), Color::Muted),
            StepBecomes::NotRun { why } => (format!("not run: {why}"), Color::Muted),
            StepBecomes::NotConverted { why, .. } => {
                (format!("not converted: {why}"), Color::Warning)
            }
        };
        let mapping = match &step.becomes {
            StepBecomes::Copy { mapping, .. } | StepBecomes::Models { mapping, .. } => {
                Some(mapping.as_str())
            }
            _ => None,
        };
        let with = mapping
            .and_then(|mapping| migration.status(&flux.path)?.migrated_with.get(mapping))
            .map(|other| {
                migration
                    .plan
                    .flux(other)
                    .map_or_else(|| other.clone(), |other| other.name.clone())
            });
        let notes = step.notes.len();
        let open = self.notes.contains(&ix);
        h_flex()
            .id(("xdi-step", ix))
            .flex_shrink_0()
            .w_full()
            .min_h(px(24.))
            .gap_2()
            .px_1()
            .pl(px(4. + step.depth as f32 * 16.))
            .rounded_sm()
            .items_start()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .when(notes > 0, |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.notes.remove(&ix) {
                            this.notes.insert(ix);
                        }
                        cx.notify();
                    }))
            })
            .child(
                div().w(px(220.)).flex_shrink_0().min_w_0().child(
                    Label::new(step.name.clone())
                        .size(LabelSize::Small)
                        .weight(if matches!(step.becomes, StepBecomes::Inlined { .. }) {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .truncate(),
                ),
            )
            .child(
                div()
                    .w(px(90.))
                    .flex_shrink_0()
                    .child(Self::muted(step.kind.clone()).truncate()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(becomes).size(LabelSize::Small).color(color))
                    .when_some(step.when.clone(), |column, when| {
                        column.child(
                            Label::new(when)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .italic(),
                        )
                    })
                    .when_some(with, |column, with| {
                        column.child(
                            Label::new(format!("migrated already, with {with}"))
                                .size(LabelSize::XSmall)
                                .color(Color::Success),
                        )
                    }),
            )
            .when(notes > 0, |row| {
                row.child(
                    div().flex_shrink_0().child(
                        Label::new(format!(
                            "{notes} note{} {}",
                            if notes == 1 { "" } else { "s" },
                            if open { "▾" } else { "▸" }
                        ))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    ),
                )
            })
            .into_any_element()
    }
}

impl EventEmitter<()> for XdiMigrationView {}

impl Focusable for XdiMigrationView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for XdiMigrationView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        format!("Migration: {}", self.project_name(cx)).into()
    }

    fn tab_content(
        &self,
        params: TabContentParams,
        _window: &Window,
        cx: &App,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_1()
            .child(Icon::new(IconName::ArrowUpRight).color(Color::Muted))
            .child(Label::new(self.tab_content_text(0, cx)).color(params.text_color()))
            .into_any_element()
    }
}

impl Render for XdiMigrationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let migration = store.migration().cloned();
        let loading = store.loading();
        let busy = store.migrating() || loading;
        let name = self.project_name(cx);
        let mut body = v_flex()
            .id("xdi-migration")
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("XdiMigrationView")
            .bg(cx.theme().colors().editor_background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_4()
            .gap_0p5()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Self::title(format!("{name} — migration to Ellode"))),
                    )
                    .when(busy, |row| {
                        row.child(Self::muted(if loading {
                            "Reading…"
                        } else {
                            "Writing…"
                        }))
                    })
                    .child(
                        IconButton::new("xdi-migration-refresh", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Look at the folder again"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store
                                    .update(cx, |store, cx| store.refresh_migration(cx));
                            })),
                    ),
            );
        let Some(migration) = migration else {
            return body.child(Self::muted(if loading {
                "Reading the project…"
            } else {
                "No xDI project read yet."
            }));
        };
        let flux = self
            .page
            .as_ref()
            .and_then(|path| migration.plan.flux(path))
            .cloned();
        let rows = match flux {
            Some(flux) => self.render_flux(&migration, &flux, busy, cx),
            None => self.render_overview(&migration, busy, cx),
        };
        // In a column of their own that keeps their heights: rows of a
        // scrolling column would shrink, and overlap, to fit it.
        body = body.child(v_flex().w_full().flex_shrink_0().gap_0p5().children(rows));
        body
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use el_import::migrate::Progress;
    use el_import::model::{Step, StepKind};
    use gpui::{TestAppContext, VisualTestContext, size};
    use project::Project;
    use workspace::MultiWorkspace;

    use super::*;
    use crate::xdi::store::{Snapshot, fixture_snapshot};

    const RUN_ORDERS: &str = "Orders/Processes/RUN_ORDERS.proc";
    const TOTALS: &str = "Reporting/Mappings/BUILD_ORDER_TOTALS.map";
    const PUBLISH: &str = "Reporting/Mappings/PUBLISH_TOTALS.map";
    const EXPORT: &str = "Orders/Processes/EXPORT.proc";

    struct Setup {
        store: Entity<XdiStore>,
        view: Entity<XdiMigrationView>,
        cx: VisualTestContext,
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    async fn setup(cx: &mut TestAppContext, edit: impl FnOnce(&mut Snapshot)) -> Setup {
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
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let store = cx.new(|cx| XdiStore::new(project.clone(), cx));
        // Its look at the workspace, which holds no project, first.
        cx.run_until_parked();
        let view = workspace.update_in(&mut cx, |_, _, cx| {
            let workspace = cx.entity().downgrade();
            cx.new(|cx| XdiMigrationView::new(workspace, store.clone(), cx))
        });
        let dir = tempfile::tempdir().unwrap();
        let (root, mut snapshot) = fixture_snapshot(dir.path());
        edit(&mut snapshot);
        store.update(&mut cx, |store, cx| {
            store.set_snapshot(root.clone(), Some(snapshot), cx)
        });
        cx.run_until_parked();
        Setup {
            store,
            view,
            cx,
            _dir: dir,
            root,
        }
    }

    impl Setup {
        fn draw(&mut self, label: &str) {
            let view = self.view.clone();
            self.cx
                .draw(gpui::Point::default(), size(px(1200.), px(900.)), |_, _| {
                    view.into_any_element()
                });
            self.cx.run_until_parked();
            log::info!("drew: {label}");
        }

        fn show(&mut self, flux: Option<&str>) {
            let flux = flux.map(str::to_owned);
            self.view
                .update(&mut self.cx, |view, cx| view.show(flux, cx));
        }

        fn op(&mut self, op: MigrationOp) {
            self.store
                .update(&mut self.cx, |store, cx| store.queue_migration(op, cx));
            self.cx.run_until_parked();
        }

        fn migration(&mut self) -> Arc<Migration> {
            self.store
                .read_with(&self.cx, |store, _| store.migration().cloned())
                .unwrap()
        }

        fn progress(&mut self, path: &str) -> Progress {
            self.migration().status(path).unwrap().progress
        }
    }

    #[gpui::test]
    async fn a_project_migrates_a_flow_at_a_time_through_the_store(cx: &mut TestAppContext) {
        let mut setup = setup(cx, |_| {}).await;
        let migration = setup.migration();
        assert_eq!(migration.folder, setup.root.with_file_name("Sales_ellode"));
        assert!(migration.writable() && migration.state.is_none());
        let order: Vec<String> = migration
            .plan
            .fluxes
            .iter()
            .map(|flux| flux.path.clone())
            .collect();
        assert_eq!(order, [RUN_ORDERS, TOTALS, PUBLISH, EXPORT]);

        for (ix, flux) in order.iter().enumerate() {
            setup.op(MigrationOp::Migrate {
                fluxes: vec![flux.clone()],
                rewrite: false,
            });
            let migration = setup.migration();
            assert!(
                matches!(setup.store.read_with(&setup.cx, |store, _| store.last_result().cloned()), Some(Ok(summary)) if summary.starts_with("Migrated ")),
                "{flux}"
            );
            assert_eq!(migration.state.as_ref().unwrap().flows.len(), ix + 1);
            assert!(
                migration.problems.is_empty(),
                "{flux}: {:#?}",
                migration.problems
            );
            assert!(
                migration.engine.is_empty(),
                "{flux}: {:#?}",
                migration.engine
            );
            assert!(matches!(setup.progress(flux), Progress::ToFinish { .. }));
        }

        // Every to-do ticked, in turn: done.
        let keys: Vec<String> = setup
            .migration()
            .plan
            .flux(RUN_ORDERS)
            .unwrap()
            .todo
            .iter()
            .map(|todo| todo.key.clone())
            .collect();
        for key in keys {
            setup.op(MigrationOp::Tick { key, done: true });
        }
        assert_eq!(setup.progress(RUN_ORDERS), Progress::Done);
        let state = el_import::migrate::State::load(&setup.root.with_file_name("Sales_ellode"))
            .unwrap()
            .unwrap();
        assert_eq!(
            state.done.len(),
            setup.migration().plan.fluxes[0].todo.len()
        );
    }

    #[gpui::test]
    async fn the_migration_draws_in_every_state(cx: &mut TestAppContext) {
        let mut setup = setup(cx, |_| {}).await;
        setup.draw("nothing migrated");
        setup.view.update(&mut setup.cx, |view, cx| {
            view.open.extend(["inside", "copies"]);
            cx.notify();
        });
        setup.draw("everything open");
        setup.show(Some(RUN_ORDERS));
        setup.draw("a flow to migrate");
        setup.view.update(&mut setup.cx, |view, cx| {
            view.notes.extend(0..20);
            cx.notify();
        });
        setup.draw("its notes open");
        setup.op(MigrationOp::Migrate {
            fluxes: vec![RUN_ORDERS.to_owned()],
            rewrite: false,
        });
        setup.draw("migrated, things to finish");
        setup.show(Some(EXPORT));
        setup.draw("a flow inside another, its mapping migrated with it");
        setup.show(None);
        setup.draw("the overview, one migrated");

        // Changed in xDI since.
        let mapping = setup.root.join("Orders/Mappings/LOAD_ORDERS.map");
        let mut text = std::fs::read_to_string(&mapping).unwrap();
        text.push_str("\n<!-- edited -->\n");
        std::fs::write(&mapping, text).unwrap();
        setup
            .store
            .update(&mut setup.cx, |store, cx| store.refresh_migration(cx));
        setup.cx.run_until_parked();
        assert_eq!(
            setup.migration().status(RUN_ORDERS).unwrap().changed,
            ["Orders/Mappings/LOAD_ORDERS.map"]
        );
        setup.show(Some(RUN_ORDERS));
        setup.draw("changed in xDI since");

        // A folder converted at once: another is offered.
        let mut setup = setup_one_shot(cx).await;
        let migration = setup.migration();
        assert!(migration.one_shot && !migration.writable());
        assert_eq!(
            migration.blocked().as_deref(),
            Some(
                "Sales_ellode holds the project converted at once: choose the folder to \
                 migrate into first."
            )
        );
        assert_eq!(
            migration.suggested.as_deref(),
            Some(setup.root.with_file_name("Sales_ellode_2").as_path())
        );
        setup.draw("a folder converted at once");
        // Never into the xDI project.
        let inside = setup.root.join("Orders");
        setup.store.update(&mut setup.cx, |store, cx| {
            store.set_migration_folder(inside, cx)
        });
        setup.cx.run_until_parked();
        assert_eq!(setup.migration().folder, migration.folder);
        assert!(matches!(
            setup
                .store
                .read_with(&setup.cx, |store, _| store.last_result().cloned()),
            Some(Err(_))
        ));
        let suggested = migration.suggested.clone().unwrap();
        setup.store.update(&mut setup.cx, |store, cx| {
            store.set_migration_folder(suggested.clone(), cx)
        });
        setup.cx.run_until_parked();
        assert_eq!(setup.migration().folder, suggested);
        assert!(setup.migration().writable());
        assert_eq!(
            setup
                .view
                .read_with(&setup.cx, |view, cx| view.tab_content_text(0, cx)),
            "Migration: Sales"
        );
    }

    async fn setup_one_shot(cx: &mut TestAppContext) -> Setup {
        let mut setup = setup(cx, |_| {}).await;
        let folder = setup.root.with_file_name("Sales_ellode");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("CONVERSION.md"), "# converted\n").unwrap();
        setup
            .store
            .update(&mut setup.cx, |store, cx| store.refresh_migration(cx));
        setup.cx.run_until_parked();
        setup
    }

    #[gpui::test]
    async fn a_missing_metadata_folder_stops_the_migration_and_variables_fold(
        cx: &mut TestAppContext,
    ) {
        // _MTD on a drive not mounted; four variables set one after another
        // in RUN_ORDERS.
        let mut setup = setup(cx, |snapshot| {
            let project = &mut snapshot.project;
            project.metadata.push(el_import::model::MetadataRoot {
                path: PathBuf::from("/Volumes/Unmounted/_MTD"),
                status: el_import::model::RootStatus::Missing,
                files: 0,
            });
            let run_orders = project
                .processes
                .iter_mut()
                .find(|process| process.name == "RUN_ORDERS")
                .unwrap();
            for n in 0..4 {
                run_orders.steps.push(Step {
                    element_id: format!("v_{n}"),
                    name: format!("V_{n}"),
                    kind: StepKind::Variable,
                    enabled: true,
                    start: true,
                });
            }
        })
        .await;
        let migration = setup.migration();
        assert_eq!(
            migration.missing_metadata,
            [PathBuf::from("/Volumes/Unmounted/_MTD")]
        );
        assert!(!migration.writable());
        assert_eq!(
            migration.blocked().as_deref(),
            Some("The metadata folder _MTD is not there: mount it, then read the project again.")
        );
        setup.draw("a metadata folder not there");
        setup.show(Some(RUN_ORDERS));
        setup.draw("its flow, nothing writable");
        // Migrating anyway is refused by the buttons; the store keeps to it.
        let flux = migration.plan.flux(RUN_ORDERS).unwrap();
        let first = flux
            .steps
            .iter()
            .position(|step| step.name == "V_0")
            .unwrap();
        assert!(
            flux.steps[first..first + 4]
                .iter()
                .all(|step| step.kind == "variable")
        );
        setup.draw("its variables folded");
        setup.view.update(&mut setup.cx, |view, cx| {
            view.runs.insert(first);
            cx.notify();
        });
        setup.draw("its variables open");
    }

    #[gpui::test]
    async fn a_flow_done_by_hand_and_one_too_long_draw(cx: &mut TestAppContext) {
        // RUN_ORDERS with only its SQL step: nothing a flow runs. EXPORT
        // running SYNC_CUSTOMERS 120 times: too many steps for one flow.
        let mut setup = setup(cx, |snapshot| {
            let project = &mut snapshot.project;
            let run_orders = project
                .processes
                .iter_mut()
                .find(|process| process.name == "RUN_ORDERS")
                .unwrap();
            run_orders
                .steps
                .retain(|step| matches!(step.kind, StepKind::Sql { .. }));
            run_orders.links.clear();
            let export = project
                .processes
                .iter_mut()
                .find(|process| process.name == "EXPORT")
                .unwrap();
            let sync = export.steps[0].clone();
            for n in 0..120 {
                export.steps.push(Step {
                    element_id: format!("again_{n}"),
                    name: format!("SYNC_{n}"),
                    ..sync.clone()
                });
            }
        })
        .await;
        let migration = setup.migration();
        let kinds: Vec<(&str, Kind)> = migration
            .plan
            .fluxes
            .iter()
            .map(|flux| (flux.path.as_str(), flux.kind))
            .collect();
        assert!(kinds.contains(&(RUN_ORDERS, Kind::Manual)), "{kinds:?}");
        assert!(kinds.contains(&(EXPORT, Kind::NoFlow)), "{kinds:?}");
        setup.draw("overview");
        setup.show(Some(RUN_ORDERS));
        setup.draw("by hand");
        let keys: Vec<String> = migration
            .plan
            .flux(RUN_ORDERS)
            .unwrap()
            .todo
            .iter()
            .map(|todo| todo.key.clone())
            .collect();
        for key in keys {
            setup.op(MigrationOp::Tick { key, done: true });
        }
        assert_eq!(setup.progress(RUN_ORDERS), Progress::Done);
        setup.draw("by hand, done");
        setup.show(Some(EXPORT));
        setup.draw("too many steps");
        setup.op(MigrationOp::Migrate {
            fluxes: vec![EXPORT.to_owned()],
            rewrite: false,
        });
        assert!(
            setup
                .migration()
                .files
                .contains_key("el/pipelines/sync_customers.yml")
        );
        assert!(
            !setup
                .migration()
                .folder
                .join("el/flows/export.yml")
                .exists()
        );
        setup.draw("too many steps, migrated");
    }
}
