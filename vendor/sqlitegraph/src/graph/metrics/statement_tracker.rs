use parking_lot::Mutex;
use std::collections::HashSet;

#[derive(Default)]
pub struct StatementTracker {
    seen: Mutex<HashSet<String>>,
}

impl StatementTracker {
    pub fn observe(&self, sql: &str) -> CacheObservation {
        let normalized = sql.trim().to_string();
        let mut guard = self.seen.lock();
        if guard.insert(normalized) {
            CacheObservation::Miss
        } else {
            CacheObservation::Hit
        }
    }
}

pub enum CacheObservation {
    Hit,
    Miss,
}
