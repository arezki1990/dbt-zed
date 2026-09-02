//! Varint (variable-length integer) encoding
//!
//! Varint encoding stores integers using a variable number of bytes.
//! Each byte uses 7 bits for data and 1 bit (MSB) as a continuation flag.
//!
//! # Encoding Format
//!
//! ```text
//! - For values < 128: 1 byte (MSB = 0)
//! - For values >= 128: Multiple bytes (MSB = 1 for all but last byte)
//!
//! Example:
//!   42     -> [0b00101010]           (1 byte)
//!   300    -> [0b10101100, 0b00000010] (2 bytes)
//!   16384  -> [0b10000000, 0b10000000, 0b00000001] (3 bytes)
//! ```
//!
//! # MSB Prefix Convention
//!
//! The Most Significant Bit (bit 7) indicates continuation:
//! - MSB = 1: More bytes follow
//! - MSB = 0: This is the last byte
//!
//! Lower 7 bits contain the actual data.

use crate::backend::native::NativeBackendError;
use crate::backend::native::NativeResult;

/// Maximum number of bytes needed to encode a u64 as varint
///
/// u64::MAX requires 10 bytes in varint encoding:
/// - 9 full bytes (7 bits each = 63 bits)
/// - 1 partial byte for remaining 1 bit
pub const MAX_VARINT_BYTES: usize = 10;

/// Maximum value that can be encoded in a single varint byte
pub const MAX_SINGLE_BYTE_VALUE: u64 = 0x7F; // 127

/// Mask for extracting 7-bit data from a byte
const DATA_MASK: u8 = 0x7F;

/// Continuation bit (MSB)
const CONTINUATION_BIT: u8 = 0x80;

/// Error type for varint decoding failures
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarintError {
    /// Input buffer is empty
    EmptyInput,

    /// Input ends before varint is complete
    IncompleteData {
        expected_bytes: usize,
        actual_bytes: usize,
    },

    /// Varint exceeds maximum allowed length (10 bytes)
    TooLong { actual_length: usize },

    /// Malformed varint (last byte has continuation bit set)
    Malformed { position: usize },
}

impl std::fmt::Display for VarintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "Cannot decode varint from empty input"),
            Self::IncompleteData {
                expected_bytes,
                actual_bytes,
            } => write!(
                f,
                "Incomplete varint: expected {} bytes, found {}",
                expected_bytes, actual_bytes
            ),
            Self::TooLong { actual_length } => write!(
                f,
                "Varint too long: maximum {} bytes, found {}",
                MAX_VARINT_BYTES, actual_length
            ),
            Self::Malformed { position } => write!(
                f,
                "Malformed varint: continuation bit set at final byte {}",
                position
            ),
        }
    }
}

impl std::error::Error for VarintError {}

/// Encode a u64 value as varint
///
/// # Arguments
/// * `value` - The value to encode
///
/// # Returns
/// A Vec<u8> containing the varint-encoded value
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::varint::encode_varint;
///
/// // Small values fit in one byte
/// assert_eq!(encode_varint(0), vec![0x00]);
/// assert_eq!(encode_varint(42), vec![0x2A]);
/// assert_eq!(encode_varint(127), vec![0x7F]);
///
/// // Larger values use multiple bytes
/// assert_eq!(encode_varint(128), vec![0x80, 0x01]);
/// assert_eq!(encode_varint(300), vec![0xAC, 0x02]);
/// ```
///
/// # Encoding Details
///
/// Each byte (except the last) has the MSB set to indicate continuation.
/// The lower 7 bits of each byte contain actual data.
///
/// ```text
/// Value: 300 (0x12C)
///
/// Byte 0: 0x12C & 0x7F = 0x2C | 0x80 = 0xAC (44 + continuation)
/// Byte 1: (0x12C >> 7) = 0x02 | 0x00 = 0x02 (2, no continuation)
///
/// Result: [0xAC, 0x02]
/// ```
pub fn encode_varint(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0];
    }

    let mut buffer = Vec::with_capacity(MAX_VARINT_BYTES);
    let mut remaining = value;

    while remaining > 0 {
        // Take 7 bits
        let mut byte = (remaining & DATA_MASK as u64) as u8;

        remaining >>= 7;

        // Set continuation bit if more bytes follow
        if remaining > 0 {
            byte |= CONTINUATION_BIT;
        }

        buffer.push(byte);
    }

    buffer
}

