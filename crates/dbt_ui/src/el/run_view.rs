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
    pub error: Option<SharedString>,
    pub done: bool,
}

struct ActiveRun {
    pipeline: SharedString,
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

    pub fn start_run(
        &mut self,
        project_root: PathBuf,
        pipeline: Arc<Pipeline>,
        cx: &mut Context<Self>,
    ) {
        if self.is_running() {
            return;
        }
        let cancel = CancelFlag::default();
        self.run = Some(ActiveRun {
            pipeline: pipeline.pipeline.clone().into(),
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
                cx.notify();
            })
            .ok();
        });
        cx.notify();
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
            } => {
                if let Some(row) = run.streams.iter_mut().find(|row| row.stream.as_ref() == stream)
                {
                    row.phase = None;
                    row.rows_read = rows_read;
                    row.rows_written = rows_written;
                    row.cast_failures = cast_failures;
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
                line = line.child(
                    Label::new(format!("{} cast failures", row.cast_failures))
                        .size(LabelSize::XSmall)
                        .color(Color::Warning),
                );
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
            let _ = ix;
        }
        let _ = &self.workspace;
        body.into_any_element()
    }
}
