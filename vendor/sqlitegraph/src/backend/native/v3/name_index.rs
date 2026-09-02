//! Name index for fast exact, prefix, and substring name lookups
//!
//! Supports:
//! - Exact match: "my_func" → O(1) lookup
//! - Prefix match: "my_func*" → O(k) lookup where k = matches
//! - Substring match: "func" → O(n) lookup where n = unique names (contains)
//! - GLOB match: "my_*func?" → O(n) lookup with `*` (any sequence) and
//!   `?` (exactly one char) wildcards, mirroring SQLite GLOB semantics.
//!
//! Does NOT support:
//! - Character classes: "func[abc]"

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Name index mapping names to node IDs
///
/// Structure: HashMap<name, Vec<node_id>>
/// - Fast exact lookup: O(1)
/// - Fast prefix lookup: O(m) where m = unique names starting with prefix
pub struct NameIndex {
    inner: Arc<RwLock<HashMap<String, Vec<i64>>>>,
}

impl Default for NameIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl NameIndex {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a node name into the index
    pub fn insert(&self, name: String, node_id: i64) {
        let mut index = self.inner.write();
        index.entry(name).or_default().push(node_id);
    }

    /// Clear all entries from the index
    pub fn clear(&self) {
        let mut index = self.inner.write();
        index.clear();
    }

    /// Exact match lookup
    pub fn get_exact(&self, name: &str) -> Vec<i64> {
        let index = self.inner.read();
        index.get(name).cloned().unwrap_or_default()
    }

    /// Prefix match lookup
    /// Returns all node IDs where name starts with the given prefix
    pub fn get_prefix(&self, prefix: &str) -> Vec<i64> {
        let index = self.inner.read();
        let mut result = Vec::new();
        for (name, ids) in index.iter() {
            if name.starts_with(prefix) {
                result.extend(ids.clone());
            }
        }
        result
    }

    /// Substring match lookup
    /// Returns all node IDs where name contains the given substring
    /// This is O(n) where n = unique names in the index
    pub fn get_substring(&self, substring: &str) -> Vec<i64> {
        let index = self.inner.read();
        let mut result = Vec::new();
        for (name, ids) in index.iter() {
            if name.contains(substring) {
                result.extend(ids.clone());
            }
        }
        result
    }

    /// GLOB match lookup.
    ///
    /// Matches names against a SQLite-style GLOB pattern where `*` matches any
    /// sequence of characters (including empty) and `?` matches exactly one
    /// character. The match is case-sensitive and the whole name must match
    /// (anchored). This is O(n) where n = unique names in the index.
    pub fn get_glob(&self, pattern: &str) -> Vec<i64> {
        let index = self.inner.read();
        let mut result = Vec::new();
        for (name, ids) in index.iter() {
            if glob_match(pattern, name) {
                result.extend(ids.clone());
            }
        }
        result
    }

    /// Get index statistics
    pub fn stats(&self) -> NameIndexStats {
        let index = self.inner.read();
        let total_names = index.len();
        let total_nodes: usize = index.values().map(|v| v.len()).sum();
        NameIndexStats {
            unique_names: total_names,
            total_nodes,
        }
    }
}

pub struct NameIndexStats {
    pub unique_names: usize,
    pub total_nodes: usize,
}

impl NameIndex {
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

/// Match a string against a SQLite-style GLOB pattern.
///
/// Wildcards: `*` matches any sequence of characters (including empty), `?`
/// matches exactly one character. The match is case-sensitive and anchored
/// (the entire `text` must match the entire `pattern`). This mirrors the
/// semantics of SQLite's `name GLOB pattern` operator for the `*` and `?`
/// metacharacters (character classes `[...]` are intentionally not supported
/// and are treated as literals).
///
/// Implemented as a recursive backtracking matcher over `&str` slices.
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut p = pattern.chars().peekable();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&mut p, &t, 0)
}

