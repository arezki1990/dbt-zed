//! Delta encoding for node IDs
//!
//! Delta encoding stores the difference between sequential node IDs
//! rather than the full ID value. This provides significant space savings
//! when nodes are stored in sorted order (which is the common case).
//!
//! # Formula
//!
//! ```text
//! id_delta = (node.id - base_id) as u32
//! reconstructed_id = base_id + id_delta as i64
//! ```
//!
//! # Saturating Arithmetic
//!
//! We use saturating subtraction to prevent underflow when `node.id < base_id`:
//! ```text
//! id_delta = node.id.saturating_sub(base_id) as u32
//! ```
//!
//! When decoding, if the delta would exceed u32::MAX, we saturate at i64::MAX.

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;

/// Maximum delta value that can be encoded (u32::MAX)
pub const MAX_DELTA: u32 = u32::MAX;

/// Maximum difference between node ID and base ID that can be encoded
pub const MAX_ID_DIFFERENCE: i64 = MAX_DELTA as i64;

/// Error type for delta decoding failures
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaDecodeError {
    /// Delta value exceeds maximum representable difference
    DeltaOverflow { delta: u32, base_id: i64 },

    /// Invalid delta (should never happen with valid u32)
    InvalidDelta { delta: u32 },
}

impl std::fmt::Display for DeltaDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeltaOverflow { delta, base_id } => write!(
                f,
                "Delta overflow: delta {} would overflow when added to base_id {}",
                delta, base_id
            ),
            Self::InvalidDelta { delta } => {
                write!(f, "Invalid delta value: {}", delta)
            }
        }
    }
}

impl std::error::Error for DeltaDecodeError {}

/// Encode a node ID as a delta from a base ID
///
/// # Arguments
/// * `node_id` - The node ID to encode
/// * `base_id` - The base ID to compute the delta from
///
/// # Returns
/// The encoded delta value (u32)
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::delta::encode_id_delta;
///
/// // Sequential IDs: small delta
/// assert_eq!(encode_id_delta(105, 100), 5);
///
/// // Non-sequential IDs: larger delta
/// assert_eq!(encode_id_delta(1000, 100), 900);
///
/// // Same ID: zero delta
/// assert_eq!(encode_id_delta(100, 100), 0);
/// ```
///
/// # Edge Cases
/// - When `node_id < base_id`, returns 0 (saturating subtraction)
/// - When difference exceeds u32::MAX, returns u32::MAX (saturation)
pub fn encode_id_delta(node_id: i64, base_id: i64) -> u32 {
    // Calculate difference with saturating subtraction
    let difference = node_id.saturating_sub(base_id);

    // Clamp to u32 range
    if difference < 0 {
        0
    } else if difference > MAX_ID_DIFFERENCE {
        MAX_DELTA
    } else {
        difference as u32
    }
}

/// Decode a node ID from a delta and base ID
///
/// # Arguments
/// * `delta` - The encoded delta value
/// * `base_id` - The base ID used during encoding
///
/// # Returns
/// The reconstructed node ID
///
/// # Errors
/// Returns `DeltaDecodeError` if the delta would cause an overflow
/// when added to the base ID (should not happen with properly encoded deltas)
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::delta::decode_id_delta;
///
/// // Sequential IDs
/// assert_eq!(decode_id_delta(5, 100).unwrap(), 105);
///
/// // Large delta
/// assert_eq!(decode_id_delta(1000, 100).unwrap(), 1100);
///
/// // Zero delta (same as base)
/// assert_eq!(decode_id_delta(0, 100).unwrap(), 100);
/// ```
pub fn decode_id_delta(delta: u32, base_id: i64) -> NativeResult<i64> {
    // Convert delta to i64 and add to base_id
    let delta_i64 = delta as i64;

    // Check for overflow before addition
    match base_id.checked_add(delta_i64) {
        Some(reconstructed_id) => Ok(reconstructed_id),
        None => Err(NativeBackendError::InvalidHeader {
            field: "id_delta".to_string(),
            reason: format!(
                "Delta overflow: delta {} would overflow when added to base_id {}",
                delta, base_id
            ),
        }),
    }
}