/// Decode a varint from a byte slice
///
/// # Arguments
/// * `bytes` - Input byte slice containing varint-encoded data
///
/// # Returns
/// A tuple of (decoded_value, bytes_read)
///
/// # Errors
/// Returns `VarintError` if:
/// - Input is empty
/// - Varint is incomplete
/// - Varint exceeds maximum length
/// - Varint is malformed
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::varint::decode_varint;
///
/// // Single byte values
/// assert_eq!(decode_varint(&[0x00]).unwrap(), (0, 1));
/// assert_eq!(decode_varint(&[0x2A]).unwrap(), (42, 1));
/// assert_eq!(decode_varint(&[0x7F]).unwrap(), (127, 1));
///
/// // Multi-byte values
/// assert_eq!(decode_varint(&[0x80, 0x01]).unwrap(), (128, 2));
/// assert_eq!(decode_varint(&[0xAC, 0x02]).unwrap(), (300, 2));
/// ```
pub fn decode_varint(bytes: &[u8]) -> NativeResult<(u64, usize)> {
    if bytes.is_empty() {
        return Err(NativeBackendError::InvalidHeader {
            field: "varint".to_string(),
            reason: VarintError::EmptyInput.to_string(),
        });
    }

    let mut result: u64 = 0;
    let mut shift: usize = 0;
    let mut bytes_read = 0;

    for (i, &byte) in bytes.iter().enumerate() {
        bytes_read = i + 1;

        // Check for maximum length
        if i >= MAX_VARINT_BYTES {
            return Err(NativeBackendError::InvalidHeader {
                field: "varint".to_string(),
                reason: VarintError::TooLong {
                    actual_length: i + 1,
                }
                .to_string(),
            });
        }

        // Extract 7 bits of data
        let data = (byte & DATA_MASK) as u64;
        result |= data << shift;

        // Check continuation bit
        if byte & CONTINUATION_BIT == 0 {
            // Last byte
            return Ok((result, bytes_read));
        }

        shift += 7;

        // Check for overflow (u64 has 64 bits, we use 7 per byte)
        if shift >= 64 {
            return Err(NativeBackendError::InvalidHeader {
                field: "varint".to_string(),
                reason: format!("Varint overflow: shift {} exceeds u64 capacity", shift),
            });
        }
    }

    // If we get here, input ended before varint was complete
    Err(NativeBackendError::InvalidHeader {
        field: "varint".to_string(),
        reason: VarintError::IncompleteData {
            expected_bytes: bytes_read + 1,
            actual_bytes: bytes_read,
        }
        .to_string(),
    })
}

/// Calculate the number of bytes needed to encode a value as varint
///
/// # Arguments
/// * `value` - The value to measure
///
/// # Returns
/// Number of bytes needed (1-10)
///
/// # Examples
/// ```
/// use sqlitegraph::backend::native::v3::compression::varint::varint_size;
///
/// assert_eq!(varint_size(0), 1);
/// assert_eq!(varint_size(127), 1);
/// assert_eq!(varint_size(128), 2);
/// assert_eq!(varint_size(16383), 2);
/// assert_eq!(varint_size(16384), 3);
/// ```
pub fn varint_size(value: u64) -> usize {
    if value == 0 {
        return 1;
    }

    let mut size = 0;
    let mut remaining = value;

    while remaining > 0 {
        size += 1;
        remaining >>= 7;
    }

    size
}

