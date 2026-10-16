//! An xDI process imported into the dbt project open in the workspace, and
//! handed to the agent to finish. The developer picks an xDI project, then
//! one of its processes; the process is migrated into the dbt project,
//! shown step by step in the migration tab, and the agent gets a short
//! brief: what each step became and what is left to do. What the agent
//! needs beyond it, it reads when it gets there: a digest per mapping,
//! written beside the brief, and the dbt project's own guidance, which it
//! follows over the layout the conversion chose.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use el_import::migrate::{Flux, Kind, Place, StepBecomes};
use el_import::model::{Job, Project};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions,
    PromptLevel, Subscription, Task, WeakEntity, Window,
};
use picker::{Picker, PickerDelegate};
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::ResultExt as _;
use workspace::{ModalView, Workspace};

use super::XdiMigrationView;
use super::store::{Migration, XdiStore};

/// Where the briefs and digests go in the dbt project: one folder per
/// process imported.
pub const BRIEF_FOLDER: &str = ".xdi-migration";

/// Files that tell an agent how a project is built, at its root.
const GUIDANCE_FILES: &[&str] = &[
    "AGENTS.md",
    "agents.md",
    "AGENT.md",
    "agent.md",
    "CLAUDE.md",
    ".rules",
    ".cursorrules",
    "README.md",
];

/// Folders holding an agent's skills or prompts, at the project's root.
const GUIDANCE_FOLDERS: &[&str] = &[
    ".agents/skills",
    ".claude/skills",
    ".claude/commands",
    ".github/prompts",
    "skills",
    "prompts",
];

/// The `ImportFromXdi` action: asks for an xDI project, adds it to the
/// workspace, and lists its processes to import into the dbt project open.
pub fn import_from_xdi(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let Some(dbt_root) = crate::database_panel::discover_workspace_root(workspace, cx) else {
        let answer = window.prompt(
            PromptLevel::Info,
            "Open a dbt project first",
            Some("A process from xDI is imported into the dbt project open in this window."),
            &["OK"],
            cx,
        );
        cx.spawn(async move |_, _| answer.await.ok()).detach();
        return;
    };
    let Some(store) = super::store(workspace, cx) else {
        return;
    };
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Import from this xDI project".into()),
    });
    let project = workspace.project().clone();
    cx.spawn_in(window, async move |workspace, cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        let Some(chosen) = paths.into_iter().next() else {
            return;
        };
        let found = cx
            .background_spawn({
                let chosen = chosen.clone();
                async move { el_import::xdi::discover(&chosen) }
            })
            .await;
        let problem = match found.first() {
            None => Some(format!(
                "{} holds no xDI project: a folder whose .project names xDI's builder, with \
                 mappings or processes.",
                chosen.display()
            )),
            Some(xdi) if dbt_root.starts_with(xdi) || xdi.starts_with(&dbt_root) => Some(
                "The xDI project and the dbt project are folders of their own, neither inside \
                 the other."
                    .to_owned(),
            ),
            Some(_) => None,
        };
        if let Some(problem) = problem {
            let answer = workspace.update_in(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Nothing to import",
                    Some(&problem),
                    &["OK"],
                    cx,
                )
            });
            if let Ok(answer) = answer {
                answer.await.ok();
            }
            return;
        }
        let Some(xdi) = found.into_iter().next() else {
            return;
        };
        // The agent reads the project's files through the workspace.
        let added = project.update(cx, |project, cx| {
            project.find_or_create_worktree(&xdi, true, cx)
        });
        if added.await.log_err().is_none() {
            return;
        }
        workspace
            .update_in(cx, |workspace, window, cx| {
                store.update(cx, |store, cx| {
                    store.import(xdi.clone(), dbt_root.clone(), cx)
                });
                let handle = cx.entity().downgrade();
                workspace.toggle_modal(window, cx, |window, cx| {
                    ImportProcessModal::new(handle, store, xdi, dbt_root, window, cx)
                });
            })
            .ok();
    })
    .detach();
}

/// Lists the processes of the xDI project being imported, to pick one.
pub struct ImportProcessModal {
    picker: Entity<Picker<ProcessDelegate>>,
    _observe: Subscription,
}

