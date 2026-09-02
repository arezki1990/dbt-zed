//! Pattern triple definition and builder functionality.

use std::collections::HashMap;

use crate::{backend::BackendDirection, errors::SqliteGraphError};

/// A lightweight triple pattern for basic graph pattern matching.
///
/// Represents a single-hop pattern: (start_label)-[edge_type]->(end_label)
/// with optional property filters on start and end nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternTriple {
    /// Optional label filter for the start node
    pub start_label: Option<String>,
    /// Edge type to match (required)
    pub edge_type: String,
    /// Optional label filter for the end node  
    pub end_label: Option<String>,
    /// Optional property filters for the start node (key -> value)
    pub start_props: HashMap<String, String>,
    /// Optional property filters for the end node (key -> value)
    pub end_props: HashMap<String, String>,
    /// Direction of the pattern (default: Outgoing)
    pub direction: BackendDirection,
}

impl PatternTriple {
    /// Create a new pattern triple with the given edge type.
    pub fn new(edge_type: impl Into<String>) -> Self {
        Self {
            start_label: None,
            edge_type: edge_type.into(),
            end_label: None,
            start_props: HashMap::new(),
            end_props: HashMap::new(),
            direction: BackendDirection::Outgoing,
        }
    }

    /// Set the start node label filter.
    pub fn start_label(mut self, label: impl Into<String>) -> Self {
        self.start_label = Some(label.into());
        self
    }

    /// Set the end node label filter.
    pub fn end_label(mut self, label: impl Into<String>) -> Self {
        self.end_label = Some(label.into());
        self
    }

    /// Add a property filter for the start node.
    pub fn start_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.start_props.insert(key.into(), value.into());
        self
    }

    /// Add a property filter for the end node.
    pub fn end_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.end_props.insert(key.into(), value.into());
        self
    }

    /// Set the direction of the pattern.
    pub fn direction(mut self, direction: BackendDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Validate that the pattern is well-formed.
    pub fn validate(&self) -> Result<(), SqliteGraphError> {
        if self.edge_type.trim().is_empty() {
            return Err(SqliteGraphError::invalid_input("edge_type is required"));
        }
        Ok(())
    }
}
