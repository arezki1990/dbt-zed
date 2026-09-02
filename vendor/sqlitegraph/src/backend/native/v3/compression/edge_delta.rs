//! Delta encoding for edge ID compression
//!
//! Compresses sequences of edge IDs using delta encoding, where each ID
//! is stored as the difference from the previous ID. Expected 30-50% space
//! savings for graphs with sequentially assigned edge IDs.
//!
//! # Zigzag Encoding
//!
//! Since deltas can be negative (when IDs decrease), we use zigzag encoding
//! to map signed integers to unsigned for varint compression:
//! ```text
//! zigzag(n) = (n << 1) ^ (n >> 63)  for i64
//! ```
//!
//! This maps:
//! - 0 -> 0
//! - -1 -> 1
//! - 1 -> 2
//! - -2 -> 3
//! - 2 -> 4
//!
//! Small deltas (positive or negative) encode efficiently in 1 byte.

use crate::backend::native::v3::compression::varint::{decode_varint, encode_varint};

/// Encode a signed i64 value using zigzag encoding
///
/// Maps signed integers to unsigned, preserving small magnitude values
/// (both positive and negative) as small unsigned values.
///
/// # Formula
///
/// ```text
/// zigzag(n) = (n << 1) ^ (n >> 63)
/// ```
///
/// # Examples
///
/// ```ignore
/// use sqlitegraph::backend::native::v3::compression::edge_delta::zigzag_encode;
///
/// assert_eq!(zigzag_encode(0), 0);
/// assert_eq!(zigzag_encode(-1), 1);
/// assert_eq!(zigzag_encode(1), 2);
/// assert_eq!(zigzag_encode(-2), 3);
/// assert_eq!(zigzag_encode(2), 4);
/// ```
fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Decode a zigzag-encoded value back to signed i64
///
/// # Formula
///
/// ```text
/// unzigzag(n) = (n >> 1) ^ -(n & 1)
/// ```
///
/// # Examples
///
/// ```ignore
/// use sqlitegraph::backend::native::v3::compression::edge_delta::zigzag_decode;
///
/// assert_eq!(zigzag_decode(0), 0);
/// assert_eq!(zigzag_decode(1), -1);
/// assert_eq!(zigzag_decode(2), 1);
/// assert_eq!(zigzag_decode(3), -2);
/// assert_eq!(zigzag_decode(4), 2);
/// ```
fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Compresses a slice of edge IDs using delta encoding
///
/// # Arguments
///
/// * `edge_ids` - Slice of edge IDs to compress
///
/// # Returns
///
/// Vec<u8> containing compressed delta-encoded varints
///
/// # Performance
///
/// - Expected 30-50% space savings for sequential IDs
/// - Overhead: ~1 byte per edge ID
///
/// # Example
///
/// ```
/// use sqlitegraph::backend::native::v3::compression::edge_delta::compress_edge_ids;
///
/// let ids = vec![1, 2, 3, 5, 8];
/// let compressed = compress_edge_ids(&ids);
/// assert!(compressed.len() < ids.len() * 8);
/// ```
pub fn compress_edge_ids(edge_ids: &[i64]) -> Vec<u8> {
    if edge_ids.is_empty() {
        return Vec::new();
    }

    let mut compressed = Vec::new();
    let mut prev_id = 0i64;

    for &edge_id in edge_ids {
        let delta = edge_id - prev_id;
        let zigzag = zigzag_encode(delta);
        let encoded = encode_varint(zigzag);
        compressed.extend_from_slice(&encoded);
        prev_id = edge_id;
    }

    compressed
}

/// Decompresses delta-encoded edge IDs
///
/// # Arguments
///
/// * `compressed` - Compressed delta-encoded varint data
/// * `count` - Number of edge IDs to decompress
///
/// # Returns
///
/// Vec<i64> containing decompressed edge IDs
///
/// # Errors
///
/// Returns error if data is malformed or insufficient data
///
/// # Example
///
/// ```
/// use sqlitegraph::backend::native::v3::compression::edge_delta::{compress_edge_ids, decompress_edge_ids};
///
/// let original = vec![1, 2, 3, 5, 8];
/// let compressed = compress_edge_ids(&original);
/// let decompressed = decompress_edge_ids(&compressed, original.len()).unwrap();
/// assert_eq!(decompressed, original);
/// ```
pub fn decompress_edge_ids(compressed: &[u8], count: usize) -> Result<Vec<i64>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let mut edge_ids = Vec::with_capacity(count);
    let mut prev_id = 0i64;
    let mut pos = 0;

    for _ in 0..count {
        if pos >= compressed.len() {
            return Err(format!(
                "Insufficient data: expected {} values, but only {} bytes available",
                count,
                compressed.len()
            ));
        }

        match decode_varint(&compressed[pos..]) {
            Ok((zigzag, bytes_read)) => {
                let delta = zigzag_decode(zigzag);
                prev_id += delta;
                edge_ids.push(prev_id);
                pos += bytes_read;
            }
            Err(e) => {
                return Err(format!(
                    "Failed to decode varint at position {}: {:?}",
                    pos, e
                ));
            }
        }
    }

    Ok(edge_ids)
}

