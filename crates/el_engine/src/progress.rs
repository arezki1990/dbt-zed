//! Progress events and cancellation, shared by every run surface (panel,
//! headless CLI, and later the daemon). Events are plain data so they
//! serialize over any transport.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

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
