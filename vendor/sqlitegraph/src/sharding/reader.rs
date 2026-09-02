//! CSR shard reader for loading sharded graphs.
//!
//! `ShardReader` loads all CSR shards into memory for fast subgraph traversal.
//! Supports incremental loads and manifest-based validation.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::errors::SqliteGraphError;
use crate::sharding::manifest::{Manifest, ShardMetadata};
use crate::sharding::shard::{CsrEdge, CsrShard};

type Result<T> = std::result::Result<T, SqliteGraphError>;

/// Shard reader that loads and indexes CSR graph shards.
///
/// # Usage
///
/// ```no_run
/// use sqlitegraph::sharding::ShardReader;
/// use std::path::Path;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let reader = ShardReader::from_dir(Path::new("/path/to/shards"))?;
/// println!("Loaded {} shards with {} edges", reader.shard_count(), reader.total_edges());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct ShardReader {
    /// All loaded shards
    shards: Vec<CsrShard>,

    /// Manifest metadata
    manifest: Manifest,
}

impl ShardReader {
    /// Load all shards from a directory.
    ///
    /// # Arguments
    ///
    /// * `shard_dir` - Directory containing shard files and `manifest.json`
    ///
    /// # Returns
    ///
    /// * `Ok(ShardReader)` if all shards loaded successfully
    /// * `Err(SqliteGraphError)` if manifest missing or shard files fail to load
    ///
    /// # Errors
    ///
    /// - `IoContext` — manifest.json not found or unreadable
    /// - `IoContext` — shard file not found or unreadable
    /// - `ParseError` — shard file format invalid
    pub fn from_dir(shard_dir: &Path) -> Result<Self> {
        // Load manifest
        let manifest_path = shard_dir.join("manifest.json");
        let manifest_json = std::fs::read_to_string(&manifest_path).map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to read manifest: {} - {}",
                manifest_path.display(),
                e
            ))
        })?;

        // Parse the manifest JSON structure
        let manifest_value: serde_json::Value =
            serde_json::from_str(&manifest_json).map_err(|e| {
                SqliteGraphError::InvalidInput(format!(
                    "Failed to parse manifest JSON: {} - {}",
                    manifest_path.display(),
                    e
                ))
            })?;

        let shard_metadata: Vec<ShardMetadata> = serde_json::from_value(
            manifest_value
                .get("shards")
                .ok_or_else(|| {
                    SqliteGraphError::InvalidInput("Manifest missing 'shards' field".to_string())
                })?
                .clone(),
        )
        .map_err(|e| {
            SqliteGraphError::InvalidInput(format!(
                "Failed to parse shard metadata: {} - {}",
                manifest_path.display(),
                e
            ))
        })?;

        let manifest = Manifest::new(shard_metadata, "1.0".to_string());

        // Load all shards
        let mut shards = Vec::with_capacity(manifest.shards.len());

        for shard_meta in &manifest.shards {
            let shard_path = shard_dir.join(&shard_meta.file_name);
            let shard = load_shard_file(&shard_path)?;

            // Validate shard matches metadata
            if shard.shard_id != shard_meta.shard_id {
                return Err(SqliteGraphError::GraphCorruption(format!(
                    "Shard ID mismatch: file={}, manifest={}",
                    shard.shard_id, shard_meta.shard_id
                )));
            }

            if shard.source_start != shard_meta.source_start
                || shard.source_end != shard_meta.source_end
            {
                return Err(SqliteGraphError::GraphCorruption(format!(
                    "Source range mismatch: file=[{}, {}), manifest=[{}, {}]",
                    shard.source_start,
                    shard.source_end,
                    shard_meta.source_start,
                    shard_meta.source_end
                )));
            }

            if shard.edge_count() != shard_meta.edge_count {
                return Err(SqliteGraphError::GraphCorruption(format!(
                    "Edge count mismatch: file={}, manifest={}",
                    shard.edge_count(),
                    shard_meta.edge_count
                )));
            }

            shards.push(shard);
        }

        Ok(Self { shards, manifest })
    }

    /// Return the total number of edges across all shards.
    pub fn total_edges(&self) -> usize {
        self.manifest.total_edges
    }

    /// Return the number of shards.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    /// Get a specific shard by ID.
    ///
    /// Returns `None` if shard ID is out of range.
    pub fn get_shard(&self, shard_id: usize) -> Option<&CsrShard> {
        self.shards.get(shard_id)
    }

    /// Get all shard IDs in sorted order.
    pub fn shard_ids(&self) -> Vec<usize> {
        self.manifest.shard_ids()
    }

    /// Get the manifest metadata.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// Load a single CSR shard file.