impl ImportProcessModal {
    fn new(
        workspace: WeakEntity<Workspace>,
        store: Entity<XdiStore>,
        project: PathBuf,
        dbt_root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = ProcessDelegate {
            modal: cx.entity().downgrade(),
            workspace,
            store: store.clone(),
            project,
            dbt_root,
            candidates: Err(READING.to_owned()),
            matches: Vec::new(),
            selected: 0,
            query: String::new(),
        };
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        let observe = cx.observe_in(&store, window, |this, _, window, cx| {
            this.picker
                .update(cx, |picker, cx| picker.refresh(window, cx));
        });
        picker.update(cx, |picker, cx| picker.refresh(window, cx));
        Self {
            picker,
            _observe: observe,
        }
    }
}

impl Render for ImportProcessModal {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().w(rems(40.)).child(self.picker.clone())
    }
}

impl Focusable for ImportProcessModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for ImportProcessModal {}
impl ModalView for ImportProcessModal {}

const READING: &str = "Reading the xDI project…";

struct Candidate {
    path: String,
    name: String,
    folder: String,
    status: String,
    color: Color,
}

pub struct ProcessDelegate {
    modal: WeakEntity<ImportProcessModal>,
    workspace: WeakEntity<Workspace>,
    store: Entity<XdiStore>,
    project: PathBuf,
    dbt_root: PathBuf,
    /// The processes, or why there are none to pick yet.
    candidates: Result<Vec<Candidate>, String>,
    /// The candidates the query keeps, by index.
    matches: Vec<usize>,
    selected: usize,
    query: String,
}

impl ProcessDelegate {
    /// The processes of the project, once it is read and planned for the
    /// dbt project; a mapping no process runs is not offered.
    fn candidates(&self, cx: &App) -> Result<Vec<Candidate>, String> {
        let store = self.store.read(cx);
        if store.active() != Some(self.project.as_path()) || store.loading() {
            return Err(READING.to_owned());
        }
        let Some(migration) = store.migration() else {
            return Err(READING.to_owned());
        };
        if migration.snapshot.project.root != self.project || migration.folder != self.dbt_root {
            return Err(READING.to_owned());
        }
        if let Some(why) = migration.blocked() {
            return Err(why);
        }
        Ok(migration
            .plan
            .fluxes
            .iter()
            .filter(|flux| flux.place != Place::Mapping)
            .map(|flux| {
                let (status, color, _) = XdiMigrationView::status_text(flux, migration);
                Candidate {
                    path: flux.path.clone(),
                    name: flux.name.clone(),
                    folder: flux.folder.clone(),
                    status,
                    color,
                }
            })
            .collect())
    }

    fn filter(&mut self) {
        let query = self.query.to_lowercase();
        self.matches = match &self.candidates {
            Ok(candidates) => candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    query.is_empty()
                        || candidate.name.to_lowercase().contains(&query)
                        || candidate.folder.to_lowercase().contains(&query)
                })
                .map(|(index, _)| index)
                .collect(),
            Err(_) => Vec::new(),
        };
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}

impl PickerDelegate for ProcessDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "xdi process importer"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Import an xDI process into the dbt project…".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some(match &self.candidates {
            Err(why) => why.clone().into(),
            Ok(candidates) if candidates.is_empty() => "The project runs no process.".into(),
            Ok(_) => "No process by that name.".into(),
        })
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.query = query;
        self.candidates = self.candidates(cx);
        self.filter();
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Ok(candidates) = &self.candidates else {
            return;
        };
        let Some(candidate) = self
            .matches
            .get(self.selected)
            .and_then(|index| candidates.get(*index))
        else {
            return;
        };
        let path = candidate.path.clone();
        let store = self.store.clone();
        self.workspace
            .update(cx, |workspace, cx| {
                super::open_migration(workspace, store, Some(path.clone()), window, cx);
                let view = workspace.items_of_type::<XdiMigrationView>(cx).next();
                if let Some(view) = view {
                    view.update(cx, |view, cx| view.prepare_with_agent(path, window, cx));
                }
            })
            .ok();
        self.dismissed(window, cx);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.modal
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let candidates = self.candidates.as_ref().ok()?;
        let candidate = candidates.get(*self.matches.get(ix)?)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(Label::new(candidate.name.clone()))
                        .child(
                            Label::new(candidate.folder.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                )
                .end_slot(
                    Label::new(candidate.status.clone())
                        .size(LabelSize::Small)
                        .color(candidate.color),
                ),
        )
    }
}

