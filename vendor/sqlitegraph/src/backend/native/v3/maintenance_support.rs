use super::{V3Backend, WriteBatchGuard, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::PersistentHeaderV3;
use crate::backend::{EdgeSpec, GraphBackend};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

impl V3Backend {
    /// Get a reference to the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Check if WAL is enabled
    pub fn is_wal_enabled(&self) -> bool {
        self.wal.is_some()
    }

    /// Get the current header state
    pub fn header(&self) -> PersistentHeaderV3 {
        self.header.read().clone()
    }

    /// Flush any pending writes to disk
    pub fn flush_to_disk(&self) -> Result<(), SqliteGraphError> {
        let edge_store = self.edge_store.write();
        edge_store.flush(None).map_err(map_v3_error)?;
        drop(edge_store);

        let mut header = self.header.write();
        if let Some(root_page) = self.edge_store.read().btree_root_page_id() {
            header.edge_data_offset = root_page;
            header.reserved = self.edge_store.read().btree_height();
        } else {
            header.edge_data_offset = 0;
            header.reserved = 0;
        }
        let allocator = self.allocator.read();
        header.total_pages = allocator.total_pages();
        drop(allocator);
        let node_count = header.node_count;
        drop(header);

        let _ = crate::backend::native::v3::index_persistence::persist_indexes(
            &self.db_path,
            &self.kind_index,
            &self.name_index,
            node_count,
        );

        self.sync_header()?;

        let kv_guard = self.kv_store.read();
        if let Some(ref kv) = *kv_guard {
            let _ = crate::backend::native::v3::wal::write_kv_checkpoint(&self.db_path, kv);
        }
        drop(kv_guard);

        self.checkpoint()?;
        if let Some(ref wal) = self.wal {
            wal.write()
                .flush()
                .map_err(|e| SqliteGraphError::connection(format!("WAL flush failed: {:?}", e)))?;
            wal.write().truncate().map_err(|e| {
                SqliteGraphError::connection(format!("WAL truncate failed: {:?}", e))
            })?;
        }
        Ok(())
    }

    /// Sync header to disk
    pub(super) fn sync_header(&self) -> Result<(), SqliteGraphError> {
        let header = self.header.read();
        let header_bytes = header.to_bytes();

        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.db_path)
            .map_err(|e| {
                SqliteGraphError::connection(format!("Failed to open file for header sync: {}", e))
            })?;

        file.seek(SeekFrom::Start(0)).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to seek to header: {}", e))
        })?;
        file.write_all(&header_bytes)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to write header: {}", e)))?;
        file.sync_all()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to sync header: {}", e)))?;

        Ok(())
    }

    /// Begin a write batch for amortized durability
    pub fn begin_batch(&self) -> WriteBatchGuard<'_> {
        WriteBatchGuard::new(self)
    }

    /// Bulk insert a list of weighted edges within a single write batch transaction
    pub fn batch_insert_edges_with_weights(
        &self,
        edges: Vec<(i64, i64, f32, Option<String>)>,
    ) -> Result<(), SqliteGraphError> {
        let mut batch = self.begin_batch();
        for (src, dst, weight, edge_type) in edges {
            let spec = EdgeSpec {
                from: src,
                to: dst,
                edge_type: edge_type.unwrap_or_default(),
                data: serde_json::json!({ "weight": weight }),
            };
            batch.insert_edge(spec)?;
        }
        batch.commit()
    }
}
