//! Lightweight triple pattern matcher for SQLiteGraph.
//!
//! Provides a minimal, deterministic pattern matcher for basic Neo4j-style patterns:
//! (start_label)-[edge_type]->(end_label) with optional property filters.
//!
//! This is designed to be a simpler alternative to the full pattern system,
//! focusing on single-hop patterns with equality-based property filtering.

pub use matcher::{TripleMatch, match_triples};
pub use pattern::PatternTriple;

mod matcher;
mod pattern;
mod property;
mod query;

#[cfg(test)]
mod tests;