/// What the dbt project says about how it is built, for the agent to
/// read first: its guidance files and skill folders, and the folders its
/// models are laid out in.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Guidance {
    /// Relative to the dbt project.
    pub files: Vec<String>,
    pub models: Vec<String>,
}

pub fn guidance(dbt_root: &Path) -> Guidance {
    let mut files: Vec<String> = GUIDANCE_FILES
        .iter()
        .chain(GUIDANCE_FOLDERS)
        .filter(|name| dbt_root.join(name).exists())
        .map(|name| (*name).to_owned())
        .collect();
    // On a case-insensitive disk `agents.md` is `AGENTS.md` again.
    let mut seen = BTreeSet::new();
    files.retain(|name| seen.insert(name.to_lowercase()));
    let mut models: Vec<String> = std::fs::read_dir(dbt_root.join("models"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Guidance { files, models }
}

/// The folder of `flux`'s brief in the dbt project, relative to it.
pub fn brief_folder(flux: &Flux) -> String {
    let name: String = flux
        .flow
        .as_deref()
        .unwrap_or(&flux.name)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{BRIEF_FOLDER}/{name}")
}

/// Writes a digest of each mapping `flux` runs into its brief folder in
/// `folder`, the dbt project: what the mapping reads, joins, filters and
/// writes, column by column. Returns them, relative to the dbt project.
pub fn write_digests(migration: &Migration, flux: &Flux) -> io::Result<Vec<String>> {
    let project = &migration.snapshot.project;
    let brief = brief_folder(flux);
    let out = migration.folder.join(&brief);
    std::fs::create_dir_all(&out)?;
    let mut written = Vec::new();
    for path in &flux.mappings {
        let Some(job) = project.jobs.iter().find(|job| &job.path == path) else {
            continue;
        };
        let file = format!("{}.md", job.name.replace(['/', '\\'], "_"));
        std::fs::write(out.join(&file), digest(project, job))?;
        written.push(format!("{brief}/{file}"));
    }
    Ok(written)
}

/// A mapping as it reads in a few lines: its targets column by column,
/// then its sources with the columns it uses, its stages, joins and
/// filters. Only what the mapping touches; the metadata has the rest.
pub fn digest(project: &Project, job: &Job) -> String {
    let relation = |dataset: Option<&String>| {
        dataset.and_then(|id| project.datasets.get(id)).map_or_else(
            || "(not in the metadata)".to_owned(),
            |dataset| {
                let server = dataset
                    .server
                    .as_deref()
                    .map(|server| format!(" on {server}"))
                    .unwrap_or_default();
                format!(
                    "{}{server} ({})",
                    dataset.relation(),
                    dataset.technology.label()
                )
            },
        )
    };
    let column_type = |dataset: Option<&String>, name: &str| {
        let column = dataset
            .and_then(|id| project.datasets.get(id))
            .and_then(|dataset| dataset.column(name))?;
        let data_type = column.data_type.clone()?;
        Some(match (column.size, column.scale) {
            (Some(size), Some(scale)) if scale > 0 => format!("{data_type}({size},{scale})"),
            (Some(size), _) => format!("{data_type}({size})"),
            _ => data_type,
        })
    };
    let mut text = format!("# {}\n\nxDI file: `{}`\n", job.name, job.path);
    for target in &job.targets {
        let _ = write!(text, "\n## Writes {}\n", relation(target.dataset.as_ref()));
        if !target.update_key.is_empty() {
            let _ = writeln!(text, "Update key: {}", target.update_key.join(", "));
        }
        for template in job
            .templates
            .iter()
            .filter(|template| template.target == target.alias)
        {
            let parameters: Vec<String> = template
                .parameters
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            let _ = writeln!(
                text,
                "Template: {}{}",
                template.template,
                if parameters.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", parameters.join(", "))
                }
            );
        }
        if !target.set_expression.is_empty() {
            let _ = writeln!(text, "Sets: {}", target.set_expression);
        }
        for column in job
            .columns
            .iter()
            .filter(|column| column.owner == target.alias && column.enabled)
        {
            let mut flags = Vec::new();
            if column.update_key {
                flags.push("key");
            }
            if column.aggregate {
                flags.push("aggregate");
            }
            if column.insert && !column.update {
                flags.push("insert only");
            }
            if column.update && !column.insert {
                flags.push("update only");
            }
            let _ = writeln!(
                text,
                "- {}{}{}{} = {}",
                column.column,
                column_type(target.dataset.as_ref(), &column.column)
                    .map(|data_type| format!(" {data_type}"))
                    .unwrap_or_default(),
                column
                    .set
                    .as_deref()
                    .map(|set| format!(" [{set}]"))
                    .unwrap_or_default(),
                if flags.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", flags.join(", "))
                },
                one_line(&column.expression.text),
            );
        }
    }
    for stage in &job.stages {
        let _ = write!(text, "\n## Stage {}\n", stage.alias);
        if !stage.set_expression.is_empty() {
            let _ = writeln!(text, "Sets: {}", stage.set_expression);
        }
        for column in job
            .columns
            .iter()
            .filter(|column| column.owner == stage.alias && column.enabled)
        {
            let _ = writeln!(
                text,
                "- {}{} = {}",
                column.column,
                column
                    .set
                    .as_deref()
                    .map(|set| format!(" [{set}]"))
                    .unwrap_or_default(),
                one_line(&column.expression.text)
            );
        }
    }
    if !job.sources.is_empty() {
        text.push_str("\n## Reads\n");
    }
    for source in &job.sources {
        let used: BTreeSet<&str> = job
            .expressions()
            .flat_map(|expression| &expression.reads)
            .filter(|(alias, _)| alias == &source.alias)
            .map(|(_, column)| column.as_str())
            .collect();
        let used: Vec<String> = used
            .into_iter()
            .map(|name| match column_type(source.dataset.as_ref(), name) {
                Some(data_type) => format!("{name} {data_type}"),
                None => name.to_owned(),
            })
            .collect();
        let _ = writeln!(
            text,
            "- {} = {}; uses {}",
            source.alias,
            relation(source.dataset.as_ref()),
            if used.is_empty() {
                "no column".to_owned()
            } else {
                used.join(", ")
            }
        );
    }
    let joins: Vec<_> = job.joins.iter().filter(|join| join.enabled).collect();
    if !joins.is_empty() {
        text.push_str("\n## Joins\n");
    }
    for join in joins {
        let _ = writeln!(
            text,
            "- {} {} {} on {}",
            join.left,
            join.kind.label(),
            join.right,
            one_line(&join.condition.text)
        );
    }
    let filters: Vec<_> = job.filters.iter().filter(|filter| filter.enabled).collect();
    if !filters.is_empty() {
        text.push_str("\n## Filters\n");
    }
    for filter in filters {
        let _ = writeln!(
            text,
            "- {}{}{}",
            filter
                .on
                .as_deref()
                .map(|on| format!("on {on}: "))
                .unwrap_or_default(),
            one_line(&filter.condition.text),
            if filter.aggregate {
                " (aggregate: HAVING)"
            } else {
                ""
            }
        );
    }
    if !job.variables.is_empty() {
        let _ = write!(text, "\n## Variables\n{}\n", job.variables.join(", "));
    }
    text
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The agent's brief for `flux`, migrated into the dbt project: short, so
/// that it reads the rest only when it gets there.
pub fn agent_prompt(
    migration: &Migration,
    flux: &Flux,
    guidance: &Guidance,
    digests: &[String],
) -> String {
    let project = &migration.snapshot.project;
    let done = migration
        .state
        .as_ref()
        .map(|state| state.done.clone())
        .unwrap_or_default();
    let mut text = format!(
        "Help me finish migrating the xDI process `{}` into this dbt project ({}). The IDE \
         already converted it; your job is the part it could not do, one to-do at a time.\n",
        flux.name,
        migration.folder.display(),
    );

    text.push_str("\n## Follow this project's own design\n");
    if guidance.files.is_empty() {
        text.push_str(
            "The project has no guidance file: follow the patterns of its existing models.\n",
        );
    } else {
        let _ = writeln!(
            text,
            "Read these first and follow them over anything below: {}.",
            guidance
                .files
                .iter()
                .map(|file| format!("`{file}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !guidance.models.is_empty() {
        let _ = writeln!(
            text,
            "Its models are laid out in `models/`: {}.",
            guidance.models.join(", ")
        );
    }
    text.push_str(
        "The conversion put each model in a folder named after the xDI schema it builds in. \
         Where that is not this project's layering, naming or materialization, move and rename \
         the models to fit it, keeping what they compute.\n",
    );

    text.push_str("\n## The process, step by step\n");
    for step in &flux.steps {
        let indent = "  ".repeat(step.depth);
        let becomes = match &step.becomes {
            StepBecomes::Copy { pipeline, .. } => format!("pipeline `{pipeline}`"),
            StepBecomes::Models {
                land,
                models,
                export,
                ..
            } => {
                let mut parts = Vec::new();
                if !land.is_empty() {
                    parts.push(format!("lands {}", land.join(", ")));
                }
                if !models.is_empty() {
                    parts.push(format!("models {}", models.join(", ")));
                }
                if !export.is_empty() {
                    parts.push(format!("writes back {}", export.join(", ")));
                }
                parts.join("; ")
            }
            StepBecomes::Inlined { .. } => "its steps follow".to_owned(),
            StepBecomes::NotRun { why } => format!("not run: {why}"),
            StepBecomes::NotConverted { why, .. } => format!("not converted: {why}"),
        };
        let _ = writeln!(
            text,
            "{indent}- {} ({}{}) → {becomes}",
            step.name,
            step.kind,
            step.when
                .as_deref()
                .map(|when| format!(", {when}"))
                .unwrap_or_default(),
        );
    }
    match flux.kind {
        Kind::Manual => text.push_str("No step is something a flow runs: it is all by hand.\n"),
        Kind::NoFlow => text.push_str(
            "It has more steps than one flow takes: its pipelines and models are written, the \
             flow is to split.\n",
        ),
        Kind::Flow => {}
    }
    if let Some(flow) = &flux.flow {
        let _ = writeln!(text, "Its flow is `el/flows/{flow}.yml`.");
    }

    let todo: Vec<_> = flux
        .todo
        .iter()
        .filter(|todo| !done.contains(&todo.key))
        .collect();
    let _ = write!(text, "\n## To do ({})\n", todo.len());
    for item in &todo {
        let _ = writeln!(text, "- [{}] {}", item.key, one_line(&item.text));
    }

    text.push_str("\n## Where to look, when you need it\n");
    if !digests.is_empty() {
        let _ = writeln!(
            text,
            "- A digest of each xDI mapping (what it reads, joins, filters and writes, column by \
             column): {}",
            digests
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = writeln!(
        text,
        "- The xDI files themselves, in `{}`, only when a digest is not enough (a SQL step's \
         code is only in its `.proc`): {}",
        project.root.display(),
        flux.xdi
            .iter()
            .map(|path| format!("`{path}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !flux.connections.is_empty() {
        let _ = writeln!(
            text,
            "- Connections it uses, in `el/connections.yml`: {}",
            flux.connections.join(", ")
        );
    }
    if !flux.vars.is_empty() {
        let _ = writeln!(text, "- dbt vars its models read: {}", flux.vars.join(", "));
    }

    text.push_str(
        "\n## How to work\n\
         - One to-do at a time, in the order above: read only the files it needs, change them, \
         then tell me in a line which to-do key is done so I tick it in the migration tab.\n\
         - Check each model against its mapping's digest: joins, filters, a GROUP BY for every \
         aggregate, and Oracle or SQL Server functions written for the warehouse.\n\
         - Never read `.env` or write a credential: connections keep their `${VAR}` \
         placeholders.\n\
         - Do not run, deploy or switch anything off in xDI: say what to run and I will.\n",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guidance_is_what_the_project_holds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# rules").unwrap();
        std::fs::create_dir_all(dir.path().join(".claude/skills/medallion")).unwrap();
        std::fs::create_dir_all(dir.path().join("models/bronze")).unwrap();
        std::fs::create_dir_all(dir.path().join("models/silver")).unwrap();
        std::fs::write(dir.path().join("models/schema.yml"), "").unwrap();
        let guidance = guidance(dir.path());
        assert_eq!(guidance.files, vec!["AGENTS.md", ".claude/skills"]);
        assert_eq!(guidance.models, vec!["bronze", "silver"]);
    }
}
