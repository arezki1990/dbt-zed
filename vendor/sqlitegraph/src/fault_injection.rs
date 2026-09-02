//! Fault injection for testing failure scenarios
//!
//! This module provides controlled fault injection for testing resilience
//! of various operations against failures.

// Module-level dead_code suppression removed as part of CODE-02
// Individual items may still need specific allows where truly unused

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use crate::SqliteGraphError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub enum FaultPoint {
    BulkInsertEntitiesBeforeCommit,
    BulkInsertEdgesBeforeCommit,
    RecoveryLoadBeforeCommit,
}

struct FaultEntry {
    remaining: usize,
}

fn registry() -> &'static Mutex<HashMap<FaultPoint, FaultEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<FaultPoint, FaultEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn check_fault(point: FaultPoint) -> Result<(), SqliteGraphError> {
    let mut guard = registry().lock().expect("fault registry poisoned");
    if let Some(entry) = guard.get_mut(&point)
        && entry.remaining > 0
    {
        entry.remaining -= 1;
        if entry.remaining == 0 {
            guard.remove(&point);
        }
        return Err(SqliteGraphError::fault_injection(format!("{point:?}")));
    }
    Ok(())
}
