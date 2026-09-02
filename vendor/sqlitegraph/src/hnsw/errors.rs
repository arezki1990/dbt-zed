//! HNSW Error Types
//!
//! This module defines all error types specific to HNSW operations.
//! These errors provide detailed information about configuration validation
//! failures, index operation errors, and edge cases during vector search.
//!
//! # Error Categories
//!
//! - **Configuration Errors**: Invalid parameters during HNSW setup
//! - **Validation Errors**: Vector dimension mismatches, invalid operations
//! - **Index Errors**: Runtime errors during index operations
//! - **Storage Errors**: I/O and persistence-related failures
//!
//! # Examples
//!
//! ```rust
//! use sqlitegraph::hnsw::{HnswConfigBuilder, HnswConfigError};
//!
//! // Build with valid dimension, then test the error type directly
//! let result: Result<(), HnswConfigError> = Err(HnswConfigError::InvalidDimension);
//! assert!(matches!(result, Err(HnswConfigError::InvalidDimension)));
//! ```

use std::fmt;

/// HNSW configuration validation errors
///
/// These errors occur during HNSW configuration creation when parameters
/// fall outside valid ranges or violate HNSW algorithm constraints.
///
/// # Error Variants
///
/// * `InvalidDimension` - Vector dimension is zero or exceeds practical limits
/// * `InvalidMParameter` - Number of connections per node is invalid
/// * `InvalidEfConstruction` - Construction ef parameter is too small
/// * `InvalidEfSearch` - Search ef parameter is invalid
/// * `InvalidMaxLayers` - Maximum layer count is invalid
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::errors::HnswConfigError;
///
/// fn handle_error(error: HnswConfigError) {
///     match error {
///         HnswConfigError::InvalidDimension => {
///             println!("Vector dimension must be > 0");
///         }
///         HnswConfigError::InvalidMParameter => {
///             println!("M parameter must be > 0");
///         }
///         _ => {
///             // Handle other error types...
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HnswConfigError {
    /// Vector dimension is zero or invalid
    InvalidDimension,

    /// Number of connections per node (M) is zero or invalid
    InvalidMParameter,

    /// Construction ef parameter is less than M
    InvalidEfConstruction,

    /// Search ef parameter is zero or invalid
    InvalidEfSearch,

    /// Maximum number of layers is zero or invalid
    InvalidMaxLayers,
}

impl fmt::Display for HnswConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HnswConfigError::InvalidDimension => {
                write!(f, "Vector dimension must be greater than 0")
            }
            HnswConfigError::InvalidMParameter => {
                write!(
                    f,
                    "M parameter (connections per node) must be greater than 0"
                )
            }
            HnswConfigError::InvalidEfConstruction => {
                write!(f, "ef_construction must be >= M parameter")
            }
            HnswConfigError::InvalidEfSearch => {
                write!(f, "ef_search parameter must be greater than 0")
            }
            HnswConfigError::InvalidMaxLayers => {
                write!(f, "Maximum number of layers must be greater than 0")
            }
        }
    }
}

impl std::error::Error for HnswConfigError {}

/// HNSW index operation errors
///
/// These errors occur during HNSW index operations such as insertion,
/// search, and index maintenance.
///
/// # Error Variants
///
/// * `VectorDimensionMismatch` - Vector length doesn't match configured dimension
/// * `DuplicateVectorId` - Attempting to insert a vector with existing ID
/// * `VectorNotFound` - No vector found with specified ID
/// * `IndexNotInitialized` - Operation attempted on uninitialized index
/// * `IndexCorrupted` - Index structure is corrupted or invalid
/// * `ZeroMagnitudeVector` - Vector has zero magnitude or non-finite values;
///   cosine similarity is undefined
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::errors::HnswIndexError;
///
/// fn handle_error(error: HnswIndexError) {
///     match error {
///         HnswIndexError::VectorDimensionMismatch { expected, actual } => {
///             println!("Expected {} dimensions, got {}", expected, actual);
///         }
///         HnswIndexError::DuplicateVectorId(id) => {
///             println!("Vector ID {} already exists", id);
///         }
///         _ => {
///             // Handle other error types...
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum HnswIndexError {
    /// Vector dimension doesn't match configured dimension
    VectorDimensionMismatch {
        /// Expected dimension count
        expected: usize,
        /// Actual vector dimension count
        actual: usize,
    },

    /// Attempt to insert duplicate vector ID
    DuplicateVectorId(u64),

    /// Vector ID not found in index
    VectorNotFound(u64),

    /// Index operation attempted on uninitialized index
    IndexNotInitialized,

    /// Index structure corruption detected
    IndexCorrupted(String),

    /// Index capacity exceeded
    CapacityExceeded,

    /// Invalid search parameters
    InvalidSearchParameters,

    /// Node not found in layer
    NodeNotFound(u64),

    /// Invalid node ID (non-sequential or out of range)
    InvalidNodeId(u64),

    /// Attempt to connect node to itself
    SelfConnection(u64),

    /// Vector has zero magnitude or contains non-finite values (NaN/Inf)
    ///
    /// Cosine similarity is undefined for such vectors. The insert boundary
    /// rejects them with this typed error instead of letting them reach the
    /// SIMD distance kernel, which panics ("First vector has zero magnitude",
    /// see `simd.rs::cosine_similarity` `# Panics`). A panic is not catchable
    /// by caller `Result` handling, so one degenerate vector would abort a
    /// whole batch; returning `Err` lets callers skip the offending vector.
    ZeroMagnitudeVector,
}

