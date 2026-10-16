//! The workspace's xDI project, read in the background by `el_import` and
//! kept as a snapshot the panel and the tabs draw from. It is read again
//! when the project is found, on Refresh, and half a second after one of
//! its mappings, processes or metadata files changes; the reader keeps what
//! it parsed, so a read again parses only what changed.
//!
//! The store also keeps the project's migration to Ellode: its plan, where
//! it is, what the check finds — worked out again after each read and each
//! migration, and when its `xdi-migration.yml` changes. Migrations and the
//! to-dos ticked are written one at a time, in the order asked.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use db::kvp::KeyValueStore;
use el_import::convert::Options;
use el_import::inventory::{self, Inventory};
use el_import::lineage::{self, Index};
use el_import::migrate::{self, Plan};
use el_import::xdi;
use gpui::{App, AppContext as _, Context, Entity, PathPromptOptions, Subscription, Task};
use project::Project;
use settings::Settings as _;
use util::ResultExt as _;

use crate::dbt_settings::DbtSettings;

/// A project as last read.
pub struct Snapshot {
    pub project: el_import::model::Project,
    pub index: Index,
    pub inventory: Inventory,
    /// The metadata folders it was read with.
    pub roots: Vec<PathBuf>,
}

impl Snapshot {
    pub fn read(reader: &mut xdi::Reader) -> Snapshot {
        let project = reader.read();
        let index = Index::new(lineage::graph(&project));
        let inventory = inventory::build(&project);
        Snapshot {
            project,
            index,
            inventory,
            roots: reader.metadata_roots().to_vec(),
        }
    }
}

/// A project's migration to Ellode, as last worked out: its plan, where it
/// is, and what the check finds in its folder.
pub struct Migration {
    /// The project as read when it was planned.
    pub snapshot: Arc<Snapshot>,
    /// The folder it goes into.
    pub folder: PathBuf,
    /// The options it is planned with: the migration's own once something
    /// is migrated or ticked, the settings' before.
    pub asked: Options,
    pub plan: Arc<Plan>,
    /// None until something is migrated or ticked there.
    pub state: Option<migrate::State>,
    /// Where each flux is, in the plan's order.
    pub statuses: Vec<migrate::FluxStatus>,
    /// What the check finds in the folder.
    pub problems: Vec<migrate::Problem>,
    /// What the engine's own readers refuse there: the connections, a
    /// pipeline, a flow.
    pub engine: Vec<migrate::Problem>,
    /// Where each file the fluxes write, or need, is.
    pub files: BTreeMap<String, migrate::FileState>,
    /// The folder holds a project converted at once: no migration goes
    /// there.
    pub one_shot: bool,
    /// A folder beside the project the migration can go into instead.
    pub suggested: Option<PathBuf>,
    /// Why the folder's state could not be read.
    pub error: Option<String>,
    /// The metadata folders the migration keeps to, when the project was
    /// read from others: read again from them first.
    pub metadata_differs: Option<Vec<PathBuf>>,
    /// The metadata folders the project was read from that are not there —
    /// a drive not mounted: their tables are not described, and their
    /// mappings not converted, until they are.
    pub missing_metadata: Vec<PathBuf>,
    /// How many of the datastores the mappings name the metadata describes,
    /// of how many.
    pub described: (usize, usize),
}

impl Migration {
    pub fn status(&self, path: &str) -> Option<&migrate::FluxStatus> {
        self.statuses.iter().find(|status| status.path == path)
    }

    /// Why nothing can be migrated now, in a few words; none when a flux
    /// can be.
    pub fn blocked(&self) -> Option<String> {
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        };
        if self.error.is_some() {
            Some(format!(
                "{} does not read: fix it first.",
                migrate::STATE_FILE
            ))
        } else if self.one_shot {
            Some(format!(
                "{} holds the project converted at once: choose the folder to migrate into \
                 first.",
                name(&self.folder)
            ))
        } else if !self.missing_metadata.is_empty() {
            let missing: Vec<String> = self
                .missing_metadata
                .iter()
                .map(|path| name(path))
                .collect();
            Some(format!(
                "The metadata folder {} is not there: mount it, then read the project again.",
                missing.join(", ")
            ))
        } else if self.metadata_differs.is_some() {
            Some("The project is read again from this migration's metadata folders…".to_owned())
        } else {
            None
        }
    }

    /// Whether a flux can be migrated now: the folder takes it, and the
    /// project was read whole, as the migration keeps it.
    pub fn writable(&self) -> bool {
        !self.one_shot
            && self.error.is_none()
            && self.metadata_differs.is_none()
            && self.missing_metadata.is_empty()
    }
}

