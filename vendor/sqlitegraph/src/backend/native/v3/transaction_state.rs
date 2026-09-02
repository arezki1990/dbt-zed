//! Native-v3 transaction frame/state helpers.

use super::{V3Backend, map_v3_error};
use crate::SqliteGraphError;
use crate::backend::native::v3::btree::BTreeManager;
use crate::backend::native::v3::{
    NodeStore, PageAllocator, PersistentHeaderV3, V3_HEADER_SIZE, V3EdgeStore,
};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct GraphTransactionFrame {
    pub(super) backup_dir: PathBuf,
    pub(super) snapshot_version: u64,
    pub(super) snapshot_registry: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct GraphTransactionState {
    pub(super) frames: Vec<GraphTransactionFrame>,
}

impl V3Backend {
    pub(super) fn graph_backup_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "sqlitegraph-v3-tx-{}-{}",
            std::process::id(),
            Self::transaction_timestamp()
        ))
    }

    pub(super) fn copy_if_exists(from: &Path, to: &Path) -> Result<(), SqliteGraphError> {
        if from.exists() {
            fs::copy(from, to).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to copy {} to {}: {}",
                    from.display(),
                    to.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn restore_optional_file(
        backup: &Path,
        target: &Path,
    ) -> Result<(), SqliteGraphError> {
        if backup.exists() {
            fs::copy(backup, target).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to restore {} from {}: {}",
                    target.display(),
                    backup.display(),
                    e
                ))
            })?;
        } else if target.exists() {
            fs::remove_file(target).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to remove stale sidecar {}: {}",
                    target.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn begin_graph_transaction_frame(&self) -> Result<(), SqliteGraphError> {
        self.flush_to_disk()?;

        let backup_dir = Self::graph_backup_dir();
        fs::create_dir_all(&backup_dir).map_err(|e| {
            SqliteGraphError::connection(format!(
                "Failed to create native-v3 transaction backup dir {}: {}",
                backup_dir.display(),
                e
            ))
        })?;

        Self::copy_if_exists(&self.db_path, &backup_dir.join("graph.snapshot"))?;
        Self::copy_if_exists(
            &Self::index_sidecar_path(&self.db_path),
            &backup_dir.join("index.snapshot"),
        )?;
        Self::copy_if_exists(
            &Self::edge_metadata_path(&self.db_path),
            &backup_dir.join("edge.snapshot"),
        )?;

        let frame = GraphTransactionFrame {
            backup_dir,
            snapshot_version: self.current_graph_version(),
            snapshot_registry: self.snapshot_registry.read().clone(),
        };

        self.graph_tx_state.lock().frames.push(frame);
        Ok(())
    }

    pub(super) fn discard_graph_transaction_frame(&self) -> Result<(), SqliteGraphError> {
        let frame = self.graph_tx_state.lock().frames.pop().ok_or_else(|| {
            SqliteGraphError::connection("No native-v3 transaction frame to discard".to_string())
        })?;

        if frame.backup_dir.exists() {
            fs::remove_dir_all(&frame.backup_dir).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to remove native-v3 transaction backup dir {}: {}",
                    frame.backup_dir.display(),
                    e
                ))
            })?;
        }

        Ok(())
    }

    pub(super) fn rollback_graph_transaction_frame(&self) -> Result<(), SqliteGraphError> {
        let frame = self.graph_tx_state.lock().frames.pop().ok_or_else(|| {
            SqliteGraphError::connection("No native-v3 transaction frame to roll back".to_string())
        })?;

        Self::restore_optional_file(&frame.backup_dir.join("graph.snapshot"), &self.db_path)?;
        Self::restore_optional_file(
            &frame.backup_dir.join("index.snapshot"),
            &Self::index_sidecar_path(&self.db_path),
        )?;
        Self::restore_optional_file(
            &frame.backup_dir.join("edge.snapshot"),
            &Self::edge_metadata_path(&self.db_path),
        )?;

        self.reload_graph_runtime_from_disk()?;
        self.set_graph_version(frame.snapshot_version);
        self.set_snapshot_registry(frame.snapshot_registry);

        if frame.backup_dir.exists() {
            fs::remove_dir_all(&frame.backup_dir).map_err(|e| {
                SqliteGraphError::connection(format!(
                    "Failed to remove native-v3 transaction backup dir {}: {}",
                    frame.backup_dir.display(),
                    e
                ))
            })?;
        }

        Ok(())
    }

    pub(super) fn reload_graph_runtime_from_disk(&self) -> Result<(), SqliteGraphError> {
        let mut file = File::open(&self.db_path).map_err(|e| {
            SqliteGraphError::connection(format!(
                "Failed to reopen native-v3 database file {}: {}",
                self.db_path.display(),
                e
            ))
        })?;

        let mut header_bytes = vec![0u8; V3_HEADER_SIZE as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to read header: {}", e)))?;
        let header = PersistentHeaderV3::from_bytes(&header_bytes).map_err(map_v3_error)?;
        header.validate().map_err(map_v3_error)?;

        *self.allocator.write() = PageAllocator::new(&header);

        let btree = BTreeManager::with_root(
            Arc::clone(&self.allocator),
            None,
            header.root_index_page,
            header.btree_height,
            self.db_path.clone(),
        );
        let mut node_store = NodeStore::new(&header, self.db_path.clone());
        node_store.initialize(
            BTreeManager::with_root(
                Arc::clone(&self.allocator),
                None,
                header.root_index_page,
                header.btree_height,
                self.db_path.clone(),
            ),
            Arc::clone(&self.allocator),
            None,
        );
        let max_node_id = node_store
            .node_ids()
            .map_err(map_v3_error)?
            .into_iter()
            .max()
            .unwrap_or(0);
        node_store.set_next_node_id(max_node_id + 1);

        let mut edge_store = V3EdgeStore::with_path_and_allocator(
            BTreeManager::with_root(
                Arc::clone(&self.allocator),
                None,
                header.edge_data_offset,
                header.reserved,
                self.db_path.clone(),
            ),
            None,
            self.db_path.clone(),
            Arc::clone(&self.allocator),
            header.page_size,
        );
        if let Some(ref wal_arc) = self.wal {
            edge_store.set_wal(Arc::clone(wal_arc));
        }
        let _ = edge_store.restore_btree_from_metadata();

        *self.btree.write() = btree;
        *self.node_store.write() = node_store;
        *self.edge_store.write() = edge_store;
        *self.header.write() = header.clone();

        self.node_cache.clear();
        self.kind_index.clear();
        self.name_index.clear();
        self.rebuild_indexes();

        let _ = crate::backend::native::v3::index_persistence::persist_indexes(
            &self.db_path,
            &self.kind_index,
            &self.name_index,
            header.node_count,
        );

        Ok(())
    }
}