/// Vector storage-related errors
#[derive(Debug, Clone, PartialEq)]
pub enum HnswStorageError {
    /// Invalid vector dimension (zero or too large)
    InvalidDimension(usize),

    /// Vector dimension mismatch between data and claimed dimension
    DimensionMismatch { expected: usize, actual: usize },

    /// Vector data contains invalid values (NaN, Inf, etc.)
    InvalidVectorData,

    /// Vector ID not found in storage
    VectorNotFound(u64),

    /// Batch operation size mismatch
    BatchSizeMismatch,

    /// Storage backend not supported
    BackendNotSupported,

    /// Vector size exceeds maximum limits
    VectorTooLarge { size: usize, max_size: usize },

    /// Storage capacity exceeded
    StorageCapacityExceeded,

    /// I/O error during storage operation
    IoError(String),

    /// Database error during persistence operation
    DatabaseError(String),
}

impl fmt::Display for HnswStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HnswStorageError::InvalidDimension(dim) => {
                write!(f, "Invalid vector dimension: {}", dim)
            }
            HnswStorageError::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "Vector dimension mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            HnswStorageError::InvalidVectorData => {
                write!(f, "Vector data contains invalid values (NaN, Inf, etc.)")
            }
            HnswStorageError::VectorNotFound(id) => {
                write!(f, "Vector ID {} not found in storage", id)
            }
            HnswStorageError::BatchSizeMismatch => {
                write!(f, "Batch operation size mismatch")
            }
            HnswStorageError::BackendNotSupported => {
                write!(f, "Storage backend not supported")
            }
            HnswStorageError::VectorTooLarge { size, max_size } => {
                write!(
                    f,
                    "Vector size {} exceeds maximum allowed size {}",
                    size, max_size
                )
            }
            HnswStorageError::StorageCapacityExceeded => {
                write!(f, "Storage capacity exceeded")
            }
            HnswStorageError::IoError(msg) => {
                write!(f, "I/O error: {}", msg)
            }
            HnswStorageError::DatabaseError(msg) => {
                write!(f, "Database error: {}", msg)
            }
        }
    }
}

impl std::error::Error for HnswStorageError {}

impl fmt::Display for HnswIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HnswIndexError::VectorDimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "Vector dimension mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            HnswIndexError::DuplicateVectorId(id) => {
                write!(f, "Vector ID {} already exists in index", id)
            }
            HnswIndexError::VectorNotFound(id) => {
                write!(f, "Vector ID {} not found in index", id)
            }
            HnswIndexError::IndexNotInitialized => {
                write!(f, "Index not initialized")
            }
            HnswIndexError::IndexCorrupted(msg) => {
                write!(f, "Index corrupted: {}", msg)
            }
            HnswIndexError::CapacityExceeded => {
                write!(f, "Index capacity exceeded")
            }
            HnswIndexError::InvalidSearchParameters => {
                write!(f, "Invalid search parameters")
            }
            HnswIndexError::NodeNotFound(id) => {
                write!(f, "Node {} not found in layer", id)
            }
            HnswIndexError::InvalidNodeId(id) => {
                write!(f, "Invalid node ID: {}", id)
            }
            HnswIndexError::SelfConnection(id) => {
                write!(f, "Attempt to connect node {} to itself", id)
            }
            HnswIndexError::ZeroMagnitudeVector => {
                write!(
                    f,
                    "Vector has zero magnitude or non-finite values; cosine similarity is undefined"
                )
            }
        }
    }
}

impl std::error::Error for HnswIndexError {}