/// Something written into a migration's folder, in its turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationOp {
    /// Migrates these fluxes, by path; `rewrite` writes their files again.
    Migrate { fluxes: Vec<String>, rewrite: bool },
    /// Ticks a to-do done, or not.
    Tick { key: String, done: bool },
}

pub struct XdiStore {
    project: Entity<Project>,
    /// The xDI projects among the workspace's folders.
    projects: Vec<PathBuf>,
    /// The one shown.
    active: Option<PathBuf>,
    /// The metadata folders picked, by project; a project without picks
    /// reads the folders found beside it.
    picked: BTreeMap<PathBuf, Vec<PathBuf>>,
    /// The metadata folders a project's migration keeps to, by project:
    /// read before those picked.
    pinned: BTreeMap<PathBuf, Vec<PathBuf>>,
    /// The folder each project migrates into, when chosen; else the one
    /// beside it.
    folders: BTreeMap<PathBuf, PathBuf>,
    snapshot: Option<Arc<Snapshot>>,
    loading: bool,
    /// Whether the workspace was looked at yet.
    detected: bool,
    reader: Option<xdi::Reader>,
    reload_pending: bool,
    migration: Option<Arc<Migration>>,
    /// What to write into the migration's folder, in order.
    queue: VecDeque<MigrationOp>,
    /// Whether a write is under way.
    migrating: bool,
    /// What the last migration did, or why it did not.
    last_result: Option<Result<String, String>>,
    /// The project an import asked to show, once the workspace's folders
    /// are looked at again: it was just added to them.
    importing: Option<PathBuf>,
    _detect: Task<()>,
    _load: Task<()>,
    _debounce: Task<()>,
    _migration: Task<()>,
    _write: Task<()>,
    _subscription: Subscription,
}

/// Where the metadata folders picked for a project are kept: in the IDE's
/// own database, never in the project.
fn picked_key(project: &Path) -> String {
    format!("xdi-metadata-folders:{}", project.display())
}

/// Where the folder a project migrates into is kept, when chosen.
fn folder_key(project: &Path) -> String {
    format!("xdi-migration-folder:{}", project.display())
}

/// The options of a first migration: the `dbt.xdi_*` settings.
pub fn options_from_settings(cx: &App) -> Options {
    let settings = DbtSettings::get_global(cx);
    Options {
        warehouse: settings.xdi_warehouse.clone(),
        dbt_schema: settings.xdi_dbt_schema.clone(),
        raw_schema: settings.xdi_raw_schema.clone(),
    }
}