///
/// # Format
///
/// Shard files are binary CSR format:
/// - Header: `CSR` magic + version + edge count
/// - Edges: Each edge = src(u32) + dst(u32) + weight(f32) + flags(u32)
fn load_shard_file(path: &Path) -> Result<CsrShard> {
    let mut file = File::open(path).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to open shard file: {} - {}",
            path.display(),
            e
        ))
    })?;

    // Read and parse header
    let mut magic = [0u8; 3];
    file.read_exact(&mut magic).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read magic from: {} - {}",
            path.display(),
            e
        ))
    })?;

    if &magic != b"CSR" {
        return Err(SqliteGraphError::InvalidInput(format!(
            "Invalid magic: {:?}",
            String::from_utf8_lossy(&magic)
        )));
    }

    let mut version_buf = [0u8; 4];
    file.read_exact(&mut version_buf).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read version from: {} - {}",
            path.display(),
            e
        ))
    })?;
    let version = u32::from_le_bytes(version_buf);

    if version != 1 {
        return Err(SqliteGraphError::Unsupported(format!(
            "Unsupported version: {}",
            version
        )));
    }

    let mut edge_count_buf = [0u8; 8];
    file.read_exact(&mut edge_count_buf).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read edge count from: {} - {}",
            path.display(),
            e
        ))
    })?;
    let edge_count = u64::from_le_bytes(edge_count_buf) as usize;

    // Read shard metadata
    let mut shard_id_buf = [0u8; 8];
    file.read_exact(&mut shard_id_buf).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read shard ID from: {} - {}",
            path.display(),
            e
        ))
    })?;
    let shard_id = u64::from_le_bytes(shard_id_buf) as usize;

    let mut source_start_buf = [0u8; 4];
    file.read_exact(&mut source_start_buf).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read source_start from: {} - {}",
            path.display(),
            e
        ))
    })?;
    let source_start = u32::from_le_bytes(source_start_buf);

    let mut source_end_buf = [0u8; 4];
    file.read_exact(&mut source_end_buf).map_err(|e| {
        SqliteGraphError::ConnectionError(format!(
            "Failed to read source_end from: {} - {}",
            path.display(),
            e
        ))
    })?;
    let source_end = u32::from_le_bytes(source_end_buf);

    let mut shard = CsrShard::new(shard_id, source_start, source_end);

    // Read edges
    for _ in 0..edge_count {
        let mut src_buf = [0u8; 4];
        file.read_exact(&mut src_buf).map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to read edge src from: {} - {}",
                path.display(),
                e
            ))
        })?;
        let src = u32::from_le_bytes(src_buf);

        let mut dst_buf = [0u8; 4];
        file.read_exact(&mut dst_buf).map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to read edge dst from: {} - {}",
                path.display(),
                e
            ))
        })?;
        let dst = u32::from_le_bytes(dst_buf);

        let mut weight_buf = [0u8; 4];
        file.read_exact(&mut weight_buf).map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to read edge weight from: {} - {}",
                path.display(),
                e
            ))
        })?;
        let weight = f32::from_le_bytes(weight_buf);

        let mut flags_buf = [0u8; 4];
        file.read_exact(&mut flags_buf).map_err(|e| {
            SqliteGraphError::ConnectionError(format!(
                "Failed to read edge flags from: {} - {}",
                path.display(),
                e
            ))
        })?;
        let flags = u32::from_le_bytes(flags_buf);

        shard.add_edge(CsrEdge {
            src,
            dst,
            weight,
            flags,
        });
    }

    Ok(shard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_shard(dir: &Path, shard_id: usize, edges: Vec<(u32, u32, f32, u32)>) -> PathBuf {
        let path = dir.join(format!("shard_{:04}.csr", shard_id));
        let mut file = std::fs::File::create(&path).unwrap();

        // Calculate source range based on shard_id (1000 sources per shard)
        let source_start = (shard_id * 1000) as u32;
        let source_end = ((shard_id + 1) * 1000) as u32;

        // Write header
        file.write_all(b"CSR").unwrap();
        file.write_all(&(1u32).to_le_bytes()).unwrap(); // version
        file.write_all(&(edges.len() as u64).to_le_bytes()).unwrap(); // edge count

        // Write shard metadata
        file.write_all(&(shard_id as u64).to_le_bytes()).unwrap();
        file.write_all(&(source_start.to_le_bytes())).unwrap(); // source_start
        file.write_all(&(source_end.to_le_bytes())).unwrap(); // source_end

        // Write edges
        for (src, dst, weight, flags) in edges {
            file.write_all(&src.to_le_bytes()).unwrap();
            file.write_all(&dst.to_le_bytes()).unwrap();
            file.write_all(&weight.to_le_bytes()).unwrap();
            file.write_all(&flags.to_le_bytes()).unwrap();
        }

        path
    }

    fn create_test_manifest(dir: &Path, shards: Vec<(usize, u32, u32, usize, &str)>) -> PathBuf {
        let path = dir.join("manifest.json");
        let mut file = std::fs::File::create(&path).unwrap();

        let shards_data: Vec<serde_json::Value> = shards
            .iter()
            .map(|(id, start, end, count, file_name)| {
                serde_json::json!({
                    "shard_id": id,
                    "source_start": start,
                    "source_end": end,
                    "edge_count": count,
                    "file_name": file_name,
                })
            })
            .collect();

        let manifest = serde_json::json!({
            "shards": shards_data,
            "total_edges": shards.iter().map(|(_, _, _, count, _)| count).sum::<usize>(),
            "total_sources": shards.iter().map(|(_, start, end, _, _)| (end - start) as usize).sum::<usize>(),
            "version": "1.0",
            "created_at": 0,
        });

        file.write_all(manifest.to_string().as_bytes()).unwrap();

        path
    }

    #[test]
    fn test_from_dir() {
        let temp = TempDir::new().unwrap();

        // Create test shards
        let shards = vec![
            (0, 0, 1000, 2, "shard_0000.csr"),
            (1, 1000, 2000, 3, "shard_0001.csr"),
        ];

        create_test_shard(
            temp.path(),
            0,
            vec![
                (500, 1000, 0.5, 0), // src in [0, 1000)
                (600, 1100, 0.3, 0), // src in [0, 1000)
            ],
        );

        create_test_shard(
            temp.path(),
            1,
            vec![
                (1500, 2000, 0.7, 0), // src in [1000, 2000)
                (1600, 2100, 0.2, 0), // src in [1000, 2000)
                (1700, 2200, 0.4, 0), // src in [1000, 2000) - third edge
            ],
        );

        create_test_manifest(temp.path(), shards);

        // Load reader
        let reader = ShardReader::from_dir(temp.path()).unwrap();

        assert_eq!(reader.shard_count(), 2);
        assert_eq!(reader.total_edges(), 5);
        assert_eq!(reader.manifest().total_sources, 2000);

        // Test get_shard
        let shard_0 = reader.get_shard(0).unwrap();
        assert_eq!(shard_0.edge_count(), 2);
        assert_eq!(shard_0.shard_id, 0);

        let shard_1 = reader.get_shard(1).unwrap();
        assert_eq!(shard_1.edge_count(), 3);
        assert_eq!(shard_1.shard_id, 1);

        assert!(reader.get_shard(2).is_none());
    }

    #[test]
    fn test_from_dir_missing_manifest() {
        let temp = TempDir::new().unwrap();
        let result = ShardReader::from_dir(temp.path());

        assert!(result.is_err());
    }

    #[test]
    fn test_from_dir_missing_shard() {
        let temp = TempDir::new().unwrap();

        // Create manifest but missing shard file
        create_test_manifest(temp.path(), vec![(0, 0, 1000, 1, "shard_0000.csr")]);

        let result = ShardReader::from_dir(temp.path());

        assert!(result.is_err());
    }

    #[test]
    fn test_from_dir_metadata_mismatch() {
        let temp = TempDir::new().unwrap();

        create_test_shard(temp.path(), 0, vec![(500, 1000, 0.5, 0)]);
        // Manifest claims 2 edges but file has 1
        create_test_manifest(temp.path(), vec![(0, 0, 1000, 2, "shard_0000.csr")]);

        let result = ShardReader::from_dir(temp.path());

        assert!(result.is_err());
    }
}
