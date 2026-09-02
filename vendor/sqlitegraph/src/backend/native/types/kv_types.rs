//! Key-Value store types (production standard)
//!
//! These types are used across all backends (SQLite, V3) for KV operations.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Value types supported by the KV store
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KvValue {
    /// Null value
    Null,
    /// Raw bytes
    Bytes(Vec<u8>),
    /// UTF-8 string
    String(String),
    /// 64-bit signed integer
    Integer(i64),
    /// 64-bit floating point
    Float(f64),
    /// Boolean
    Boolean(bool),
    /// JSON value
    Json(serde_json::Value),
}

/// Metadata for a KV entry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvMetadata {
    pub created_at: u64,
    pub updated_at: u64,
    pub ttl_seconds: Option<u64>,
    pub version: u64,
}

/// A key-value entry with metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvEntry {
    pub key: Vec<u8>,
    pub value: KvValue,
    pub metadata: KvMetadata,
}

/// KV store errors
#[derive(Debug, Error, Clone, PartialEq)]
pub enum KvStoreError {
    #[error("Key not found: {0:?}")]
    KeyNotFound(Vec<u8>),

    #[error("Expired key: {0:?}")]
    ExpiredKey(Vec<u8>),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    #[error("Invalid TTL: {0}")]
    InvalidTtl(String),

    #[error("KV recovery failed: {0}")]
    RecoveryFailed(String),
}
