//! Kind Index for V3 Native Backend
//!
//! Provides O(1) lookup of node IDs by kind, eliminating O(n) full scans.
//!
//! ## Design
//!
//! - Simple in-memory HashMap: kind -> Vec<node_id>
//! - Rebuilt on reopen from existing node data
//! - Updated incrementally on insert
//! - No persistent storage required (rebuildable from primary data)

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Kind Index: maps kind string to list of node IDs with that kind
///
/// This provides O(1) lookup for query_nodes_by_kind, eliminating
/// the O(n) full scan that was previously required.
#[derive(Clone)]
pub struct KindIndex {
    inner: Arc<RwLock<HashMap<String, Vec<i64>>>>,
}

impl KindIndex {
    /// Create a new empty kind index
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a node ID to the index for a given kind
    pub fn insert(&self, kind: String, node_id: i64) {
        let mut index = self.inner.write();
        index.entry(kind).or_default().push(node_id);
    }

    /// Get all node IDs for a given kind
    ///
    /// Returns empty Vec if kind not found (O(1) operation)
    pub fn get(&self, kind: &str) -> Vec<i64> {
        let index = self.inner.read();
        index.get(kind).cloned().unwrap_or_default()
    }

    /// Get all kinds in the index
    pub fn all_kinds(&self) -> Vec<String> {
        let index = self.inner.read();
        index.keys().cloned().collect()
    }

    /// Clear the index (used before rebuilding)
    pub fn clear(&self) {
        let mut index = self.inner.write();
        index.clear();
    }

    /// Get the number of kinds in the index
    pub fn kind_count(&self) -> usize {
        let index = self.inner.read();
        index.len()
    }

    /// Get total node count across all kinds
    pub fn total_nodes(&self) -> usize {
        let index = self.inner.read();
        index.values().map(|v| v.len()).sum()
    }

    /// Export all index data for persistence
    pub(crate) fn export(&self) -> HashMap<String, Vec<i64>> {
        self.inner.read().clone()
    }

    /// Import index data from persistence
    pub(crate) fn import(&self, data: HashMap<String, Vec<i64>>) {
        let mut index = self.inner.write();
        *index = data;
    }
}

impl Default for KindIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let index = KindIndex::new();
        index.insert("Function".to_string(), 1);
        index.insert("Function".to_string(), 2);
        index.insert("Class".to_string(), 3);

        assert_eq!(index.get("Function"), vec![1, 2]);
        assert_eq!(index.get("Class"), vec![3]);
        assert_eq!(index.get("Unknown"), Vec::<i64>::new());
    }

    #[test]
    fn test_clear() {
        let index = KindIndex::new();
        index.insert("Test".to_string(), 1);
        assert_eq!(index.kind_count(), 1);
        index.clear();
        assert_eq!(index.kind_count(), 0);
    }
}