/// Calculates the compression ratio achieved
///
/// # Arguments
///
/// * `original` - Original uncompressed data
/// * `compressed` - Compressed data
///
/// # Returns
///
/// Compression ratio as f32 (1.0 = no compression, 0.5 = 50% reduction)
///
/// # Example
///
/// ```
/// use sqlitegraph::backend::native::v3::compression::edge_delta::{compress_edge_ids, compression_ratio};
///
/// let original = vec![1i64; 1000];
/// let compressed = compress_edge_ids(&original);
/// let ratio = compression_ratio(&original, &compressed);
/// assert!(ratio < 1.0); // Should be compressed
/// ```
pub fn compression_ratio(original: &[i64], compressed: &[u8]) -> f32 {
    if original.is_empty() {
        return 1.0;
    }

    let original_size = original.len() * 8; // 8 bytes per i64
    let compressed_size = compressed.len();

    compressed_size as f32 / original_size as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zigzag_encode_decode() {
        let test_cases = vec![
            (0, 0),
            (1, 2),
            (-1, 1),
            (2, 4),
            (-2, 3),
            (100, 200),
            (-100, 199),
        ];

        for (original, encoded) in test_cases {
            assert_eq!(zigzag_encode(original), encoded);
            assert_eq!(zigzag_decode(encoded), original);
        }
    }

    #[test]
    fn test_compress_decompress_sequential_ids() {
        let ids = vec![1, 2, 3, 4, 5];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_compress_decompress_sparse_ids() {
        let ids = vec![1, 5, 10, 100, 1000];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_compress_empty_slice() {
        let ids: Vec<i64> = vec![];
        let compressed = compress_edge_ids(&ids);

        assert!(compressed.is_empty());
    }

    #[test]
    fn test_decompress_empty() {
        let compressed = vec![];
        let result = decompress_edge_ids(&compressed, 0).unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn test_compression_ratio_sequential() {
        let ids: Vec<i64> = (1..=1000).collect();
        let compressed = compress_edge_ids(&ids);
        let ratio = compression_ratio(&ids, &compressed);

        // Should achieve significant compression for sequential IDs
        assert!(
            ratio < 0.5,
            "Compression ratio {} should be < 0.5 for sequential IDs",
            ratio
        );
    }

    #[test]
    fn test_compression_ratio_sparse() {
        let ids: Vec<i64> = (1..=1000).filter(|x| x % 10 == 0).collect();
        let compressed = compress_edge_ids(&ids);
        let ratio = compression_ratio(&ids, &compressed);

        // Should still achieve some compression for sparse sequential IDs
        assert!(ratio < 1.0, "Compression ratio {} should be < 1.0", ratio);
    }

    #[test]
    fn test_large_delta_values() {
        let ids = vec![1, 1000, 1000000, 1000000000];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_negative_deltas() {
        let ids = vec![100, 50, 25, 10]; // Decreasing sequence
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_alternating_sequence() {
        let ids = vec![10, 20, 15, 25, 30, 20]; // Up and down
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_decompress_insufficient_data() {
        let compressed = vec![0x80]; // Incomplete varint (has continuation bit set)
        let result = decompress_edge_ids(&compressed, 1);

        assert!(result.is_err(), "Should fail with insufficient data");
    }

    #[test]
    fn test_single_id() {
        let ids = vec![42];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
        // Delta from 0 to 42 should be 1 byte (zigzag(42) = 84)
        assert_eq!(compressed.len(), 1);
    }

    #[test]
    fn test_zero_start() {
        let ids = vec![0, 1, 2, 3];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_negative_ids() {
        let ids = vec![-100, -50, 0, 50, 100];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_large_negative_deltas() {
        let ids = vec![1000000000, -1000000000, 0, 1000000000];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_compression_efficiency_sequential() {
        // Sequential IDs should compress very well
        let ids: Vec<i64> = (1..=100).collect();
        let compressed = compress_edge_ids(&ids);

        // Each delta is 1, which zigzag-encodes to 2 (1 byte)
        // Original: 100 * 8 = 800 bytes
        // Compressed: ~100 bytes
        assert!(
            compressed.len() < 150,
            "Sequential IDs should compress to < 150 bytes, got {}",
            compressed.len()
        );
    }

    #[test]
    fn test_extreme_values() {
        let ids = vec![i64::MIN, i64::MIN + 1, 0, i64::MAX - 1, i64::MAX];
        let compressed = compress_edge_ids(&ids);
        let decompressed = decompress_edge_ids(&compressed, ids.len()).unwrap();

        assert_eq!(decompressed, ids);
    }
}
