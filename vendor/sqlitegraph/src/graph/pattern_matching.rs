//! Pattern matching methods for SqliteGraph.

use crate::errors::SqliteGraphError;

use super::SqliteGraph;

impl SqliteGraph {
    /// Match lightweight triple patterns using pattern engine.
    ///
    /// This method provides a simple interface for matching single-hop patterns
    /// like (start_label)-\[edge_type\]->(end_label) with optional property filters.
    ///
    /// # Arguments
    /// * `pattern` - The pattern triple to match
    ///
    /// # Returns
    /// A vector of triple matches in deterministic order
    pub fn match_triples(
        &self,
        pattern: &crate::pattern_engine::PatternTriple,
    ) -> Result<Vec<crate::pattern_engine::TripleMatch>, SqliteGraphError> {
        crate::pattern_engine::match_triples(self, pattern)
    }

    /// Match lightweight triple patterns using cache-enabled fast-path.
    ///
    /// This method provides an optimized version of pattern matching that:
    /// - Uses cache as a fast-path where safe
    /// - Falls back to SQL where pattern requires it
    /// - Returns IDENTICAL results to match_triples()
    /// - Maintains deterministic ordering
    ///
    /// # Arguments
    /// * `pattern` - The pattern triple to match
    ///
    /// # Returns
    /// A vector of triple matches in deterministic order
    pub fn match_triples_fast(
        &self,
        pattern: &crate::pattern_engine::PatternTriple,
    ) -> Result<Vec<crate::pattern_engine::TripleMatch>, SqliteGraphError> {
        crate::pattern_engine_cache::match_triples_fast(self, pattern)
    }
}
