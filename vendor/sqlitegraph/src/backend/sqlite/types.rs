//! Type definitions for SQLite backend operations.
//!
//! This module contains all input specification types, query configurations,
//! and direction enumerations needed for graph operations through the SQLite backend.

use serde::{Deserialize, Serialize};

/// Direction specification for graph traversal operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendDirection {
    Outgoing,
    Incoming,
}

/// Query configuration for neighbor lookups with optional filtering.
#[derive(Clone, Debug)]
pub struct NeighborQuery {
    pub direction: BackendDirection,
    pub edge_type: Option<String>,
}

impl Default for NeighborQuery {
    fn default() -> Self {
        Self {
            direction: BackendDirection::Outgoing,
            edge_type: None,
        }
    }
}

/// Node specification for insertion operations.
#[derive(Clone, Debug)]
pub struct NodeSpec {
    pub kind: String,
    pub name: String,
    pub file_path: Option<String>,
    pub data: serde_json::Value,
}

/// Edge specification for insertion operations.
#[derive(Clone, Debug)]
pub struct EdgeSpec {
    pub from: i64,
    pub to: i64,
    pub edge_type: String,
    pub data: serde_json::Value,
}