/// Recursive core of [`glob_match`].
///
/// `p` is the remaining pattern (peekable char iterator) and `chars` is the
/// full text with `idx` pointing at the current character under consideration.
fn glob_match_inner<I: Iterator<Item = char>>(
    p: &mut std::iter::Peekable<I>,
    chars: &[char],
    mut idx: usize,
) -> bool {
    loop {
        match p.next() {
            None => return idx == chars.len(),
            Some('?') => {
                // Exactly one character required.
                if idx >= chars.len() {
                    return false;
                }
                idx += 1;
            }
            Some('*') => {
                // Collapse consecutive '*' (a**b === a*b).
                while p.peek() == Some(&'*') {
                    p.next();
                }
                // '*' can match the rest greedily; try every possible split,
                // including matching zero characters (idx unchanged).
                let rest: Vec<char> = p.by_ref().collect();
                for end in idx..=chars.len() {
                    let mut rp = rest.iter().copied().peekable();
                    if glob_match_inner(&mut rp, chars, end) {
                        return true;
                    }
                }
                return false;
            }
            Some(literal) => {
                if idx >= chars.len() || chars[idx] != literal {
                    return false;
                }
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_basic() {
        // No wildcards → exact, anchored.
        assert!(glob_match("User", "User"));
        assert!(!glob_match("User", "SuperUser"));
        assert!(!glob_match("User", "UserAdmin"));
        assert!(!glob_match("User", "Use"));
        // Trailing star → prefix.
        assert!(glob_match("User*", "User"));
        assert!(glob_match("User*", "UserAdmin"));
        assert!(!glob_match("User*", "SuperUser"));
        // Leading star → suffix.
        assert!(glob_match("*User", "User"));
        assert!(glob_match("*User", "SuperUser"));
        assert!(!glob_match("*User", "UserAdmin"));
        // Star both sides → contains.
        assert!(glob_match("*User*", "User"));
        assert!(glob_match("*User*", "SuperUser"));
        assert!(glob_match("*User*", "UserAdmin"));
        // Single-char wildcard '?'.
        assert!(glob_match("User?", "User1"));
        assert!(!glob_match("User?", "User"));
        assert!(!glob_match("User?", "User12"));
        assert!(glob_match("?ser", "User"));
        assert!(glob_match("Us?r", "User"));
        // Mixed wildcards.
        assert!(glob_match("U*r?", "User1"));
        assert!(glob_match("U*r?", "Usr1"));
        assert!(!glob_match("U*r?", "Usr"));
        // Bare star matches everything (including empty).
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        // Consecutive stars collapse.
        assert!(glob_match("a**b", "axyzb"));
        assert!(glob_match("a**b", "ab"));
        // Empty pattern matches only empty text.
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        // Case sensitivity (GLOB is case-sensitive).
        assert!(!glob_match("user", "User"));
    }

    #[test]
    fn test_exact_match() {
        let index = NameIndex::new();
        index.insert("func_a".to_string(), 1);
        index.insert("func_b".to_string(), 2);
        index.insert("class_a".to_string(), 3);

        assert_eq!(index.get_exact("func_a"), vec![1]);
        assert_eq!(index.get_exact("func_b"), vec![2]);
        assert_eq!(index.get_exact("nonexistent"), Vec::<i64>::new());
    }

    #[test]
    fn test_prefix_match() {
        let index = NameIndex::new();
        index.insert("func_a".to_string(), 1);
        index.insert("func_b".to_string(), 2);
        index.insert("class_a".to_string(), 3);
        index.insert("func_ab".to_string(), 4);

        let results = index.get_prefix("func");
        assert_eq!(results.len(), 3);
        assert!(results.contains(&1));
        assert!(results.contains(&2));
        assert!(results.contains(&4));
        assert!(!results.contains(&3));
    }

    #[test]
    fn test_multiple_nodes_same_name() {
        let index = NameIndex::new();
        index.insert("duplicate".to_string(), 1);
        index.insert("duplicate".to_string(), 2);
        index.insert("duplicate".to_string(), 3);

        let mut results = index.get_exact("duplicate");
        results.sort();
        assert_eq!(results, vec![1, 2, 3]);
    }

    #[test]
    fn test_stats() {
        let index = NameIndex::new();
        index.insert("a".to_string(), 1);
        index.insert("b".to_string(), 2);
        index.insert("a".to_string(), 3); // duplicate name

        let stats = index.stats();
        assert_eq!(stats.unique_names, 2);
        assert_eq!(stats.total_nodes, 3);
    }
}
