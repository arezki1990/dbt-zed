//! The EL Runs tab: live per-stream progress for a pipeline run — phase
//! chips, ticking row counts, cast-failure badges, cancel. Owned by the
//! results panel and rendered as its ElRuns view.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::{FutureExt as _, StreamExt as _};
use gpui::{Context, SharedString, Task, WeakEntity, Window};
use ui::{Tooltip, prelude::*};
use workspace::Workspace;

use el_engine::progress::{CancelFlag, Phase, ProgressEvent};
use el_engine::spec::Pipeline;

#[derive(Default)]
pub struct StreamRow {
    pub stream: SharedString,
    pub phase: Option<Phase>,
    pub rows_read: u64,
    pub rows_written: u64,
    pub cast_failures: u64,
    /// Which columns failed, with samples — arrives with StreamFinished.
    pub column_failures: Vec<el_engine::ColumnFailures>,
    pub error: Option<SharedString>,
    pub done: bool,
}

struct ActiveRun {
    pipeline: SharedString,
    /// What to re-run: the project, the spec's file (re-read fresh so a
    /// re-run picks up fixes), and the snapshot as a fallback.
    project_root: PathBuf,
    spec_path: Option<PathBuf>,
    spec: Arc<Pipeline>,
    started: Instant,
    streams: Vec<StreamRow>,
    cancel: CancelFlag,
    cancelling: bool,
    finished: Option<bool>,
    fatal: Option<SharedString>,
}

pub struct ElRunView {
    workspace: WeakEntity<Workspace>,
    run: Option<ActiveRun>,
    _run: Task<()>,
}

