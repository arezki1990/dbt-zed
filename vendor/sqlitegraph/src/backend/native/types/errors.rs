//! Comprehensive error type for native backend operations

use super::{NativeEdgeId, NativeNodeId};

/// Comprehensive error type for native backend operations
#[derive(Debug, thiserror::Error)]
pub enum NativeBackendError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid magic number: expected {expected:08x}, found {found:08x}")]
    InvalidMagic { expected: u64, found: u64 },

    #[error("Unsupported version: {version} (supported: {supported_version})")]
    UnsupportedVersion {
        version: u32,
        supported_version: u32,
    },

    #[error("Invalid header field '{field}': {reason}")]
    InvalidHeader { field: String, reason: String },

    #[error("Invalid header checksum: expected {expected:08x}, found {found:08x}")]
    InvalidChecksum { expected: u64, found: u64 },

    #[error("Node ID {id} out of bounds (valid range: 1-{max_id})")]
    InvalidNodeId {
        id: NativeNodeId,
        max_id: NativeNodeId,
    },

    #[error("Edge ID {id} out of bounds (valid range: 1-{max_id})")]
    InvalidEdgeId {
        id: NativeEdgeId,
        max_id: NativeEdgeId,
    },

    #[error("Corrupt node record at node {node_id}: {reason}")]
    CorruptNodeRecord {
        node_id: NativeNodeId,
        reason: String,
    },

    #[error("Corrupt edge record at edge {edge_id}: {reason}")]
    CorruptEdgeRecord {
        edge_id: NativeEdgeId,
        reason: String,
    },

    #[error(
        "Inconsistent adjacency for node {node_id}: {count} {direction} edges in metadata but file indicates {file_count}"
    )]
    InconsistentAdjacency {
        node_id: NativeNodeId,
        count: u32,
        direction: String,
        file_count: u32,
    },

    #[error("File too small: {size} bytes (minimum {min_size} bytes required)")]
    FileTooSmall { size: u64, min_size: u64 },

    #[error("Record too large: {size} bytes (maximum {max_size} bytes)")]
    RecordTooLarge { size: u32, max_size: u32 },

    #[error("UTF-8 encoding error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Binary serialization error: {0}")]
    BincodeError(#[from] Box<bincode::ErrorKind>),

    #[error("Invalid UTF-8 string: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    #[error("Buffer too small: {size} bytes (need at least {min_size} bytes)")]
    BufferTooSmall { size: usize, min_size: usize },

    #[error("Invalid string table offset: {offset}")]
    InvalidStringOffset { offset: u32 },

    #[error("Corrupt string table: {reason}")]
    CorruptStringTable { reason: String },

    #[error("Invalid magic bytes: found {found:?}")]
    InvalidMagicBytes { found: [u8; 8] },

    #[error("Invalid operation: {context}")]
    InvalidOperation { context: String },

    #[error("Validation failed for metric '{metric}': expected {expected}, found {actual}")]
    ValidationFailed {
        metric: String,
        expected: f64,
        actual: f64,
    },

    #[error("Out of space in file")]
    OutOfSpace,

    #[error("Corrupt free space: {reason}")]
    CorruptFreeSpace { reason: String },

    #[error("Transaction rolled back: {0}")]
    TransactionRolledBack(String),

    #[error("Node {node_id} not found during {operation}")]
    NodeNotFound {
        node_id: NativeNodeId,
        operation: String,
    },

    #[error("Invalid parameter: {context}")]
    InvalidParameter {
        context: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Invalid state: {context}")]
    InvalidState {
        context: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Corruption detected: {context}")]
    CorruptionDetected {
        context: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Invalid configuration for {parameter}: {reason}")]
    InvalidConfiguration { parameter: String, reason: String },

    #[error("Version mismatch: expected {expected}, found {found}")]
    VersionMismatch {
        expected: String,
        found: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Invalid transaction {tx_id}: {reason}")]
    InvalidTransaction { tx_id: u64, reason: String },

    #[error("I/O error during {context}: {source}")]
    IoError {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Recovery error: {0}")]
    Recovery(String),

    #[error("Node {node_id} already exists")]
    NodeExists { node_id: NativeNodeId },

    #[error("Lock error: {context}")]
    LockError { context: String },

    #[error("Edge {edge_id} already exists")]
    EdgeExists { edge_id: NativeEdgeId },

    #[error("Edge {edge_id} not found")]
    EdgeNotFound { edge_id: NativeEdgeId },

    #[error("Transaction {tx_id} not found")]
    TransactionNotFound { tx_id: u64 },

    #[error("Savepoint {savepoint_id} not found")]
    SavepointNotFound { savepoint_id: String },

    #[error("Deadlock detected involving transaction {tx_id}")]
    DeadlockDetected {
        tx_id: u64,
        conflicting_resources: Vec<i64>,
    },

    #[error("Invalid transaction state: {state}")]
    InvalidTransactionState { tx_id: u64, state: String },

    #[error("Migration failed: {0}")]
    MigrationFailed(String),

    #[error(
        "Transaction ID exhaustion: current ID {current_id}, {remaining} IDs remaining before wraparound"
    )]
    TransactionIdExhaustion { current_id: u64, remaining: u64 },

    #[error("WAL contiguity violation: {0}")]
    WalContiguityViolation(String),

    #[error("Serialization error: {context}")]
    SerializationError { context: String },

    #[error("Deserialization error: {context}")]
    DeserializationError { context: String },
}

// Add conversion from SystemTimeError
impl From<std::time::SystemTimeError> for NativeBackendError {
    fn from(error: std::time::SystemTimeError) -> Self {
        Self::InvalidHeader {
            field: "system_time".to_string(),
            reason: format!("System time error: {}", error),
        }
    }
}

// Conversion impls for CheckpointError and RecoveryError removed
// along with the deprecated V2 WAL backend.