/// Multi-layer HNSW specific errors
///
/// These errors occur during multi-layer HNSW operations such as layer mapping,
/// level distribution, and cross-layer coordination.
///
/// # Error Variants
///
/// * `LayerMappingConflict` - Conflict in layer ID assignment
/// * `InconsistentMapping` - Bidirectional mapping inconsistency
/// * `InconsistentLayerState` - Layer state corruption detected
/// * `LayerMemoryExceeded` - Memory limit exceeded for layer
/// * `CrossLayerSearchFailed` - Cross-layer search operation failed
/// * `LevelDistributionFailure` - Level distribution algorithm failed
#[derive(Debug, Clone, PartialEq)]
pub enum HnswMultiLayerError {
    /// Conflict in layer ID mapping
    LayerMappingConflict {
        /// Global vector ID
        global_id: u64,
        /// Layer ID where conflict occurred
        layer_id: usize,
        /// Assigned local ID
        local_id: u64,
        /// Expected local ID
        expected: u64,
    },

    /// Inconsistent bidirectional mapping
    InconsistentMapping {
        /// Global vector ID
        global_id: u64,
        /// Layer ID where inconsistency detected
        layer_id: usize,
        /// Local ID in mapping
        local_id: u64,
        /// Mapped global ID that differs
        mapped_global: u64,
    },

    /// Inconsistent layer state
    InconsistentLayerState {
        /// Layer ID with inconsistent state
        layer_id: usize,
        /// Expected number of nodes
        expected_nodes: usize,
        /// Actual number of nodes
        actual_nodes: usize,
    },

    /// Layer memory limit exceeded
    LayerMemoryExceeded {
        /// Layer index
        layer: usize,
        /// Required memory in bytes
        required: usize,
        /// Available memory in bytes
        available: usize,
    },

    /// Cross-layer search failure
    CrossLayerSearchFailed {
        /// Source layer
        from_layer: usize,
        /// Target layer
        to_layer: usize,
    },

    /// Level distribution failure
    LevelDistributionFailure {
        /// Number of attempts made
        attempts: usize,
        /// Maximum level attempted
        max_level: usize,
    },
}

impl fmt::Display for HnswMultiLayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HnswMultiLayerError::LayerMappingConflict {
                global_id,
                layer_id,
                local_id,
                expected,
            } => {
                write!(
                    f,
                    "Layer mapping conflict: global ID {} in layer {} assigned local ID {}, expected {}",
                    global_id, layer_id, local_id, expected
                )
            }
            HnswMultiLayerError::InconsistentMapping {
                global_id,
                layer_id,
                local_id,
                mapped_global,
            } => {
                write!(
                    f,
                    "Inconsistent mapping: global ID {} → layer {} → local ID {}, but local {} → global ID {}",
                    global_id, layer_id, local_id, local_id, mapped_global
                )
            }
            HnswMultiLayerError::InconsistentLayerState {
                layer_id,
                expected_nodes,
                actual_nodes,
            } => {
                write!(
                    f,
                    "Inconsistent layer state: layer {} expects {} nodes but has {}",
                    layer_id, expected_nodes, actual_nodes
                )
            }
            HnswMultiLayerError::LayerMemoryExceeded {
                layer,
                required,
                available,
            } => {
                write!(
                    f,
                    "Layer {} memory limit exceeded: required {} bytes, available {} bytes",
                    layer, required, available
                )
            }
            HnswMultiLayerError::CrossLayerSearchFailed {
                from_layer,
                to_layer,
            } => {
                write!(
                    f,
                    "Cross-layer search failed: from layer {} to layer {}",
                    from_layer, to_layer
                )
            }
            HnswMultiLayerError::LevelDistributionFailure {
                attempts,
                max_level,
            } => {
                write!(
                    f,
                    "Level distribution failed after {} attempts, max level {}",
                    attempts, max_level
                )
            }
        }
    }
}

impl std::error::Error for HnswMultiLayerError {}

/// Combined HNSW error type
///
/// This type encompasses all possible HNSW-related errors for convenience
/// when handling errors from HNSW operations.
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::hnsw::errors::HnswError;
///
/// fn handle_hnsw_result(result: Result<(), HnswError>) {
///     match result {
///         Ok(()) => println!("Operation successful"),
///         Err(e) => println!("HNSW error: {}", e),
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum HnswError {
    /// Configuration-related errors
    Config(HnswConfigError),
    /// Index operation errors
    Index(HnswIndexError),

    /// Storage operation errors
    Storage(HnswStorageError),

    /// Multi-layer operation errors
    MultiLayer(HnswMultiLayerError),
}

impl fmt::Display for HnswError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HnswError::Config(err) => write!(f, "Configuration error: {}", err),
            HnswError::Index(err) => write!(f, "Index error: {}", err),
            HnswError::Storage(err) => write!(f, "Storage error: {}", err),
            HnswError::MultiLayer(err) => write!(f, "Multi-layer error: {}", err),
        }
    }
}

impl std::error::Error for HnswError {}

impl From<HnswConfigError> for HnswError {
    fn from(err: HnswConfigError) -> Self {
        HnswError::Config(err)
    }
}

