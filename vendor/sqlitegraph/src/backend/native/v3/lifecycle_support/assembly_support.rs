use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::v3::btree::BTreeManager;
use crate::backend::native::v3::name_index::NameIndex;
use crate::backend::native::v3::wal::{V3WALPaths, WALWriter};
use crate::backend::native::v3::{
    FileCoordinator, KindIndex, KvStore, NodeCache, NodeStore, PageAllocator, PersistentHeaderV3,
    V3EdgeStore,
};
use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

impl V3Backend {
    pub(crate) fn assemble_backend(
        db_path: PathBuf,
        coordinator: Arc<FileCoordinator>,
        sqlite_conn: Connection,
        btree: BTreeManager,
        node_store: NodeStore,
        edge_store: V3EdgeStore,
        allocator: Arc<RwLock<PageAllocator>>,
        wal: Option<Arc<RwLock<WALWriter>>>,
        header: PersistentHeaderV3,
        kind_index: KindIndex,
        name_index: NameIndex,
    ) -> Self {
        Self {
            db_path,
            file_coordinator: Some(coordinator),
            async_coordinator: std::sync::OnceLock::new(),
            sqlite_conn: Arc::new(Mutex::new(sqlite_conn)),
            btree: RwLock::new(btree),
            node_store: RwLock::new(node_store),
            edge_store: RwLock::new(edge_store),
            allocator,
            wal,
            header: RwLock::new(header),
            kv_store: RwLock::new(None),
            publisher: RwLock::new(None),
            kind_index,
            name_index,
            node_cache: NodeCache::new(
                crate::backend::native::v3::constants::node_cache::DEFAULT_CACHE_CAPACITY,
            ),
            current_snapshot_version: RwLock::new(1),
            snapshot_registry: RwLock::new(HashMap::new()),
            hnsw_indexes: RwLock::new(HashMap::new()),
            graph_tx_state: Mutex::new(
                super::super::transaction_state::GraphTransactionState::default(),
            ),
        }
    }

    pub(crate) fn create_wal_writer(
        db_path: &Path,
    ) -> Result<Arc<RwLock<WALWriter>>, SqliteGraphError> {
        let wal_path = V3WALPaths::wal_file(db_path);
        let wal_writer = WALWriter::new(wal_path, 1)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to create WAL: {:?}", e)))?;
        wal_writer.write_header().map_err(|e| {
            SqliteGraphError::connection(format!("Failed to write WAL header: {:?}", e))
        })?;
        Ok(Arc::new(RwLock::new(wal_writer)))
    }

    pub(crate) fn open_wal_if_present(
        db_path: &Path,
    ) -> Result<Option<Arc<RwLock<WALWriter>>>, SqliteGraphError> {
        let wal_path = V3WALPaths::wal_file(db_path);
        if !wal_path.exists() {
            return Ok(None);
        }

        let wal_writer = WALWriter::new(wal_path, 1)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to open WAL: {:?}", e)))?;
        Ok(Some(Arc::new(RwLock::new(wal_writer))))
    }

    pub(crate) fn recover_kv_store(db_path: &Path) -> Option<KvStore> {
        let wal_path = V3WALPaths::wal_file(db_path);
        let mut kv_store = KvStore::new();
        let mut recovered = false;

        if wal_path.exists() {
            let mut recovery = crate::backend::native::v3::wal::WALRecovery::new(wal_path);
            if let Ok(count) = recovery.recover_kv(&mut kv_store) {
                recovered = count > 0;
            }
        }

        if !recovered
            && let Ok(found) =
                crate::backend::native::v3::wal::read_kv_checkpoint(db_path, &mut kv_store)
        {
            recovered = found;
        }

        recovered.then_some(kv_store)
    }
}
