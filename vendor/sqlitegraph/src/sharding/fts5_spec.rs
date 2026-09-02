//! FTS5 full-text search specification for PropertyStore.
//!
//! Tests define expected behavior for content indexing and search.
//! Implementation must pass all specs before being considered complete.

/// Content match result from FTS5 search.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentMatch {
    pub token_id: u32,
    pub score: f32,
    pub content_type: String,
    pub snippet: String,
}

#[cfg(test)]
mod specs {
    use crate::sharding::PropertyStore;

    /// Spec: Basic FTS5 index and search functionality.
    #[test]
    fn spec_fts5_index_and_search() {
        let mut store = PropertyStore::in_memory().unwrap();

        // Index content
        store
            .index_content(1000, "docstring", "Parses input JSON into struct")
            .unwrap();
        store
            .index_content(1001, "comment", "// TODO: refactor this function")
            .unwrap();
        store
            .index_content(1002, "docstring", "Validates HTTP request headers")
            .unwrap();

        // Search should return ranked results (BM25)
        let results = store.search_content("parse json", 10).unwrap();

        // Should match "Parses input JSON"
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].token_id, 1000);
        assert_eq!(results[0].content_type, "docstring");
        // BM25 score can be negative (lower is better)
        assert!(results[0].snippet.contains("JSON") || results[0].snippet.contains("json"));
    }

    /// Spec: FTS5 tokenization and stemming.
    #[test]
    fn spec_fts5_tokenization() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "body", "function validate_request() {}")
            .unwrap();
        store
            .index_content(1001, "docstring", "Validating incoming HTTP requests")
            .unwrap();

        // Stemming: "validate" matches "validating", "validates"
        let results = store.search_content("validate request", 10).unwrap();

        // Should match both due to stemming
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.token_id == 1000));
        assert!(results.iter().any(|r| r.token_id == 1001));
    }

    /// Spec: Phrase search with quotes.
    #[test]
    fn spec_fts5_phrase_search() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "Processes user authentication tokens")
            .unwrap();
        store
            .index_content(1001, "docstring", "Processes user session tokens")
            .unwrap();

        // Exact phrase match
        let results = store
            .search_content("\"authentication tokens\"", 10)
            .unwrap();

        // Only match exact phrase
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].token_id, 1000);
    }

    /// Spec: Content type filtering.
    #[test]
    fn spec_fts5_content_type_filter() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "Parses JSON")
            .unwrap();
        store
            .index_content(1001, "comment", "// Parses JSON")
            .unwrap();
        store
            .index_content(1002, "body", "fn parse_json() {}")
            .unwrap();

        // Filter by content type
        let results = store.search_content_type("json", "docstring", 10).unwrap();

        // Only docstrings
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].token_id, 1000);
    }

    /// Spec: Search result ranking (BM25).
    #[test]
    fn spec_fts5_ranking() {
        let mut store = PropertyStore::in_memory().unwrap();

        // Token appears multiple times → higher rank
        store
            .index_content(1000, "docstring", "parse json parse json parse")
            .unwrap();
        store
            .index_content(1001, "docstring", "parse json once")
            .unwrap();

        let results = store.search_content("parse json", 10).unwrap();

        // First result has higher frequency → better rank (lower BM25 score)
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].token_id, 1000);
        // BM25: lower score = better match
        assert!(results[0].score < results[1].score);
    }

    /// Spec: Snippet generation around matches.
    #[test]
    fn spec_fts5_snippet_generation() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(
                1000,
                "docstring",
                "This function parses JSON input from HTTP requests",
            )
            .unwrap();

        let results = store.search_content("json", 10).unwrap();

        assert_eq!(results.len(), 1);
        // Snippet should contain match with <marker> tags
        assert!(results[0].snippet.contains("JSON") || results[0].snippet.contains("json"));
    }

    /// Spec: Update existing content.
    #[test]
    fn spec_fts5_update_content() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "Original content")
            .unwrap();
        store
            .index_content(1000, "docstring", "Updated content with JSON parsing")
            .unwrap();

        let results = store.search_content("json", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].token_id, 1000);
    }

    /// Spec: Delete content removes from FTS5 index.
    #[test]
    fn spec_fts5_delete_content() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "Content to delete")
            .unwrap();
        store.delete_content(1000).unwrap();

        let results = store.search_content("content delete", 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    /// Spec: Empty search returns no results.
    #[test]
    fn spec_fts5_empty_search() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "Some content")
            .unwrap();

        let results = store.search_content("", 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    /// Spec: Limit parameter controls result count.
    #[test]
    fn spec_fts5_result_limit() {
        let mut store = PropertyStore::in_memory().unwrap();

        for i in 0..10 {
            store
                .index_content(i, "docstring", &format!("Content {}", i))
                .unwrap();
        }

        let results = store.search_content("content", 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    /// Spec: Case-insensitive search.
    #[test]
    fn spec_fts5_case_insensitive() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .index_content(1000, "docstring", "JSON Parsing Function")
            .unwrap();

        let results_lower = store.search_content("json", 10).unwrap();
        let results_upper = store.search_content("JSON", 10).unwrap();

        assert_eq!(results_lower.len(), 1);
        assert_eq!(results_upper.len(), 1);
        assert_eq!(results_lower[0].token_id, results_upper[0].token_id);
    }
}