impl From<HnswIndexError> for HnswError {
    fn from(err: HnswIndexError) -> Self {
        HnswError::Index(err)
    }
}

impl From<HnswStorageError> for HnswError {
    fn from(err: HnswStorageError) -> Self {
        HnswError::Storage(err)
    }
}

impl From<HnswMultiLayerError> for HnswError {
    fn from(err: HnswMultiLayerError) -> Self {
        HnswError::MultiLayer(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_display() {
        assert_eq!(
            HnswConfigError::InvalidDimension.to_string(),
            "Vector dimension must be greater than 0"
        );
        assert_eq!(
            HnswConfigError::InvalidMParameter.to_string(),
            "M parameter (connections per node) must be greater than 0"
        );
        assert_eq!(
            HnswConfigError::InvalidEfConstruction.to_string(),
            "ef_construction must be >= M parameter"
        );
        assert_eq!(
            HnswConfigError::InvalidEfSearch.to_string(),
            "ef_search parameter must be greater than 0"
        );
        assert_eq!(
            HnswConfigError::InvalidMaxLayers.to_string(),
            "Maximum number of layers must be greater than 0"
        );
    }

    #[test]
    fn test_index_error_display() {
        let dim_error = HnswIndexError::VectorDimensionMismatch {
            expected: 768,
            actual: 512,
        };
        assert_eq!(
            dim_error.to_string(),
            "Vector dimension mismatch: expected 768, got 512"
        );

        let dup_error = HnswIndexError::DuplicateVectorId(42);
        assert_eq!(
            dup_error.to_string(),
            "Vector ID 42 already exists in index"
        );

        let not_found = HnswIndexError::VectorNotFound(99);
        assert_eq!(not_found.to_string(), "Vector ID 99 not found in index");

        assert_eq!(
            HnswIndexError::IndexNotInitialized.to_string(),
            "Index not initialized"
        );

        let corrupted = HnswIndexError::IndexCorrupted("layer data corrupted".to_string());
        assert_eq!(
            corrupted.to_string(),
            "Index corrupted: layer data corrupted"
        );

        assert_eq!(
            HnswIndexError::CapacityExceeded.to_string(),
            "Index capacity exceeded"
        );

        assert_eq!(
            HnswIndexError::InvalidSearchParameters.to_string(),
            "Invalid search parameters"
        );
    }

    #[test]
    fn test_hnsw_error_display() {
        let config_err = HnswError::Config(HnswConfigError::InvalidDimension);
        assert!(config_err.to_string().contains("Configuration error"));
        assert!(
            config_err
                .to_string()
                .contains("Vector dimension must be greater than 0")
        );

        let index_err = HnswError::Index(HnswIndexError::VectorNotFound(1));
        assert!(index_err.to_string().contains("Index error"));
        assert!(index_err.to_string().contains("Vector ID 1 not found"));
    }

    #[test]
    fn test_error_conversions() {
        let config_err = HnswConfigError::InvalidMParameter;
        let hnsw_err: HnswError = config_err.into();
        assert!(matches!(
            hnsw_err,
            HnswError::Config(HnswConfigError::InvalidMParameter)
        ));

        let index_err = HnswIndexError::DuplicateVectorId(123);
        let hnsw_err: HnswError = index_err.into();
        assert!(matches!(
            hnsw_err,
            HnswError::Index(HnswIndexError::DuplicateVectorId(123))
        ));
    }

    #[test]
    fn test_error_equality() {
        assert_eq!(
            HnswConfigError::InvalidDimension,
            HnswConfigError::InvalidDimension
        );
        assert_ne!(
            HnswConfigError::InvalidDimension,
            HnswConfigError::InvalidMParameter
        );

        let dim_error1 = HnswIndexError::VectorDimensionMismatch {
            expected: 256,
            actual: 128,
        };
        let dim_error2 = HnswIndexError::VectorDimensionMismatch {
            expected: 256,
            actual: 128,
        };
        assert_eq!(dim_error1, dim_error2);

        assert_ne!(
            HnswIndexError::DuplicateVectorId(1),
            HnswIndexError::DuplicateVectorId(2)
        );
    }

    #[test]
    fn test_error_debug_format() {
        let config_err = HnswConfigError::InvalidEfConstruction;
        assert_eq!(format!("{:?}", config_err), "InvalidEfConstruction");

        let index_err = HnswIndexError::VectorDimensionMismatch {
            expected: 768,
            actual: 384,
        };
        let debug_str = format!("{:?}", index_err);
        assert!(debug_str.contains("VectorDimensionMismatch"));
        assert!(debug_str.contains("768"));
        assert!(debug_str.contains("384"));
    }
}