/// Encode a u32 value as varint (convenience wrapper)
///
/// # Arguments
/// * `value` - The u32 value to encode
///
/// # Returns
/// A Vec<u8> containing the varint-encoded value
pub fn encode_varint_u32(value: u32) -> Vec<u8> {
    encode_varint(value as u64)
}

/// Decode a u32 varint from a byte slice
///
/// # Arguments
/// * `bytes` - Input byte slice
///
/// # Returns
/// A tuple of (decoded_u32_value, bytes_read)
///
/// # Errors
/// Returns an error if the varint value exceeds u32::MAX
pub fn decode_varint_u32(bytes: &[u8]) -> NativeResult<(u32, usize)> {
    let (value, bytes_read) = decode_varint(bytes)?;

    if value > u32::MAX as u64 {
        return Err(NativeBackendError::InvalidHeader {
            field: "varint_u32".to_string(),
            reason: format!("Value {} exceeds u32::MAX", value),
        });
    }

    Ok((value as u32, bytes_read))
}

/// Encode a u16 value as varint (convenience wrapper)
///
/// # Arguments
/// * `value` - The u16 value to encode
///
/// # Returns
/// A Vec<u8> containing the varint-encoded value
pub fn encode_varint_u16(value: u16) -> Vec<u8> {
    encode_varint(value as u64)
}

