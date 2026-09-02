//! V3 Compression utilities
//!
//! This module provides compression utilities for the V3 storage format:
//! - Delta encoding for node IDs (saves ~4 bytes per node)
//! - Varint encoding for variable-length fields (saves ~8 bytes per node)
//!
//! All encoding/decoding uses saturating arithmetic to prevent underflow/overflow.

pub mod delta;
pub mod edge_delta;
pub mod varint;

// Re-export compression utilities
pub use delta::{DeltaDecodeError, decode_id_delta, encode_id_delta};
pub use edge_delta::{compress_edge_ids, compression_ratio, decompress_edge_ids};
pub use varint::{MAX_VARINT_BYTES, VarintError, decode_varint, encode_varint};

/// Compression statistics for tracking space savings
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompressionStats {
    /// Original (uncompressed) size in bytes
    pub original_size: usize,

    /// Compressed size in bytes
    pub compressed_size: usize,

    /// Number of records compressed
    pub record_count: u32,
}

impl CompressionStats {
    /// Calculate compression ratio (original / compressed)
    pub fn compression_ratio(&self) -> f64 {
        if self.compressed_size == 0 {
            1.0
        } else {
            self.original_size as f64 / self.compressed_size as f64
        }
    }

    /// Calculate space savings percentage
    ///
    /// Returns 0.0 if compression expanded the size (no savings).
    /// Returns negative value if compressed size is larger than original.
    pub fn space_savings_pct(&self) -> f64 {
        if self.original_size == 0 {
            0.0
        } else {
            let diff = self.original_size as f64 - self.compressed_size as f64;
            (diff / self.original_size as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_stats_empty() {
        let stats = CompressionStats::default();
        assert_eq!(stats.compression_ratio(), 1.0);
        assert_eq!(stats.space_savings_pct(), 0.0);
    }

    #[test]
    fn test_compression_stats_calculation() {
        let stats = CompressionStats {
            original_size: 100,
            compressed_size: 50,
            record_count: 10,
        };

        assert_eq!(stats.compression_ratio(), 2.0);
        assert_eq!(stats.space_savings_pct(), 50.0);
    }

    #[test]
    fn test_compression_stats_no_compression() {
        let stats = CompressionStats {
            original_size: 100,
            compressed_size: 100,
            record_count: 5,
        };

        assert_eq!(stats.compression_ratio(), 1.0);
        assert_eq!(stats.space_savings_pct(), 0.0);
    }

    #[test]
    fn test_compression_stats_expansion() {
        // Edge case: compression actually expands (shouldn't happen with good encoding)
        let stats = CompressionStats {
            original_size: 50,
            compressed_size: 100,
            record_count: 1,
        };

        assert_eq!(stats.compression_ratio(), 0.5);
        assert!(stats.space_savings_pct() < 0.0);
    }
}