impl ElRunView {
    pub fn new(workspace: WeakEntity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            run: None,
            _run: Task::ready(()),
        }
    }

    pub fn is_running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| run.finished.is_none() && run.fatal.is_none())
    }

    /// The failing columns of `stream` in the current run of `pipeline`
    /// (empty when the run is for another pipeline, or clean).
    pub fn stream_failures(&self, pipeline: &str, stream: &str) -> Vec<el_engine::ColumnFailures> {
        self.run
            .as_ref()
            .filter(|run| run.pipeline.as_ref() == pipeline)
            .and_then(|run| run.streams.iter().find(|row| row.stream.as_ref() == stream))
            .map(|row| row.column_failures.clone())
            .unwrap_or_default()
    }

    /// Opens the failing-columns grid for stream `ix` in the console.
    fn show_cast_failures(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.run.as_ref().and_then(|run| run.streams.get(ix)) else {
            return;
        };
        if row.column_failures.is_empty() {
            return;
        }
        let title = super::runs_panel::failures_title(&row.stream);
        let (columns, rows) = super::runs_panel::failures_table(&row.column_failures);
        // Leases Workspace then the panel; show_preview never reads this
        // view back, so the chain stays single-ownership.
        self.workspace
            .update(cx, |workspace, cx| {
                let Some(panel) = workspace.panel::<super::ElRunsPanel>(cx) else {
                    return;
                };
                panel.update(cx, |panel, cx| panel.show_preview(title, columns, rows, cx));
            })
            .ok();
    }

    pub fn start_run(
        &mut self,
        project_root: PathBuf,
        pipeline: Arc<Pipeline>,
        cx: &mut Context<Self>,
    ) {
        self.start_run_streams(project_root, pipeline, None, Vec::new(), cx);
    }

    /// Runs `only` those streams (all when empty).
    pub fn start_run_streams(
        &mut self,
        project_root: PathBuf,
        pipeline: Arc<Pipeline>,
        spec_path: Option<PathBuf>,
        only: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.is_running() {
            return;
        }
        let spec = pipeline.clone();
        let pipeline: Arc<Pipeline> = if only.is_empty() {
            pipeline
        } else {
            let mut subset = (*pipeline).clone();
            subset.streams.retain(|stream| only.contains(&stream.name));
            Arc::new(subset)
        };
        if pipeline.streams.is_empty() {
            return;
        }
        let cancel = CancelFlag::default();
        self.run = Some(ActiveRun {
            pipeline: pipeline.pipeline.clone().into(),
            project_root: project_root.clone(),
            spec_path,
            spec,
            started: Instant::now(),
            streams: pipeline
                .streams
                .iter()
                .map(|stream| StreamRow {
                    stream: stream.name.clone().into(),
                    ..Default::default()
                })
                .collect(),
            cancel: cancel.clone(),
            cancelling: false,
            finished: None,
            fatal: None,
        });

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let request = el_engine::run::RunRequest {
            project_root,
            pipeline: (*pipeline).clone(),
            worker: super::find_worker(),
            driver: None,
            chunk_rows: 50_000,
        profile_override: None,
        };
        let engine_cancel = cancel.clone();
        // The engine run is blocking: one background thread for its
        // lifetime, cancellation via the flag between chunks.
        let engine = cx.background_spawn(async move {
            el_engine::run::run_pipeline(&request, &tx, &engine_cancel)
        });

        self._run = cx.spawn(async move |this, cx| {
            use futures::future::FusedFuture as _;
            let mut engine = std::pin::pin!(engine.fuse());
            let mut engine_result = None;
            loop {
                futures::select_biased! {
                    event = rx.next() => {
                        match event {
                            Some(event) => {
                                // Drain whatever else is immediately ready
                                // before one notify. NOTE: try_recv hitting
                                // Closed terminates the receiver, so the
                                // next rx.next() is born terminated — the
                                // `complete` arm below is what keeps a
                                // fully-terminated select from panicking.
                                let mut events = vec![event];
                                while let Ok(next) = rx.try_recv() {
                                    events.push(next);
                                }
                                this.update(cx, |this, cx| {
                                    for event in events {
                                        this.absorb(event);
                                    }
                                    cx.notify();
                                })
                                .ok();
                            }
                            None => break,
                        }
                    }
                    result = engine => {
                        engine_result = Some(result);
                    }
                    complete => break,
                }
            }
            // The channel can close before the engine branch is taken —
            // the run's verdict must not be lost with it. The only way
            // engine's Fuse is terminated is the select arm that fills
            // engine_result, so this await cannot hang.
            if engine_result.is_none() && !engine.is_terminated() {
                engine_result = Some(engine.as_mut().await);
            }
            this.update(cx, |this, cx| {
                if let Some(Err(error)) = engine_result {
                    if let Some(run) = &mut this.run {
                        if run.finished.is_none() {
                            run.fatal = Some(format!("{error:#}").into());
                        }
                    }
                }
                this.announce_verdict(cx);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// One toast per run: what happened, in numbers.
    fn announce_verdict(&self, cx: &mut Context<Self>) {
        let Some(run) = &self.run else { return };
        let failed: Vec<&str> = run
            .streams
            .iter()
            .filter(|row| row.error.is_some())
            .map(|row| row.stream.as_ref())
            .collect();
        let rows: u64 = run.streams.iter().map(|row| row.rows_written).sum();
        let pipeline = run.pipeline.clone();
        let message = match (&run.fatal, failed.is_empty()) {
            _ if run.cancelling => Some((true, format!("{pipeline} cancelled."))),
            (Some(fatal), _) => Some((false, format!("{pipeline} failed: {fatal}"))),
            (None, false) => Some((
                false,
                format!(
                    "{pipeline}: {} of {} stream(s) failed — {}",
                    failed.len(),
                    run.streams.len(),
                    failed.join(", ")
                ),
            )),
            (None, true) if run.finished.is_some() => Some((
                true,
                format!(
                    "{pipeline} finished: {} stream(s), {rows} rows written.",
                    run.streams.len()
                ),
            )),
            _ => None,
        };
        if let Some((ok, message)) = message {
            self.workspace
                .update(cx, |workspace, cx| {
                    if ok {
                        super::toast(workspace, &message, cx);
                    } else {
                        super::toast_error(workspace, &message, None, cx);
                    }
                })
                .ok();
        }
    }

    /// Re-runs just the streams that failed last time.
    fn rerun_failed(&mut self, cx: &mut Context<Self>) {
        let Some(run) = &self.run else { return };
        let failed: Vec<String> = run
            .streams
            .iter()
            .filter(|row| row.error.is_some())
            .map(|row| row.stream.to_string())
            .collect();
        if failed.is_empty() {
            return;
        }
        let root = run.project_root.clone();
        let spec_path = run.spec_path.clone();
        // Re-read the YAML: the whole point of re-running is that the
        // user fixed something since.
        let spec = match spec_path
            .as_ref()
            .map(|path| el_engine::spec::load_pipeline(path))
        {
            Some(Ok(fresh)) => Arc::new(fresh),
            Some(Err(error)) => {
                self.workspace
                    .update(cx, |workspace, cx| {
                        super::toast_error(
                            workspace,
                            &format!("Can't re-run: {error}"),
                            spec_path.clone(),
                            cx,
                        )
                    })
                    .ok();
                return;
            }
            None => run.spec.clone(),
        };
        self.start_run_streams(root, spec, spec_path, failed, cx);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.run {
            run.cancel.cancel();
            run.cancelling = true;
            cx.notify();
        }
    }

    fn absorb(&mut self, event: ProgressEvent) {
        let Some(run) = &mut self.run else { return };
        match event {
            ProgressEvent::RunStarted { .. } => {}
            ProgressEvent::StreamStarted { stream } => {
                if let Some(row) = run.streams.iter_mut().find(|row| row.stream.as_ref() == stream)
                {
                    row.phase = Some(Phase::Connect);
                }
            }
            ProgressEvent::Chunk {
                stream,
                phase,
                rows_read,
                rows_written,
                cast_failures,
            } => {
                if let Some(row) = run.streams.iter_mut().find(|row| row.stream.as_ref() == stream)
                {
                    row.phase = Some(phase);
                    row.rows_read = rows_read;
                    row.rows_written = rows_written;
                    row.cast_failures = cast_failures;
                }
            }
            ProgressEvent::StreamFinished {
                stream,
                rows_read,
                rows_written,
                cast_failures,
                column_failures,
            } => {
                if let Some(row) = run.streams.iter_mut().find(|row| row.stream.as_ref() == stream)
                {
                    row.phase = None;
                    row.rows_read = rows_read;
                    row.rows_written = rows_written;
                    row.cast_failures = cast_failures;
                    row.column_failures = column_failures;
                    row.done = true;
                }
            }
            ProgressEvent::StreamFailed { stream, error } => {
                if let Some(row) = run.streams.iter_mut().find(|row| row.stream.as_ref() == stream)
                {
                    row.phase = None;
                    row.error = Some(error.into());
                    row.done = true;
                }
            }
            ProgressEvent::RunFinished { ok } => {
                run.finished = Some(ok);
            }
        }
    }

    fn phase_label(phase: Phase) -> &'static str {
        match phase {
            Phase::Connect => "connect",
            Phase::Extract => "extract",
            Phase::Cast => "cast",
            Phase::Stage => "stage",
            Phase::Copy => "copy",
            Phase::Merge => "merge",
            Phase::Finalize => "finalize",
        }
    }
}

impl Render for ElRunView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let Some(run) = &self.run else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    Label::new("No EL run yet — press ▶ on a pipeline canvas.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        };

        let status: SharedString = if let Some(fatal) = &run.fatal {
            format!("failed: {fatal}").into()
        } else {
            match run.finished {
                Some(true) => "finished".into(),
                Some(false) => "finished with failures".into(),
                None if run.cancelling => "cancelling…".into(),
                None => format!("running · {:.0}s", run.started.elapsed().as_secs_f32()).into(),
            }
        };
        let running = run.finished.is_none() && run.fatal.is_none();

        let mut body = v_flex()
            .size_full()
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(Label::new(run.pipeline.clone()).size(LabelSize::Small))
                    .child(
                        Label::new(status)
                            .size(LabelSize::XSmall)
                            .color(match (run.finished, &run.fatal) {
                                (_, Some(_)) | (Some(false), _) => Color::Error,
                                (Some(true), _) => Color::Success,
                                _ => Color::Muted,
                            }),
                    )
                    .child(div().flex_1())
                    .when(
                        !running && run.streams.iter().any(|row| row.error.is_some()),
                        |header| {
                            header.child(
                                Button::new("el-run-rerun-failed", "Re-run failed")
                                    .label_size(LabelSize::XSmall)
                                    .tooltip(Tooltip::text(
                                        "Run only the streams that failed",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.rerun_failed(cx)
                                    })),
                            )
                        },
                    )
                    .when(running, |header| {
                        header.child(
                            IconButton::new("el-run-cancel", IconName::Close)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Cancel the run"))
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                    }),
            );

        for (ix, row) in run.streams.iter().enumerate() {
            let phase_chip: Option<SharedString> = row
                .phase
                .map(|phase| Self::phase_label(phase).into())
                .or_else(|| {
                    row.done.then(|| {
                        if row.error.is_some() {
                            "failed".into()
                        } else {
                            "done".into()
                        }
                    })
                });
            let chip_color = if row.error.is_some() {
                Color::Error
            } else if row.done {
                Color::Success
            } else {
                Color::Info
            };
            let mut line = h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(colors.border_variant)
                .child(
                    div().w(px(160.)).flex_shrink_0().child(
                        Label::new(row.stream.clone()).size(LabelSize::Small).truncate(),
                    ),
                )
                .child(
                    div().w(px(70.)).flex_shrink_0().child(
                        Label::new(phase_chip.unwrap_or_else(|| "queued".into()))
                            .size(LabelSize::XSmall)
                            .color(chip_color),
                    ),
                )
                .child(
                    Label::new(format!(
                        "{} read · {} written",
                        row.rows_read, row.rows_written
                    ))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
                );
            if row.cast_failures > 0 {
                let badge = format!("{} cast failures", row.cast_failures);
                if row.column_failures.is_empty() {
                    // Count only, until the stream finishes and reports
                    // which columns failed.
                    line = line.child(
                        Label::new(badge)
                            .size(LabelSize::XSmall)
                            .color(Color::Warning),
                    );
                } else {
                    line = line.child(
                        Button::new(("el-run-casts", ix), badge)
                            .label_size(LabelSize::XSmall)
                            .color(Color::Warning)
                            .tooltip(Tooltip::text(
                                "Show the failing columns and sample values",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_cast_failures(ix, cx)
                            })),
                    );
                }
            }
            if let Some(error) = &row.error {
                line = line.child(
                    div().flex_1().min_w_0().child(
                        Label::new(error.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Error)
                            .truncate(),
                    ),
                );
            }
            body = body.child(line);
        }
        let _ = &self.workspace;
        body.into_any_element()
    }
}
