//! The workspace's xDI project, read in the background by `el_import` and
//! kept as a snapshot the panel and the tabs draw from. It is read again
//! when the project is found, on Refresh, and half a second after a
//! mapping, a process or a metadata file in the workspace changes; the
//! reader keeps what it parsed, so a read again parses only what changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use db::kvp::KeyValueStore;
use el_import::inventory::{self, Inventory};
use el_import::lineage::{self, Index};
use el_import::xdi;
use gpui::{AppContext as _, Context, Entity, PathPromptOptions, Subscription, Task};
use project::Project;
use util::ResultExt as _;

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

pub struct XdiStore {
    project: Entity<Project>,
    /// The xDI projects among the workspace's folders.
    projects: Vec<PathBuf>,
    /// The one shown.
    active: Option<PathBuf>,
    /// The metadata folders picked, by project; a project without picks
    /// reads the folders found beside it.
    picked: BTreeMap<PathBuf, Vec<PathBuf>>,
    snapshot: Option<Arc<Snapshot>>,
    loading: bool,
    /// Whether the workspace was looked at yet.
    detected: bool,
    reader: Option<xdi::Reader>,
    reload_pending: bool,
    _detect: Task<()>,
    _load: Task<()>,
    _debounce: Task<()>,
    _subscription: Subscription,
}

/// Where the metadata folders picked for a project are kept: in the IDE's
/// own database, never in the project.
fn picked_key(project: &Path) -> String {
    format!("xdi-metadata-folders:{}", project.display())
}

impl XdiStore {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> XdiStore {
        let subscription = cx.subscribe(&project, |this: &mut Self, _, event, cx| match event {
            project::Event::WorktreeAdded(_) | project::Event::WorktreeRemoved(_) => {
                this.detect(cx);
            }
            project::Event::WorktreeUpdatedEntries(_, entries) => {
                let touched = entries
                    .iter()
                    .any(|(path, _, _)| matches!(path.extension(), Some("map" | "proc" | "md")));
                if touched && this.active.is_some() {
                    this.schedule_reload(cx);
                }
            }
            _ => {}
        });
        let mut store = XdiStore {
            project,
            projects: Vec::new(),
            active: None,
            picked: BTreeMap::new(),
            snapshot: None,
            loading: false,
            detected: false,
            reader: None,
            reload_pending: false,
            _detect: Task::ready(()),
            _load: Task::ready(()),
            _debounce: Task::ready(()),
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
            (projects, picked)
        });
        self._detect = cx.spawn(async move |this, cx| {
            let (projects, picked) = found.await;
            this.update(cx, |this, cx| {
                this.detected = true;
                this.picked = picked;
                let keep = this
                    .active
                    .as_ref()
                    .is_some_and(|active| projects.contains(active));
                this.projects = projects;
                if !keep {
                    this.active = this.projects.first().cloned();
                    this.snapshot = None;
                    this.reader = None;
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
        let picked = self.picked.get(&project).cloned();
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
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }
}

/// The fixture `el_import` writes, read: for tests.
#[cfg(test)]
pub(crate) fn fixture_snapshot(dir: &Path) -> (PathBuf, Snapshot) {
    let fixture = el_import::fixtures::write(dir).unwrap();
    let mut reader = xdi::Reader::new(fixture.project.clone(), vec![fixture.metadata]);
    (fixture.project, Snapshot::read(&mut reader))
}
