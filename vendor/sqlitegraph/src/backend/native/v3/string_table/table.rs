//! String Table - Deduplicated string storage for V3 backend
//!
//! Provides efficient storage and retrieval of strings (kinds, names, edge types)
//! with automatic deduplication and offset-based lookup.
//!
//! ## Design
//!
//! - Strings are stored sequentially in a buffer
//! - Each unique string gets a unique u16 offset
//! - Offsets are cumulative: offset[n] = sum(len(string[0..n-1]))
//! - Deduplication via HashMap<string, offset>
//! - Pre-populated with common edge types for efficiency
//!
//! ## Format
//!
//! ```text
//! [count: u32 BE]
//! [offset_0: u32 BE][len_0: u16 BE][string_0 bytes]
//! [offset_1: u32 BE][len_1: u16 BE][string_1 bytes]
//! ...
//! ```

use crate::backend::native::{NativeBackendError, NativeResult};
use std::collections::HashMap;

/// Common edge types pre-populated for efficiency
const COMMON_EDGE_TYPES: [&str; 16] = [
    "calls",
    "imports",
    "defines",
    "uses",
    "contains",
    "implements",
    "extends",
    "references",
    "declares",
    "exports",
    "inherits",
    "overrides",
    "depends_on",
    "relates_to",
    "includes",
    "aliases",
];

/// String table for deduplicated string storage
///
/// Maps strings to u16 offsets for compact storage in node records.
/// Supports serialization for persistence.
#[derive(Debug, Clone)]
pub struct StringTable {
    /// String data stored sequentially
    strings: Vec<String>,
    /// Cumulative offsets for each string
    offsets: Vec<u32>,
    /// Fast lookup: string -> offset
    string_to_offset: HashMap<String, u16>,
    /// Common edge type cache for fast lookup
    common_edge_types: HashMap<String, u16>,
}

impl StringTable {
    /// Create a new empty string table with common types pre-populated
    pub fn new() -> Self {
        let mut table = Self {
            strings: Vec::new(),
            offsets: Vec::new(),
            string_to_offset: HashMap::new(),
            common_edge_types: HashMap::new(),
        };
        table.prepopulate_common_types();
        table
    }

    /// Pre-populate common edge types with low offsets
    fn prepopulate_common_types(&mut self) {
        for edge_type in COMMON_EDGE_TYPES {
            let offset = self.add_string_internal(edge_type.to_string());
            self.common_edge_types
                .insert(edge_type.to_string(), offset as u16);
        }
    }

    /// Get or add a string, returning its offset
    ///
    /// If the string already exists, returns the existing offset.
    /// Otherwise, adds the string and returns the new offset.
    pub fn get_or_add_offset(&mut self, string: &str) -> NativeResult<u16> {
        // Check common types first (fast path)
        if let Some(&offset) = self.common_edge_types.get(string) {
            return Ok(offset);
        }

        // Check if already in table
        if let Some(&offset) = self.string_to_offset.get(string) {
            return Ok(offset);
        }

        // Add new string
        let offset = self.add_string_internal(string.to_string());
        if offset > u16::MAX as u32 {
            return Err(NativeBackendError::RecordTooLarge {
                size: offset,
                max_size: u16::MAX as u32,
            });
        }
        Ok(offset as u16)
    }

    /// Get string by offset
    ///
    /// Returns error if offset not found.
    pub fn get_string(&self, offset: u16) -> NativeResult<&str> {
        let offset_u32 = offset as u32;

        // Find the string with matching offset
        if let Some(index) = self.offsets.iter().position(|&o| o == offset_u32) {
            return Ok(&self.strings[index]);
        }

        Err(NativeBackendError::InvalidStringOffset { offset: offset_u32 })
    }

    /// Get the offset for an existing string (if present)
    pub fn get_offset(&self, string: &str) -> Option<u16> {
        self.string_to_offset.get(string).copied()
    }

    /// Check if string exists in table
    pub fn contains(&self, string: &str) -> bool {
        self.string_to_offset.contains_key(string)
    }

    /// Number of unique strings in table
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Check if table is empty
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Clear all strings from table
    pub fn clear(&mut self) {
        self.strings.clear();
        self.offsets.clear();
        self.string_to_offset.clear();
        self.common_edge_types.clear();
        self.prepopulate_common_types();
    }