/// Works out where the migration of `snapshot`'s project into `folder` is,
/// planned with the migration's options, else `defaults`; `planned` — a
/// plan of the same read, and the options it was asked with — is used again
/// when those are still the options.
pub fn read_migration(
    snapshot: Arc<Snapshot>,
    folder: PathBuf,
    defaults: Options,
    planned: Option<(Arc<Plan>, Options)>,
) -> Migration {
    let (state, error) = match migrate::State::load(&folder) {
        Ok(state) => (state, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let asked = state
        .as_ref()
        .map_or(defaults, |state| state.options.options());
    let plan = match planned {
        Some((plan, made_with)) if made_with == asked || plan.options == asked => plan,
        _ => Arc::new(migrate::plan(&snapshot.project, &asked)),
    };
    let empty = migrate::State::default();
    let statuses = migrate::status(&snapshot.project, &plan, state.as_ref().unwrap_or(&empty));
    let problems = migrate::check(&folder).unwrap_or_default();
    let engine = engine_problems(&folder);
    let mut paths: Vec<String> = plan
        .fluxes
        .iter()
        .flat_map(|flux| flux.files.iter().chain(&flux.sources))
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    let files = migrate::file_states(&folder, state.as_ref().unwrap_or(&empty), &paths)
        .into_iter()
        .collect();
    let one_shot = state.is_none() && migrate::converted_at_once(&folder);
    let suggested = one_shot.then(|| suggest_folder(&snapshot.project.root));
    let metadata_differs = state
        .as_ref()
        .filter(|state| !state.metadata.is_empty())
        .map(|state| state.metadata_roots(&folder))
        .filter(|wanted| !same_folders(wanted, &snapshot.roots));
    let missing_metadata = snapshot
        .project
        .metadata
        .iter()
        .filter(|root| root.status == el_import::model::RootStatus::Missing)
        .map(|root| root.path.clone())
        .collect();
    let stats = &snapshot.project.stats;
    let described = (
        stats.resolved + stats.path_resolved,
        stats.datastores + stats.path_datastores,
    );
    Migration {
        snapshot,
        folder,
        asked,
        plan,
        state,
        statuses,
        problems,
        engine,
        files,
        one_shot,
        suggested,
        error,
        metadata_differs,
        missing_metadata,
        described,
    }
}

/// What the engine's own readers refuse in a migration's `folder`: its
/// connections, each pipeline (against them), each flow.
pub fn engine_problems(folder: &Path) -> Vec<migrate::Problem> {
    use el_engine::spec::{
        SpecError, parse_connections_yaml, parse_flow_yaml, parse_pipeline_yaml, validate_flow,
        validate_shape,
    };
    let said = |error: SpecError| match error {
        SpecError::Parse { message, .. } => message,
        other => other.to_string(),
    };
    let mut problems = Vec::new();
    let connections = match std::fs::read_to_string(folder.join("el/connections.yml")) {
        Ok(text) => match parse_connections_yaml(&text) {
            Ok(connections) => Some(connections),
            Err(error) => {
                problems.push(migrate::Problem {
                    path: "el/connections.yml".to_owned(),
                    text: said(error),
                });
                None
            }
        },
        Err(_) => None,
    };
    for dir in ["el/pipelines", "el/flows"] {
        let Ok(entries) = std::fs::read_dir(folder.join(dir)) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let at = format!("{dir}/{name}");
            let found: Vec<String> = if dir == "el/flows" {
                match parse_flow_yaml(&text) {
                    Ok(flow) => validate_flow(&flow)
                        .into_iter()
                        .map(|issue| match issue.step {
                            Some(step) => format!("{step}: {}", issue.message),
                            None => issue.message,
                        })
                        .collect(),
                    Err(error) => vec![said(error)],
                }
            } else {
                match parse_pipeline_yaml(&text) {
                    Ok(pipeline) => connections
                        .as_ref()
                        .map(|connections| {
                            validate_shape(&pipeline, connections)
                                .into_iter()
                                .map(|issue| match issue.stream {
                                    Some(stream) => format!("{stream}: {}", issue.message),
                                    None => issue.message,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    Err(error) => vec![said(error)],
                }
            };
            problems.extend(found.into_iter().map(|text| migrate::Problem {
                path: at.clone(),
                text,
            }));
        }
    }
    problems
}

/// A folder beside `project` a migration can go into: `<name>_ellode`, or
/// `<name>_ellode_2`… — the first that is not there, is empty, or holds a
/// migration.
pub fn suggest_folder(project: &Path) -> PathBuf {
    let base = super::ellode_folder(project);
    let usable = |folder: &Path| {
        !folder.exists()
            || folder.join(migrate::STATE_FILE).is_file()
            || std::fs::read_dir(folder).is_ok_and(|mut entries| entries.next().is_none())
    };
    if usable(&base) {
        return base;
    }
    (2..100)
        .map(|n| PathBuf::from(format!("{}_{n}", base.display())))
        .find(|folder| usable(folder))
        .unwrap_or(base)
}

/// Whether `a` and `b` name the same folders, links followed when they are
/// there.
fn same_folders(a: &[PathBuf], b: &[PathBuf]) -> bool {
    let set = |folders: &[PathBuf]| -> std::collections::BTreeSet<PathBuf> {
        folders.iter().map(|folder| normalized(folder)).collect()
    };
    set(a) == set(b)
}

fn normalized(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

impl XdiStore {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> XdiStore {
        let subscription = cx.subscribe(&project, |this: &mut Self, _, event, cx| match event {
            project::Event::WorktreeAdded(_) | project::Event::WorktreeRemoved(_) => {
                this.detect(cx);
            }
            project::Event::WorktreeUpdatedEntries(worktree, entries) => {
                let Some(base) = this
                    .project
                    .read(cx)
                    .worktree_for_id(*worktree, cx)
                    .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
                else {
                    return;
                };
                let folder = this.migration_folder();
                let (mut xdi, mut state) = (false, false);
                for (path, _, _) in entries.iter() {
                    let abs = base.join(path.as_std_path());
                    if matches!(path.extension(), Some("map" | "proc" | "md")) && this.reads(&abs) {
                        xdi = true;
                    }
                    if path.file_name() == Some(migrate::STATE_FILE)
                        && folder
                            .as_ref()
                            .is_some_and(|folder| abs.starts_with(folder))
                    {
                        state = true;
                    }
                }
                if xdi && this.active.is_some() {
                    this.schedule_reload(cx);
                } else if state && !this.migrating {
                    this.refresh_migration(cx);
                }
            }
            _ => {}
        });
        let mut store = XdiStore {
            project,
            projects: Vec::new(),
            active: None,
            picked: BTreeMap::new(),
            pinned: BTreeMap::new(),
            folders: BTreeMap::new(),
            snapshot: None,
            loading: false,
            detected: false,
            reader: None,
            reload_pending: false,
            migration: None,
            queue: VecDeque::new(),
            migrating: false,
            last_result: None,
            importing: None,
            _detect: Task::ready(()),
            _load: Task::ready(()),
            _debounce: Task::ready(()),
            _migration: Task::ready(()),
            _write: Task::ready(()),
            _subscription: subscription,
        };
        store.detect(cx);
        store
    }

    pub fn projects(&self) -> &[PathBuf] {
        &self.projects
    }

    pub fn active(&self) -> Option<&Path> {
        self.active.as_deref()
    }

    pub fn snapshot(&self) -> Option<&Arc<Snapshot>> {
        self.snapshot.as_ref()
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn detected(&self) -> bool {
        self.detected
    }

    /// Whether the folders picked are read, rather than those found.
    pub fn has_picked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| self.picked.contains_key(active))
    }

    /// Whether the project is read from the metadata folders its migration
    /// keeps to.
    pub fn has_pinned(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| self.pinned.contains_key(active))
    }

    pub fn migration(&self) -> Option<&Arc<Migration>> {
        self.migration.as_ref()
    }

    /// Whether something is being written into the migration's folder.
    pub fn migrating(&self) -> bool {
        self.migrating
    }

    pub fn last_result(&self) -> Option<&Result<String, String>> {
        self.last_result.as_ref()
    }

    pub fn clear_last_result(&mut self, cx: &mut Context<Self>) {
        if self.last_result.take().is_some() {
            cx.notify();
        }
    }

    /// The folder the project shown migrates into.
    pub fn migration_folder(&self) -> Option<PathBuf> {
        let project = self.active.as_ref()?;
        Some(
            self.folders
                .get(project)
                .cloned()
                .unwrap_or_else(|| super::ellode_folder(project)),
        )
    }

    /// Whether `path` is a file of the project shown or of its metadata.
    fn reads(&self, path: &Path) -> bool {
        let Some(project) = &self.active else {
            return false;
        };
        path.starts_with(project)
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.roots.iter().any(|root| path.starts_with(root)))
    }

    /// Looks for xDI projects in the workspace's folders, in the
    /// background, and reads the one shown.
    pub fn detect(&mut self, cx: &mut Context<Self>) {
        let roots: Vec<PathBuf> = self
            .project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            .collect();
        let kvp = KeyValueStore::global(cx);
        let found = cx.background_spawn(async move {
            let projects = super::detect(&roots);
            let picked: BTreeMap<PathBuf, Vec<PathBuf>> = projects
                .iter()
                .filter_map(|project| {
                    let text = kvp.read_kvp(&picked_key(project)).log_err().flatten()?;
                    let folders: Vec<PathBuf> = serde_json::from_str(&text).log_err()?;
                    Some((project.clone(), folders))
                })
                .collect();
            let folders: BTreeMap<PathBuf, PathBuf> = projects
                .iter()
                .filter_map(|project| {
                    let folder = kvp.read_kvp(&folder_key(project)).log_err().flatten()?;
                    Some((project.clone(), PathBuf::from(folder)))
                })
                .collect();
            (projects, picked, folders)
        });
        self._detect = cx.spawn(async move |this, cx| {
            let (projects, picked, folders) = found.await;
            this.update(cx, |this, cx| {
                this.detected = true;
                this.picked = picked;
                // A folder chosen while they were looked at, not stored yet.
                let chosen = std::mem::replace(&mut this.folders, folders);
                for (project, folder) in chosen {
                    this.folders.entry(project).or_insert(folder);
                }
                let importing = this
                    .importing
                    .take_if(|importing| projects.contains(importing));
                let keep = importing.is_none()
                    && this
                        .active
                        .as_ref()
                        .is_some_and(|active| projects.contains(active));
                this.projects = projects;
                if let Some(importing) = importing {
                    this.active = Some(importing);
                    this.snapshot = None;
                    this.reader = None;
                    this.migration = None;
                    this.last_result = None;
                    this.reload(cx);
                } else if !keep {
                    this.active = this.projects.first().cloned();
                    this.snapshot = None;
                    this.reader = None;
                    this.migration = None;
                    if this.active.is_some() {
                        this.reload(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Shows another of the workspace's xDI projects.
    pub fn set_active(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        if self.active.as_ref() == Some(&project) {
            return;
        }
        self.active = Some(project);
        self.snapshot = None;
        self.reader = None;
        self.migration = None;
        self.last_result = None;
        self.reload(cx);
        cx.notify();
    }

    /// Reads the project shown again; asked during a read, it reads once
    /// more after.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.active.clone() else {
            return;
        };
        if self.loading {
            self.reload_pending = true;
            return;
        }
        let picked = self
            .pinned
            .get(&project)
            .or_else(|| self.picked.get(&project))
            .cloned();
        let reader = self
            .reader
            .take()
            .filter(|reader| reader.project() == project);
        self.loading = true;
        cx.notify();
        let read = cx.background_spawn(async move {
            let roots = picked.unwrap_or_else(|| xdi::default_metadata_roots(&project));
            let mut reader = reader.unwrap_or_else(|| xdi::Reader::new(project, Vec::new()));
            reader.set_metadata_roots(roots);
            let snapshot = Snapshot::read(&mut reader);
            (reader, snapshot)
        });
        self._load = cx.spawn(async move |this, cx| {
            let (reader, snapshot) = read.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                // Read for a project no longer shown: dropped.
                if this.active.as_deref() == Some(reader.project()) {
                    this.reader = Some(reader);
                    this.snapshot = Some(Arc::new(snapshot));
                    this.refresh_migration(cx);
                }
                cx.notify();
                if std::mem::take(&mut this.reload_pending) {
                    this.reload(cx);
                }
            })
            .ok();
        });
    }

    /// Reads again half a second after the last change.
    fn schedule_reload(&mut self, cx: &mut Context<Self>) {
        self._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| this.reload(cx)).ok();
        });
    }

    /// Asks for the folders the project's metadata is in, keeps them, and
    /// reads again.
    pub fn pick_metadata_folders(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.active.clone() else {
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Use these metadata folders".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(folders))) = prompt.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.set_metadata_folders(project, Some(folders), cx)
            })
            .ok();
        })
        .detach();
    }

    /// Reads `project`'s metadata from `folders`; none: from the folders
    /// found beside it.
    pub fn set_metadata_folders(
        &mut self,
        project: PathBuf,
        folders: Option<Vec<PathBuf>>,
        cx: &mut Context<Self>,
    ) {
        let kvp = KeyValueStore::global(cx);
        let key = picked_key(&project);
        match &folders {
            Some(folders) => {
                self.picked.insert(project.clone(), folders.clone());
                let value = serde_json::to_string(folders).unwrap_or_default();
                cx.background_spawn(async move { kvp.write_kvp(key, value).await.log_err() })
                    .detach();
            }
            None => {
                self.picked.remove(&project);
                cx.background_spawn(async move { kvp.delete_kvp(key).await.log_err() })
                    .detach();
            }
        }
        if self.active.as_ref() == Some(&project) {
            self.reload(cx);
        }
        cx.notify();
    }

    /// Works out again where the migration of the project shown is, in the
    /// background: after a read, a migration, a change of its state.
    pub fn refresh_migration(&mut self, cx: &mut Context<Self>) {
        // A write under way works it out again when done.
        if self.migrating {
            return;
        }
        let (Some(project), Some(snapshot), Some(folder)) = (
            self.active.clone(),
            self.snapshot.clone(),
            self.migration_folder(),
        ) else {
            self.migration = None;
            return;
        };
        let planned = self
            .migration
            .as_ref()
            .filter(|migration| {
                Arc::ptr_eq(&migration.snapshot, &snapshot) && migration.folder == folder
            })
            .map(|migration| (migration.plan.clone(), migration.asked.clone()));
        let defaults = options_from_settings(cx);
        let read = cx.background_spawn({
            let snapshot = snapshot.clone();
            let folder = folder.clone();
            async move { read_migration(snapshot, folder, defaults, planned) }
        });
        self._migration = cx.spawn(async move |this, cx| {
            let migration = read.await;
            this.update(cx, |this, cx| {
                let current = this.active.as_ref() == Some(&project)
                    && this.migration_folder().as_ref() == Some(&folder)
                    && this
                        .snapshot
                        .as_ref()
                        .is_some_and(|now| Arc::ptr_eq(now, &snapshot));
                if !current {
                    return;
                }
                // The migration keeps to other metadata folders: read again
                // from them.
                if let Some(roots) = migration.metadata_differs.clone() {
                    this.pinned.insert(project.clone(), roots);
                    this.reload(cx);
                }
                this.migration = Some(Arc::new(migration));
                cx.notify();
            })
            .ok();
        });
    }

    /// Migrates into `folder` from now on, kept for the project shown: never
    /// a folder of the xDI project or of its metadata, which are not written.
    pub fn set_migration_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.active.clone() else {
            return;
        };
        let roots = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.roots.clone())
            .unwrap_or_default();
        let folder_at = normalized(&folder);
        if std::iter::once(&project)
            .chain(&roots)
            .any(|root| folder_at.starts_with(normalized(root)))
        {
            self.last_result = Some(Err(
                "A migration goes into a folder of its own, outside the xDI project and its \
                 metadata folders."
                    .to_owned(),
            ));
            cx.notify();
            return;
        }
        let kvp = KeyValueStore::global(cx);
        let key = folder_key(&project);
        let value = folder.to_string_lossy().into_owned();
        cx.background_spawn(async move { kvp.write_kvp(key, value).await.log_err() })
            .detach();
        self.folders.insert(project.clone(), folder);
        self.last_result = None;
        // The folder's own metadata folders, if it has any, come with it.
        if self.pinned.remove(&project).is_some() {
            self.reload(cx);
        } else {
            self.refresh_migration(cx);
        }
        cx.notify();
    }

    /// Shows `project`, migrating into `folder`: a process imported into a
    /// dbt project. The project may not be among the workspace's yet,
    /// when it was just added to it.
    pub fn import(&mut self, project: PathBuf, folder: PathBuf, cx: &mut Context<Self>) {
        let kvp = KeyValueStore::global(cx);
        let key = folder_key(&project);
        let value = folder.to_string_lossy().into_owned();
        cx.background_spawn(async move { kvp.write_kvp(key, value).await.log_err() })
            .detach();
        let moved = self.folders.get(&project) != Some(&folder);
        self.folders.insert(project.clone(), folder);
        if moved {
            self.pinned.remove(&project);
        }
        if self.projects.contains(&project) {
            if self.active.as_ref() == Some(&project) {
                if moved {
                    self.last_result = None;
                    self.reload(cx);
                }
            } else {
                self.set_active(project, cx);
            }
        } else {
            self.importing = Some(project);
            self.detect(cx);
        }
        cx.notify();
    }

    /// Asks for the folder to migrate into.
    pub fn pick_migration_folder(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Migrate into this folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(folders))) = prompt.await else {
                return;
            };
            let Some(folder) = folders.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.set_migration_folder(folder, cx))
                .ok();
        })
        .detach();
    }

    /// Writes `op` into the migration's folder once those asked before are.
    pub fn queue_migration(&mut self, op: MigrationOp, cx: &mut Context<Self>) {
        self.queue.push_back(op);
        self.write_next(cx);
    }

    fn write_next(&mut self, cx: &mut Context<Self>) {
        if self.migrating {
            return;
        }
        let Some(op) = self.queue.pop_front() else {
            return;
        };
        let Some(migration) = self.migration.clone() else {
            self.queue.clear();
            return;
        };
        self.migrating = true;
        // What a look begun before would say is stale once written.
        self._migration = Task::ready(());
        cx.notify();
        let defaults = options_from_settings(cx);
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let write = cx.background_spawn({
            let op = op.clone();
            async move {
                let project = &migration.snapshot.project;
                let result = match &op {
                    MigrationOp::Migrate { fluxes, rewrite } => migrate::migrate(
                        project,
                        &migration.plan,
                        &migration.folder,
                        fluxes,
                        &today,
                        *rewrite,
                    )
                    .map(|migrated| migrated_summary(&migration.plan, fluxes, &migrated))
                    .map_err(|error| error.to_string()),
                    MigrationOp::Tick { key, done } => {
                        migrate::tick(project, &migration.plan, &migration.folder, key, *done)
                            .map(|_| String::new())
                            .map_err(|error| error.to_string())
                    }
                };
                let planned = Some((migration.plan.clone(), migration.asked.clone()));
                let next = read_migration(
                    migration.snapshot.clone(),
                    migration.folder.clone(),
                    defaults,
                    planned,
                );
                (result, next)
            }
        });
        self._write = cx.spawn(async move |this, cx| {
            let (result, next) = write.await;
            this.update(cx, |this, cx| {
                this.migrating = false;
                let current = this.migration.as_ref().is_some_and(|now| {
                    Arc::ptr_eq(&now.snapshot, &next.snapshot) && now.folder == next.folder
                });
                if current {
                    this.migration = Some(Arc::new(next));
                }
                if matches!(op, MigrationOp::Migrate { .. }) || result.is_err() {
                    this.last_result = Some(result);
                }
                cx.notify();
                if !this.queue.is_empty() {
                    this.write_next(cx);
                    return;
                }
                // The project was read again while writing: worked out again
                // for what was read.
                let stale = this.snapshot.as_ref().is_some_and(|now| {
                    this.migration
                        .as_ref()
                        .is_none_or(|migration| !Arc::ptr_eq(now, &migration.snapshot))
                });
                if stale {
                    this.refresh_migration(cx);
                }
            })
            .ok();
        });
    }

    /// Shows `snapshot` as `project`'s, read elsewhere: for tests.
    #[cfg(test)]
    pub(crate) fn set_snapshot(
        &mut self,
        project: PathBuf,
        snapshot: Option<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        self.detected = true;
        if !self.projects.contains(&project) {
            self.projects.push(project.clone());
        }
        self.active = Some(project);
        self.snapshot = snapshot.map(Arc::new);
        self.refresh_migration(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }
}