/// Decode a u16 varint from a byte slice
///
/// # Arguments
/// * `bytes` - Input byte slice
///
/// # Returns
/// A tuple of (decoded_u16_value, bytes_read)
///
/// # Errors
/// Returns an error if the varint value exceeds u16::MAX
pub fn decode_varint_u16(bytes: &[u8]) -> NativeResult<(u16, usize)> {
    let (value, bytes_read) = decode_varint(bytes)?;

    if value > u16::MAX as u64 {
        return Err(NativeBackendError::InvalidHeader {
            field: "varint_u16".to_string(),
            reason: format!("Value {} exceeds u16::MAX", value),
        });
    }

    Ok((value as u16, bytes_read))
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Encoding Tests ===

    #[test]
    fn test_encode_varint_zero() {
        assert_eq!(encode_varint(0), vec![0x00]);
    }

    #[test]
    fn test_encode_varint_single_byte() {
        // Values 0-127 fit in one byte
        assert_eq!(encode_varint(1), vec![0x01]);
        assert_eq!(encode_varint(42), vec![0x2A]);
        assert_eq!(encode_varint(127), vec![0x7F]);
    }

    #[test]
    fn test_encode_varint_two_bytes() {
        // Values 128-16383 fit in two bytes
        assert_eq!(encode_varint(128), vec![0x80, 0x01]);
        assert_eq!(encode_varint(300), vec![0xAC, 0x02]);
        assert_eq!(encode_varint(16383), vec![0xFF, 0x7F]);
    }

    #[test]
    fn test_encode_varint_three_bytes() {
        // Values 16384-2097151 fit in three bytes
        assert_eq!(encode_varint(16384), vec![0x80, 0x80, 0x01]);
        assert_eq!(encode_varint(2097151), vec![0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn test_encode_varint_max_u64() {
        // u64::MAX requires 10 bytes
        let encoded = encode_varint(u64::MAX);
        assert_eq!(encoded.len(), 10);

        // Verify MSB pattern: first 9 bytes have MSB set, last byte doesn't
        for (i, &byte) in encoded.iter().enumerate() {
            if i < 9 {
                assert!(
                    byte & CONTINUATION_BIT != 0,
                    "Byte {} should have MSB set",
                    i
                );
            } else {
                assert!(
                    byte & CONTINUATION_BIT == 0,
                    "Byte {} should not have MSB set",
                    i
                );
            }
        }
    }

    #[test]
    fn test_encode_varint_u16() {
        // u16::MAX should encode efficiently
        let encoded = encode_varint_u16(u16::MAX);
        assert_eq!(encoded, vec![0xFF, 0xFF, 0x03]);

        // Small value
        assert_eq!(encode_varint_u16(100), vec![100]);
    }

    #[test]
    fn test_encode_varint_u32() {
        // u32::MAX requires 5 bytes
        let encoded = encode_varint_u32(u32::MAX);
        assert_eq!(encoded.len(), 5);
        assert_eq!(encoded, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    // === Decoding Tests ===

    #[test]
    fn test_decode_varint_zero() {
        assert_eq!(decode_varint(&[0x00]).unwrap(), (0, 1));
    }

    #[test]
    fn test_decode_varint_single_byte() {
        assert_eq!(decode_varint(&[0x01]).unwrap(), (1, 1));
        assert_eq!(decode_varint(&[0x2A]).unwrap(), (42, 1));
        assert_eq!(decode_varint(&[0x7F]).unwrap(), (127, 1));
    }

    #[test]
    fn test_decode_varint_two_bytes() {
        assert_eq!(decode_varint(&[0x80, 0x01]).unwrap(), (128, 2));
        assert_eq!(decode_varint(&[0xAC, 0x02]).unwrap(), (300, 2));
        assert_eq!(decode_varint(&[0xFF, 0x7F]).unwrap(), (16383, 2));
    }

    #[test]
    fn test_decode_varint_three_bytes() {
        assert_eq!(decode_varint(&[0x80, 0x80, 0x01]).unwrap(), (16384, 3));
        assert_eq!(decode_varint(&[0xFF, 0xFF, 0x7F]).unwrap(), (2097151, 3));
    }

    #[test]
    fn test_decode_varint_with_extra_data() {
        // Should only read the varint bytes
        let data = vec![0x80, 0x01, 0xFF, 0xFF, 0xFF];
        assert_eq!(decode_varint(&data).unwrap(), (128, 2));
    }

    #[test]
    fn test_decode_varint_u16() {
        // Single-byte value (127 max)
        assert_eq!(decode_varint_u16(&[0x7F]).unwrap(), (127, 1));
        // Two-byte value (255)
        assert_eq!(decode_varint_u16(&[0xFF, 0x01]).unwrap(), (255, 2));
        // Max u16
        assert_eq!(
            decode_varint_u16(&[0xFF, 0xFF, 0x03]).unwrap(),
            (u16::MAX, 3)
        );
    }

    #[test]
    fn test_decode_varint_u16_incomplete() {
        // Test incomplete varint - should error
        let result = decode_varint_u16(&[0x80]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_varint_u32() {
        // Single-byte value (127 max)
        assert_eq!(decode_varint_u32(&[0x7F]).unwrap(), (127, 1));
        // Two-byte value (255)
        assert_eq!(decode_varint_u32(&[0xFF, 0x01]).unwrap(), (255, 2));
        // Max u32
        assert_eq!(
            decode_varint_u32(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).unwrap(),
            (u32::MAX, 5)
        );
    }

    #[test]
    fn test_decode_varint_u32_incomplete() {
        // Test incomplete varint - should error
        let result = decode_varint_u32(&[0x80]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_varint_u32_overflow() {
        // Value > u32::MAX should error
        let large_value = encode_varint(u32::MAX as u64 + 1);
        assert!(decode_varint_u32(&large_value).is_err());
    }

    #[test]
    fn test_decode_varint_u16_overflow() {
        // Value > u16::MAX should error
        let large_value = encode_varint(u16::MAX as u64 + 1);
        assert!(decode_varint_u16(&large_value).is_err());
    }

    // === Error Cases ===

    #[test]
    fn test_decode_varint_empty_input() {
        let result = decode_varint(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_varint_incomplete() {
        // Two-byte varint but only one byte provided
        let result = decode_varint(&[0x80]);
        assert!(result.is_err());

        // Three-byte varint but only two bytes provided
        let result = decode_varint(&[0x80, 0x80]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_varint_too_long() {
        // Create a varint longer than 10 bytes
        let too_long = vec![0x80u8; 11];
        let result = decode_varint(&too_long);
        assert!(result.is_err());
    }

    // === Round-trip Tests ===

    #[test]
    fn test_round_trip_small_values() {
        let test_values = vec![0, 1, 42, 127, 128, 255, 256, 1000];

        for value in test_values {
            let encoded = encode_varint(value);
            let (decoded, bytes_read) = decode_varint(&encoded).unwrap();

            assert_eq!(decoded, value, "Round-trip failed for {}", value);
            assert_eq!(
                bytes_read,
                encoded.len(),
                "Byte count mismatch for {}",
                value
            );
        }
    }

    #[test]
    fn test_round_trip_edge_values() {
        let test_values = vec![
            0,
            1,
            127,
            128,
            16383,
            16384,
            2097151,
            2097152,
            u16::MAX as u64,
            u32::MAX as u64,
            u64::MAX,
        ];

        for value in test_values {
            let encoded = encode_varint(value);
            let (decoded, bytes_read) = decode_varint(&encoded).unwrap();

            assert_eq!(decoded, value, "Round-trip failed for {}", value);
            assert_eq!(bytes_read, encoded.len());
        }
    }

    #[test]
    fn test_round_trip_u16() {
        for value in [0, 1, 100, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
            let encoded = encode_varint_u16(value);
            let (decoded, bytes_read) = decode_varint_u16(&encoded).unwrap();

            assert_eq!(decoded, value);
            assert_eq!(bytes_read, encoded.len());
        }
    }

    #[test]
    fn test_round_trip_u32() {
        for value in [0, 1, 1000, u16::MAX as u32, u32::MAX / 2, u32::MAX] {
            let encoded = encode_varint_u32(value);
            let (decoded, bytes_read) = decode_varint_u32(&encoded).unwrap();

            assert_eq!(decoded, value);
            assert_eq!(bytes_read, encoded.len());
        }
    }

    // === varint_size Tests ===

    #[test]
    fn test_varint_size_calculation() {
        assert_eq!(varint_size(0), 1);
        assert_eq!(varint_size(127), 1);
        assert_eq!(varint_size(128), 2);
        assert_eq!(varint_size(16383), 2);
        assert_eq!(varint_size(16384), 3);
        assert_eq!(varint_size(2097151), 3);
        assert_eq!(varint_size(2097152), 4);

        // Verify matches actual encoding
        for value in [0, 1, 127, 128, 16383, 16384, u32::MAX as u64, u64::MAX] {
            assert_eq!(
                varint_size(value),
                encode_varint(value).len(),
                "varint_size mismatch for {}",
                value
            );
        }
    }

    // === Data Integrity Tests ===

    #[test]
    fn test_varint_7_bit_chunks() {
        // Verify that each byte correctly extracts 7 bits
        let value = 0b111_1111_1111_1111_1111_1111; // 22 bits

        let encoded = encode_varint(value);
        assert_eq!(encoded.len(), 4); // ceil(22/7) = 4

        // Verify bit patterns
        assert_eq!(encoded[0] & !DATA_MASK, CONTINUATION_BIT); // Has MSB
        assert_eq!(encoded[0] & DATA_MASK, 0b1111111); // Lower 7 bits

        assert_eq!(encoded[3] & CONTINUATION_BIT, 0); // Last byte, no MSB
    }

    #[test]
    fn test_varint_efficiency_for_small_values() {
        // Most node offsets and counts are small (< 128)
        assert_eq!(encode_varint(0).len(), 1);
        assert_eq!(encode_varint(10).len(), 1);
        assert_eq!(encode_varint(42).len(), 1);
        assert_eq!(encode_varint(100).len(), 1);
        assert_eq!(encode_varint(127).len(), 1);

        // 128+ requires 2 bytes
        assert_eq!(encode_varint(128).len(), 2);
        assert_eq!(encode_varint(255).len(), 2);
        assert_eq!(encode_varint(256).len(), 2);
        assert_eq!(encode_varint(1000).len(), 2);
    }
}