    /// Add string internally, returning cumulative offset
    fn add_string_internal(&mut self, string: String) -> u32 {
        // Calculate cumulative offset
        let offset = if let Some(last) = self.offsets.last() {
            last + self.strings.last().map(|s| s.len() as u32).unwrap_or(0)
        } else {
            0
        };

        // Store the offset and string
        self.offsets.push(offset);
        self.string_to_offset.insert(string.clone(), offset as u16);
        self.strings.push(string);

        offset
    }

    /// Serialize to bytes for persistence
    ///
    /// Format:
    /// - 4 bytes: count (u32 BE)
    /// - For each string:
    ///   - 4 bytes: offset (u32 BE)
    ///   - 2 bytes: length (u16 BE)
    ///   - N bytes: string data (UTF-8)
    pub fn serialize(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(self.serialized_size());

        // Write count
        buffer.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());

        // Write each string entry
        for (string, &offset) in self.strings.iter().zip(self.offsets.iter()) {
            buffer.extend_from_slice(&offset.to_be_bytes());

            let bytes = string.as_bytes();
            let len = bytes.len().min(u16::MAX as usize) as u16;
            buffer.extend_from_slice(&len.to_be_bytes());
            buffer.extend_from_slice(&bytes[..len as usize]);
        }

        buffer
    }

    /// Deserialize from bytes
    ///
    /// Reconstructs the string table from serialized format.
    pub fn deserialize(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < 4 {
            return Err(NativeBackendError::BufferTooSmall {
                size: bytes.len(),
                min_size: 4,
            });
        }

        const MAX_STRING_COUNT: usize = 1_000_000; // Safety limit

        let mut offset = 0;
        let string_count = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        offset += 4;

        // Safety check: prevent excessive allocation
        if string_count > MAX_STRING_COUNT {
            return Err(NativeBackendError::CorruptStringTable {
                reason: format!(
                    "String count {} exceeds maximum {}",
                    string_count, MAX_STRING_COUNT
                ),
            });
        }

        let mut strings = Vec::with_capacity(string_count);
        let mut offsets = Vec::with_capacity(string_count);
        let mut string_to_offset = HashMap::with_capacity(string_count);

        for _ in 0..string_count {
            if offset + 6 > bytes.len() {
                return Err(NativeBackendError::BufferTooSmall {
                    size: bytes.len(),
                    min_size: offset + 6,
                });
            }

            // Read offset
            let string_offset = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            offset += 4;

            // Read length
            let string_len = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
            offset += 2;

            if offset + string_len > bytes.len() {
                return Err(NativeBackendError::BufferTooSmall {
                    size: bytes.len(),
                    min_size: offset + string_len,
                });
            }

            // Read string data
            let data = &bytes[offset..offset + string_len];
            let string =
                std::str::from_utf8(data).map_err(|e| NativeBackendError::CorruptStringTable {
                    reason: e.to_string(),
                })?;

            // Store
            string_to_offset.insert(string.to_string(), string_offset as u16);
            strings.push(string.to_string());
            offsets.push(string_offset);
            offset += string_len;
        }

        let mut table = StringTable {
            strings,
            offsets,
            string_to_offset,
            common_edge_types: HashMap::new(),
        };

        table.rebuild_common_type_cache();
        Ok(table)
    }

    /// Calculate serialized size in bytes
    pub fn serialized_size(&self) -> usize {
        4 + self.strings.iter().map(|s| 4 + 2 + s.len()).sum::<usize>()
    }

    /// Rebuild common type cache after deserialization
    fn rebuild_common_type_cache(&mut self) {
        self.common_edge_types.clear();
        for edge_type in COMMON_EDGE_TYPES {
            if let Some(&offset) = self.string_to_offset.get(edge_type) {
                self.common_edge_types.insert(edge_type.to_string(), offset);
            }
        }
    }

    /// Get all strings in table (for debugging)
    pub fn all_strings(&self) -> Vec<&str> {
        self.strings.iter().map(|s| s.as_str()).collect()
    }

    /// Get statistics about the table
    pub fn stats(&self) -> StringTableStats {
        let total_bytes: usize = self.strings.iter().map(|s| s.len()).sum();
        StringTableStats {
            unique_strings: self.strings.len(),
            total_bytes,
            common_types_cached: self.common_edge_types.len(),
        }
    }
}

impl Default for StringTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics for the string table
#[derive(Debug, Clone, Copy)]
pub struct StringTableStats {
    /// Number of unique strings
    pub unique_strings: usize,
    /// Total bytes of string data
    pub total_bytes: usize,
    /// Number of common types in cache
    pub common_types_cached: usize,
}