/// `Migrated RUN_ORDERS: 12 files written, 1 kept as it was. The check finds
/// nothing wrong.`
pub fn migrated_summary(plan: &Plan, fluxes: &[String], migrated: &migrate::Migrated) -> String {
    let names: Vec<&str> = fluxes
        .iter()
        .map(|path| {
            plan.flux(path)
                .map_or(path.as_str(), |flux| flux.name.as_str())
        })
        .collect();
    let named = match names.as_slice() {
        [one] => (*one).to_owned(),
        many => format!("{} flows", many.len()),
    };
    let plural = |count: usize, one: &str, many: &str| {
        format!(
            "{} {}",
            super::grouped(count),
            if count == 1 { one } else { many }
        )
    };
    let mut text = format!(
        "Migrated {named}: {} written",
        plural(migrated.written.len(), "file", "files")
    );
    if !migrated.kept.is_empty() {
        text.push_str(&format!(", {} kept as it was", migrated.kept.len()));
    }
    text.push_str(&match migrated.problems.len() {
        0 => ". The check finds nothing wrong.".to_owned(),
        count => format!(
            ". The check finds {}.",
            plural(count, "problem", "problems")
        ),
    });
    text
}

/// The fixture `el_import` writes, read: for tests.
#[cfg(test)]
pub(crate) fn fixture_snapshot(dir: &Path) -> (PathBuf, Snapshot) {
    let fixture = el_import::fixtures::write(dir).unwrap();
    let mut reader = xdi::Reader::new(fixture.project.clone(), vec![fixture.metadata]);
    (fixture.project, Snapshot::read(&mut reader))
}
