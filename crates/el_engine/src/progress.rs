//! Progress events and cancellation, shared by every run surface (panel,
//! headless CLI, and later the daemon). Events are plain data so they
//! serialize over any transport.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::cast::ColumnFailures;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Connect,
    Extract,
    Cast,
    Stage,
    Copy,
    Merge,
    Finalize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgressEvent {
    RunStarted {
        pipeline: String,
        streams: Vec<String>,
    },
    StreamStarted {
        stream: String,
    },
    Chunk {
        stream: String,
        phase: Phase,
        rows_read: u64,
        rows_written: u64,
        cast_failures: u64,
    },
    StreamFinished {
        stream: String,
        rows_read: u64,
        rows_written: u64,
        cast_failures: u64,
        /// Which columns failed, with sample values. Optional on the wire:
        /// older daemons omit it and the IDE then shows the count alone.
        #[serde(default)]
        column_failures: Vec<ColumnFailures>,
    },
    StreamFailed {
        stream: String,
        error: String,
    },
    RunFinished {
        ok: bool,
    },
}

/// Cooperative cancellation, checked between chunks and before every
/// loader request. Cheap to clone into background tasks.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event from a daemon that predates per-column failures still
    /// deserializes — the samples are simply absent.
    #[test]
    fn stream_finished_without_column_failures_deserializes() {
        let old = r#"{"StreamFinished":{"stream":"s","rows_read":1,"rows_written":1,"cast_failures":0}}"#;
        let event: ProgressEvent = serde_json::from_str(old).unwrap();
        match event {
            ProgressEvent::StreamFinished { column_failures, cast_failures, .. } => {
                assert_eq!(cast_failures, 0);
                assert!(column_failures.is_empty());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn stream_finished_round_trips_column_failures() {
        let failures = vec![
            ColumnFailures {
                column: "amount".into(),
                count: 2,
                samples: vec!["oops".into(), "n/a".into()],
            },
            ColumnFailures {
                column: "placed_at".into(),
                count: 1,
                samples: vec!["never".into()],
            },
        ];
        let event = ProgressEvent::StreamFinished {
            stream: "s".into(),
            rows_read: 3,
            rows_written: 3,
            cast_failures: 3,
            column_failures: failures.clone(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::StreamFinished { column_failures, .. } => {
                assert_eq!(column_failures, failures);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
