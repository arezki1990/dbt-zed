//! The per-stream mapping & casting editor: source columns on the left,
//! Snowflake types on the right, include/strict toggles, rename fields.
//! Drafts never half-apply — Apply serializes the whole draft into the
//! spec and writes the YAML through the buffer.

use anyhow::Result;
use editor::Editor;
use gpui::{Context, Entity, SharedString, Window};
use ui::prelude::*;

use el_engine::spec::{ColumnSpec, Mode, Pipeline, Select};

/// The type list offered in the dropdown; anything else stays possible by
/// editing the YAML (Custom entry arrives with U4 polish).
pub const SNOWFLAKE_TYPES: &[&str] = &[
    "NUMBER(38,0)",
    "NUMBER(18,2)",
    "FLOAT",
    "VARCHAR",
    "BOOLEAN",
    "DATE",
    "TIME",
    "TIMESTAMP_NTZ",
    "TIMESTAMP_TZ",
    "BINARY",
    "VARIANT",
];

pub struct ColumnDraft {
    pub name: SharedString,
    /// Source dtype from the probe ("str", "i64"…), when it has arrived.
    pub inferred: Option<SharedString>,
    pub rename: Entity<Editor>,
    /// Snowflake type spelling; None = inherit the source type.
    pub cast: Option<SharedString>,
    pub strict: bool,
    pub include: bool,
    /// chrono parse format carried through untouched (edited in YAML).
    parse: Option<String>,
}

pub struct MappingEditorState {
    pub stream_ix: usize,
    pub stream_name: SharedString,
    pub target_table: Entity<Editor>,
    /// Airbyte's per-stream sync settings: mode, primary key, cursor
    /// field (our update_key). Names are source-side, like the spec's.
    pub mode: Mode,
    pub primary_key: Vec<String>,
    pub update_key: Option<String>,
    pub drafts: Vec<ColumnDraft>,
    /// True until the background schema probe lands (or fails).
    pub probing: bool,
    pub probe_error: Option<SharedString>,
    pub dirty: bool,
}

impl MappingEditorState {
    /// Builds drafts from the spec alone; the caller kicks the probe that
    /// fills `inferred` and appends unspecced source columns.
    pub fn open(
        pipeline: &Pipeline,
        stream_ix: usize,
        window: &mut Window,
        cx: &mut Context<crate::el::ElPipelineCanvas>,
    ) -> Self {
        let stream = &pipeline.streams[stream_ix];
        let mut make_editor = |text: &str, placeholder: &str, cx: &mut Context<_>| {
            let text = text.to_owned();
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text(&placeholder, window, cx);
                if !text.is_empty() {
                    editor.set_text(text, window, cx);
                }
                editor
            })
        };

        let excluded: Vec<&str> = stream
            .select
            .as_ref()
            .map(|select| select.exclude.iter().map(String::as_str).collect())
            .unwrap_or_default();

        let mut drafts = Vec::with_capacity(stream.columns.len());
        for rule in &stream.columns {
            let rename = make_editor(rule.rename.as_deref().unwrap_or(""), "rename…", cx);
            drafts.push(ColumnDraft {
                name: rule.name.clone().into(),
                inferred: None,
                rename,
                cast: rule.cast.as_ref().map(|cast| cast.to_string().into()),
                strict: rule.strict.unwrap_or(false),
                include: !excluded.contains(&rule.name.as_str()),
                parse: rule.parse.clone(),
            });
        }

        Self {
            stream_ix,
            stream_name: stream.name.clone().into(),
            mode: stream.mode(pipeline.defaults.as_ref()),
            primary_key: stream.primary_key.clone(),
            update_key: stream.update_key.clone(),
            target_table: make_editor(
                stream.target_table.as_deref().unwrap_or(""),
                &stream.target_table(&pipeline.target),
                cx,
            ),
            drafts,
            probing: true,
            probe_error: None,
            dirty: false,
        }
    }

    /// Merges the probe result in: fills inferred dtypes and appends
    /// source columns the spec doesn't mention yet.
    pub fn absorb_probe(
        &mut self,
        columns: &[el_engine::PreviewColumn],
        excluded: &[String],
        window: &mut Window,
        cx: &mut Context<crate::el::ElPipelineCanvas>,
    ) {
        self.probing = false;
        for column in columns {
            if let Some(draft) = self
                .drafts
                .iter_mut()
                .find(|draft| draft.name.as_ref() == column.name)
            {
                draft.inferred = Some(column.source_dtype.clone().into());
            } else {
                let rename = cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text("rename…", window, cx);
                    editor
                });
                self.drafts.push(ColumnDraft {
                    name: column.name.clone().into(),
                    inferred: Some(column.source_dtype.clone().into()),
                    rename,
                    cast: None,
                    strict: false,
                    include: !excluded.iter().any(|name| name == &column.name),
                    parse: None,
                });
            }
        }
    }

    /// Serializes the draft into the pipeline. Returns false when nothing
    /// changed.
    pub fn apply(&self, pipeline: &mut Pipeline, cx: &Context<crate::el::ElPipelineCanvas>) -> Result<bool> {
        let Some(stream) = pipeline.streams.get_mut(self.stream_ix) else {
            anyhow::bail!("stream disappeared from the spec");
        };

        let target_table = self.target_table.read(cx).text(cx).trim().to_owned();
        stream.target_table = (!target_table.is_empty()).then_some(target_table);

        // Sync settings — incremental needs both halves of the cursor
        // story before it can run.
        if self.mode == Mode::Incremental {
            if self.primary_key.is_empty() {
                anyhow::bail!("incremental sync needs a primary key — pick one under Sync");
            }
            if self.update_key.is_none() {
                anyhow::bail!("incremental sync needs a cursor column — pick one under Sync");
            }
        }
        stream.mode = Some(self.mode);
        stream.primary_key = self.primary_key.clone();
        stream.update_key = self.update_key.clone();

        // Exclusions: keep an existing include-list philosophy if present,
        // else use select.exclude.
        let excluded: Vec<String> = self
            .drafts
            .iter()
            .filter(|draft| !draft.include)
            .map(|draft| draft.name.to_string())
            .collect();
        match (&mut stream.select, excluded.is_empty()) {
            (Some(select), _) if !select.include.is_empty() => {
                select
                    .include
                    .retain(|name| !excluded.iter().any(|ex| ex == name));
            }
            (Some(select), _) => select.exclude = excluded,
            (None, false) => {
                stream.select = Some(Select {
                    include: vec![],
                    exclude: excluded,
                });
            }
            (None, true) => {}
        }

        stream.columns = self
            .drafts
            .iter()
            .filter(|draft| draft.include)
            .filter_map(|draft| {
                let rename = self
                    .drafts
                    .iter()
                    .find(|d| d.name == draft.name)
                    .map(|d| d.rename.read(cx).text(cx).trim().to_owned())
                    .filter(|text| !text.is_empty());
                let cast = draft.cast.as_ref().map(|cast| cast.to_string());
                if rename.is_none() && cast.is_none() && !draft.strict && draft.parse.is_none()
                {
                    return None; // pure pass-through needs no rule
                }
                Some(ColumnSpec {
                    name: draft.name.to_string(),
                    cast: cast.and_then(|cast| cast.parse().ok()),
                    strict: draft.strict.then_some(true),
                    parse: draft.parse.clone(),
                    rename,
                })
            })
            .collect();
        Ok(true)
    }
}
