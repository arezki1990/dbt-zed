//! V3 String Table Module
//!
//! Provides deduplicated string storage for node kinds, names, and edge types.
//!
//! ## Usage
//!
//! ```rust
//! use sqlitegraph::backend::native::v3::string_table::StringTable;
//!
//! let mut table = StringTable::new();
//!
//! // Add strings and get offsets
//! let kind_offset = table.get_or_add_offset("Function").unwrap();
//! let name_offset = table.get_or_add_offset("my_func").unwrap();
//!
//! // Retrieve strings by offset
//! let kind = table.get_string(kind_offset).unwrap();
//! let name = table.get_string(name_offset).unwrap();
//!
//! // Serialize for persistence
//! let bytes = table.serialize();
//! let restored = StringTable::deserialize(&bytes).unwrap();
//! ```

mod table;

pub use table::{StringTable, StringTableStats};

#[cfg(test)]
mod tests;