/// Calculate the optimal base ID for a batch of node IDs
///
/// The optimal base ID is the minimum ID in the batch, which minimizes
/// the delta values and maximizes compression efficiency.
///
/// # Arguments
/// * `node_ids` - A slice of node IDs
///
/// # Returns
/// The optimal base ID (minimum of the input IDs), or 0 if the slice is empty
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::delta::calculate_optimal_base_id;
///
/// let ids = vec![100, 105, 110, 115];
/// assert_eq!(calculate_optimal_base_id(&ids), 100);
///
/// let ids = vec![1000, 1001, 1002];
/// assert_eq!(calculate_optimal_base_id(&ids), 1000);
/// ```
pub fn calculate_optimal_base_id(node_ids: &[i64]) -> i64 {
    if node_ids.is_empty() {
        return 0;
    }

    *node_ids.iter().min().unwrap_or(&0)
}

/// Estimate space savings from delta encoding
///
/// # Arguments
/// * `node_count` - Number of nodes to encode
/// * `avg_delta` - Average delta value (affects varint size)
///
/// # Returns
/// Estimated bytes saved compared to full i64 encoding
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::delta::estimate_delta_savings;
///
/// // Sequential nodes: 4 bytes saved per node (i64 -> u32)
/// let sequential_savings = estimate_delta_savings(1000, 1);
/// assert_eq!(sequential_savings, 4000);
/// ```
pub fn estimate_delta_savings(node_count: usize, _avg_delta: u32) -> usize {
    // Full i64 encoding: 8 bytes per node
    let full_size = node_count * 8;

    // Delta encoding: u32 (4 bytes) per node
    // (In practice, varint could save more for small deltas)
    let delta_size = node_count * 4;

    full_size.saturating_sub(delta_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_id_delta_sequential() {
        // Sequential IDs: small delta
        assert_eq!(encode_id_delta(105, 100), 5);
        assert_eq!(encode_id_delta(110, 100), 10);
        assert_eq!(encode_id_delta(1000, 100), 900);
    }

    #[test]
    fn test_encode_id_delta_same_id() {
        // Same ID: zero delta
        assert_eq!(encode_id_delta(100, 100), 0);
        assert_eq!(encode_id_delta(0, 0), 0);
        assert_eq!(encode_id_delta(i64::MAX, i64::MAX), 0);
    }

    #[test]
    fn test_encode_id_delta_node_before_base() {
        // node_id < base_id: saturating subtraction returns 0
        assert_eq!(encode_id_delta(95, 100), 0);
        assert_eq!(encode_id_delta(-100, 0), 0);
        assert_eq!(encode_id_delta(i64::MIN, i64::MAX), 0);
    }

    #[test]
    fn test_encode_id_delta_max_difference() {
        // Maximum representable difference
        let base_id = 1000i64;
        let max_node_id = base_id + MAX_ID_DIFFERENCE;
        assert_eq!(encode_id_delta(max_node_id, base_id), MAX_DELTA);

        // Beyond maximum: saturates at MAX_DELTA
        let beyond_max = max_node_id + 1;
        assert_eq!(encode_id_delta(beyond_max, base_id), MAX_DELTA);
    }

    #[test]
    fn test_encode_id_delta_negative_ids() {
        // Negative node IDs
        assert_eq!(encode_id_delta(-95, -100), 5);
        assert_eq!(encode_id_delta(-90, -100), 10);
        assert_eq!(encode_id_delta(-100, -100), 0);
        assert_eq!(encode_id_delta(-105, -100), 0); // node < base
    }

    #[test]
    fn test_decode_id_delta_sequential() {
        assert_eq!(decode_id_delta(5, 100).unwrap(), 105);
        assert_eq!(decode_id_delta(10, 100).unwrap(), 110);
        assert_eq!(decode_id_delta(900, 100).unwrap(), 1000);
    }

    #[test]
    fn test_decode_id_delta_zero() {
        assert_eq!(decode_id_delta(0, 100).unwrap(), 100);
        assert_eq!(decode_id_delta(0, 0).unwrap(), 0);
        assert_eq!(decode_id_delta(0, -100).unwrap(), -100);
    }

    #[test]
    fn test_decode_id_delta_max_delta() {
        assert_eq!(decode_id_delta(MAX_DELTA, 0).unwrap(), MAX_DELTA as i64);
        assert_eq!(
            decode_id_delta(MAX_DELTA, 1000).unwrap(),
            1000 + MAX_DELTA as i64
        );
    }

    #[test]
    fn test_decode_id_delta_negative_base() {
        assert_eq!(decode_id_delta(5, -100).unwrap(), -95);
        assert_eq!(decode_id_delta(10, -100).unwrap(), -90);
        assert_eq!(decode_id_delta(100, -100).unwrap(), 0);
    }

    #[test]
    fn test_decode_id_delta_overflow_edge() {
        // This should not overflow (i64::MAX - u32::MAX is still valid)
        let base_id = i64::MAX - 1000;
        let delta = 500;
        assert_eq!(decode_id_delta(delta, base_id).unwrap(), i64::MAX - 500);

        // At the edge: delta = u32::MAX, base near i64::MAX
        // i64::MAX - u32::MAX = 9223372032559808512
        let base_at_edge = i64::MAX - MAX_DELTA as i64;
        assert_eq!(decode_id_delta(MAX_DELTA, base_at_edge).unwrap(), i64::MAX);
    }

    #[test]
    fn test_decode_encode_roundtrip() {
        let test_cases = vec![
            (100, 100, 0),
            (105, 100, 5),
            (1000, 100, 900),
            (0, 0, 0),
            (-95, -100, 5),
            (i64::MAX, i64::MAX, 0),
        ];

        for (node_id, base_id, expected_delta) in test_cases {
            let delta = encode_id_delta(node_id, base_id);
            assert_eq!(delta, expected_delta);

            let reconstructed = decode_id_delta(delta, base_id).unwrap();
            // For cases where node < base, reconstruction won't match
            if node_id >= base_id && (node_id - base_id) <= MAX_ID_DIFFERENCE {
                assert_eq!(reconstructed, node_id);
            }
        }
    }

    #[test]
    fn test_calculate_optimal_base_id() {
        let ids = vec![100, 105, 110, 115];
        assert_eq!(calculate_optimal_base_id(&ids), 100);

        let ids = vec![1000, 1001, 1002];
        assert_eq!(calculate_optimal_base_id(&ids), 1000);

        let ids = vec![-100, -50, 0, 50, 100];
        assert_eq!(calculate_optimal_base_id(&ids), -100);

        let ids = vec![42];
        assert_eq!(calculate_optimal_base_id(&ids), 42);

        let ids: Vec<i64> = vec![];
        assert_eq!(calculate_optimal_base_id(&ids), 0);
    }

    #[test]
    fn test_estimate_delta_savings() {
        // Sequential nodes: 4 bytes saved per node (i64 -> u32)
        let sequential_savings = estimate_delta_savings(1000, 1);
        assert_eq!(sequential_savings, 4000);

        // Large deltas: still save 4 bytes per node
        let large_delta_savings = estimate_delta_savings(1000, 10000);
        assert_eq!(large_delta_savings, 4000);

        // Zero nodes: zero savings
        assert_eq!(estimate_delta_savings(0, 0), 0);
    }

    #[test]
    fn test_delta_with_extreme_values() {
        // Test with i64::MIN and i64::MAX
        let min_id = i64::MIN;
        let max_id = i64::MAX;

        // Encoding from MIN
        assert_eq!(encode_id_delta(min_id, min_id), 0);
        assert_eq!(encode_id_delta(min_id + 100, min_id), 100);

        // Encoding to MAX
        assert_eq!(encode_id_delta(max_id, max_id), 0);

        // Large base near MAX
        let base_near_max = max_id - 1000;
        assert_eq!(encode_id_delta(max_id, base_near_max), 1000);
        assert_eq!(encode_id_delta(max_id, base_near_max + 500), 500);
    }
}
