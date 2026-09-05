//! Spec writes go through the buffer system, never `std::fs::write`: a
//! canvas edit becomes an ordinary buffer transaction, so cmd+Z in the
//! YAML editor undoes it, and a dirty (unsaved) buffer is refused rather
//! than clobbered.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use gpui::{AsyncWindowContext, Entity, WeakEntity};
use project::Project;
use workspace::Workspace;

use el_engine::spec::Pipeline;

/// Serializes `pipeline` canonically and writes it into `spec_path` via
/// the project's buffer. Fails with a human message when the buffer holds
/// unsaved edits.
pub async fn write_spec(
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    spec_path: PathBuf,
    pipeline: Pipeline,
    cx: &mut AsyncWindowContext,
) -> Result<()> {
    let text = el_engine::spec::to_canonical_yaml(&pipeline);
    write_text(workspace, project, spec_path, text, cx).await
}

/// Pre-flight for multi-file writes: refuses before ANYTHING is written
/// when any target buffer holds unsaved edits, so a rename can't strand a
/// half-updated spec set.
pub async fn check_clean(
    project: Entity<Project>,
    paths: &[PathBuf],
    cx: &mut AsyncWindowContext,
) -> Result<()> {
    for path in paths {
        let Some(project_path) = project.update(cx, |project, cx| {
            project.project_path_for_absolute_path(path, cx)
        }) else {
            continue; // outside the project — the write itself will say so
        };
        let open = project.update(cx, |project, cx| project.open_buffer(project_path, cx));
        let buffer = open.await.context("opening spec buffer")?;
        if buffer.update(cx, |buffer, _| buffer.is_dirty()) {
            anyhow::bail!(
                "{} has unsaved edits — save or revert it first, then apply again",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("a spec file")
            );
        }
    }
    Ok(())
}

/// Same buffer-routed write for any canonical spec text (connections.yml).
pub async fn write_text(
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    spec_path: PathBuf,
    text: String,
    cx: &mut AsyncWindowContext,
) -> Result<()> {

    let project_path = project
        .update(cx, |project, cx| {
            project.project_path_for_absolute_path(&spec_path, cx)
        })
        .ok_or_else(|| anyhow!("{} is outside the project", spec_path.display()))?;

    let open = project.update(cx, |project, cx| project.open_buffer(project_path, cx));
    let buffer = open.await.context("opening spec buffer")?;

    let dirty = buffer.update(cx, |buffer, _| buffer.is_dirty());
    if dirty {
        anyhow::bail!(
            "{} has unsaved edits — save or revert it first, then apply again",
            spec_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("the spec file")
        );
    }

    buffer.update(cx, |buffer, cx| {
        let len = buffer.len();
        buffer.edit([(0..len, text)], None, cx);
    });

    let save = project.update(cx, |project, cx| project.save_buffer(buffer, cx));
    save.await.context("saving spec buffer")?;

    let _ = workspace;
    Ok(())
}
