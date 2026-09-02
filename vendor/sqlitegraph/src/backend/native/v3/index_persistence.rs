//! Index Persistence for V3 Native Backend
//!
//! This module provides functionality to persist and restore the kind and name indexes
//! to avoid O(N) page scan during database open.
//!
//! ## Format
//!
//! The `.v3index` file format:
//! - magic[4]: "V3XI" (V3 eXlude Index)
//! - version[4]: Version number (currently 1)
//! - db_node_count[8]: Node count from DB header when index was persisted (staleness guard)
//! - kind_count[4]: Number of unique kinds
//! - kind_entries: Variable length
//!   - For each kind:
//!     - kind_len[4]: Length of kind string
//!     - kind_bytes: Kind string data
//!     - node_count[4]: Number of nodes with this kind
//!     - node_ids: Node IDs (8 bytes each)
//! - name_count[4]: Number of unique names
//! - name_entries: Variable length
//!   - For each name:
//!     - name_len[4]: Length of name string
//!     - name_bytes: Name string data
//!     - node_count[4]: Number of nodes with this name
//!     - node_ids: Node IDs (8 bytes each)

use crate::backend::native::v3::kind_index::KindIndex;
use crate::backend::native::v3::name_index::NameIndex;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

mod restore_support;
mod support;

/// Magic number for the index file
pub const INDEX_MAGIC: &[u8; 4] = b"V3XI";
/// Current version of the index file format
pub const INDEX_VERSION: u32 = 1;

/// Error type for index persistence operations
#[derive(Debug, Clone)]
pub enum IndexPersistenceError {
    Io(String),
    InvalidMagic(Vec<u8>),
    UnsupportedVersion(u32),
    Corrupted(String),
}

impl std::fmt::Display for IndexPersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "IO error: {}", msg),
            Self::InvalidMagic(bytes) => write!(f, "Invalid magic: {:?}", bytes),
            Self::UnsupportedVersion(v) => write!(f, "Unsupported version: {}", v),
            Self::Corrupted(msg) => write!(f, "Corrupted data: {}", msg),
        }
    }
}

impl std::error::Error for IndexPersistenceError {}

/// Persist the kind and name indexes to a sidecar file
///
/// # Arguments
/// * `db_path` - Path to the main database file
/// * `kind_index` - The kind index to persist
/// * `name_index` - The name index to persist
/// * `db_node_count` - Node count from DB header (for staleness detection on restore)
///
/// # Returns
/// Ok(()) if persistence succeeded, Err otherwise
pub fn persist_indexes(
    db_path: &Path,
    kind_index: &KindIndex,
    name_index: &NameIndex,
    db_node_count: u64,
) -> Result<(), IndexPersistenceError> {
    let index_path = index_path_for_db(db_path);
    let temp_path = temp_path_for_db(db_path);

    // Create temporary file
    let mut file = std::fs::File::create(&temp_path)
        .map_err(|e| IndexPersistenceError::Io(format!("Failed to create temp file: {}", e)))?;

    // Write magic and version
    support::write_all(&mut file, INDEX_MAGIC, "magic")?;
    support::write_all(&mut file, &INDEX_VERSION.to_be_bytes(), "version")?;

    // Write DB node count (staleness guard)
    support::write_all(&mut file, &db_node_count.to_be_bytes(), "db node count")?;

    // Write kind index
    let kind_data = kind_index.export();
    let kind_entries: Vec<(&String, &Vec<i64>)> = kind_data.iter().collect();
    support::write_all(
        &mut file,
        &(kind_entries.len() as u32).to_be_bytes(),
        "kind count",
    )?;

    for (kind, node_ids) in kind_entries {
        support::write_string_entry(&mut file, kind, node_ids, "kind")?;
    }

    // Write name index
    let name_data = name_index.export();
    let name_entries: Vec<(&String, &Vec<i64>)> = name_data.iter().collect();
    support::write_all(
        &mut file,
        &(name_entries.len() as u32).to_be_bytes(),
        "name count",
    )?;

    for (name, node_ids) in name_entries {
        support::write_string_entry(&mut file, name, node_ids, "name")?;
    }

    // Sync to ensure data is written
    file.sync_all()
        .map_err(|e| IndexPersistenceError::Io(format!("Failed to sync file: {}", e)))?;
    drop(file);

    // Atomic rename
    std::fs::rename(&temp_path, &index_path)
        .map_err(|e| IndexPersistenceError::Io(format!("Failed to rename index file: {}", e)))?;

    Ok(())
}

/// Restore the kind and name indexes from a sidecar file
///
/// # Arguments
/// * `db_path` - Path to the main database file
/// * `db_node_count` - Node count from DB header (for staleness validation)
///
/// # Returns
/// Ok((KindIndex, NameIndex)) if restoration succeeded, Err otherwise
pub fn restore_indexes(
    db_path: &Path,
    db_node_count: u64,
) -> Result<(KindIndex, NameIndex), IndexPersistenceError> {
    restore_support::restore_indexes(db_path, db_node_count)
}

/// Get the path to the index file for a given database path
pub fn index_path_for_db(db_path: &Path) -> std::path::PathBuf {
    let mut path = db_path.to_path_buf();
    path.set_extension("v3index");
    path
}

/// Get the temporary path for index file creation
pub fn temp_path_for_db(db_path: &Path) -> std::path::PathBuf {
    let mut path = db_path.to_path_buf();
    path.set_extension("v3index.tmp");
    path
}

/// Remove the index file (used during tests or cleanup)
pub fn remove_index_file(db_path: &Path) -> Result<(), std::io::Error> {
    let index_path = index_path_for_db(db_path);
    std::fs::remove_file(index_path)
}

#[cfg(test)]
mod tests;
