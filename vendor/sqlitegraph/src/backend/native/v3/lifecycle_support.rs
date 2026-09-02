use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::native::v3::btree::BTreeManager;
use crate::backend::native::v3::storage::AdaptivePageManager;
use crate::backend::native::v3::{
    FileCoordinator, KindIndex, NodeStore, PageAllocator, PersistentHeaderV3, V3_HEADER_SIZE,
    V3EdgeStore,
};
use parking_lot::RwLock;
use rusqlite::Connection;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

#[path = "lifecycle_support/assembly_support.rs"]
mod assembly_support;

impl V3Backend {
    /// Create a new V3 database file at the specified path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path where the database file will be created
    ///
    /// # Returns
    ///
    /// * `Ok(V3Backend)` - Newly created backend
    /// * `Err(SqliteGraphError)` - If file creation or initialization fails
    ///
    /// # Example
    ///
    /// ```ignore
    /// let backend = V3Backend::create("/path/to/db.graph")?;
    /// ```
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self, SqliteGraphError> {
        let db_path = path.as_ref().to_path_buf();
        Self::validate_base_path(&db_path)?;

        let mut adaptive_manager = AdaptivePageManager::new(&db_path);
        let page_config = adaptive_manager.get_config();

        let mut header = PersistentHeaderV3::new_v3();
        header.page_size = page_config.page_size;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&db_path)
            .map_err(|e| {
                SqliteGraphError::connection(format!("Failed to create database file: {}", e))
            })?;

        let header_bytes = header.to_bytes();
        file.write_all(&header_bytes)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to write header: {}", e)))?;
        file.sync_all()
            .map_err(|e| SqliteGraphError::connection(format!("Failed to sync file: {}", e)))?;

        let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
        let node_btree = BTreeManager::new(Arc::clone(&allocator), None, db_path.clone());
        let edge_btree = BTreeManager::new(Arc::clone(&allocator), None, db_path.clone());
        let mut node_store = NodeStore::new(&header, db_path.clone());
        node_store.initialize(node_btree.clone(), Arc::clone(&allocator), None);
        let max_node_id = node_store
            .node_ids()
            .map_err(super::map_v3_error)?
            .into_iter()
            .max()
            .unwrap_or(0);
        node_store.set_next_node_id(max_node_id + 1);
        let edge_store = V3EdgeStore::with_path_and_allocator(
            edge_btree,
            None,
            db_path.clone(),
            Arc::clone(&allocator),
            header.page_size,
        );

        let coordinator = Arc::new(FileCoordinator::create(&db_path).map_err(super::map_v3_error)?);
        node_store.set_file_coordinator(Arc::clone(&coordinator));
        let mut edge_store = edge_store;
        edge_store.set_file_coordinator(Arc::clone(&coordinator));

        let sqlite_path = Self::sqlite_sidecar_path(&db_path)?;
        let sqlite_conn = Connection::open(sqlite_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to open SQLite connection: {}", e))
        })?;
        Self::init_sqlite_schema(&sqlite_conn)?;

        Ok(Self::assemble_backend(
            db_path,
            coordinator,
            sqlite_conn,
            node_btree,
            node_store,
            edge_store,
            allocator,
            None,
            header,
            KindIndex::new(),
            super::NameIndex::new(),
        ))
    }

    /// Create a new V3 database with WAL enabled
    pub fn create_with_wal<P: AsRef<Path>>(
        path: P,
        enable_wal: bool,
    ) -> Result<Self, SqliteGraphError> {
        let mut backend = Self::create(path)?;

        if enable_wal {
            backend.wal = Some(Self::create_wal_writer(&backend.db_path)?);
        }

        Ok(backend)
    }

    /// Open an existing V3 database from the specified path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, SqliteGraphError> {
        let db_path = path.as_ref().to_path_buf();
        Self::validate_base_path(&db_path)?;

        if !db_path.exists() {
            return Err(SqliteGraphError::connection(format!(
                "Database file does not exist: {}",
                db_path.display()
            )));
        }

        let mut file = File::open(&db_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to open database file: {}", e))
        })?;

        let mut header_bytes = vec![0u8; V3_HEADER_SIZE as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|e| SqliteGraphError::connection(format!("Failed to read header: {}", e)))?;

        let header = PersistentHeaderV3::from_bytes(&header_bytes).map_err(super::map_v3_error)?;
        header.validate().map_err(super::map_v3_error)?;

        let allocator = Arc::new(RwLock::new(PageAllocator::new(&header)));
        let btree = BTreeManager::with_root(
            Arc::clone(&allocator),
            None,
            header.root_index_page,
            header.btree_height,
            db_path.clone(),
        );
        let mut node_store = NodeStore::new(&header, db_path.clone());
        node_store.initialize(
            BTreeManager::with_root(
                Arc::clone(&allocator),
                None,
                header.root_index_page,
                header.btree_height,
                db_path.clone(),
            ),
            Arc::clone(&allocator),
            None,
        );
        let max_node_id = node_store
            .node_ids()
            .map_err(super::map_v3_error)?
            .into_iter()
            .max()
            .unwrap_or(0);
        node_store.set_next_node_id(max_node_id + 1);
        let mut edge_store = V3EdgeStore::with_path_and_allocator(
            BTreeManager::with_root(
                Arc::clone(&allocator),
                None,
                header.edge_data_offset,
                header.reserved,
                db_path.clone(),
            ),
            None,
            db_path.clone(),
            Arc::clone(&allocator),
            header.page_size,
        );

        let sqlite_path = Self::sqlite_sidecar_path(&db_path)?;
        let sqlite_conn = Connection::open(sqlite_path).map_err(|e| {
            SqliteGraphError::connection(format!("Failed to open SQLite connection: {}", e))
        })?;

        let _ = edge_store.restore_btree_from_metadata();

        let wal = Self::open_wal_if_present(&db_path)?;

        if let Some(ref wal_arc) = wal {
            edge_store.set_wal(Arc::clone(wal_arc));
        }

        let coordinator = Arc::new(FileCoordinator::create(&db_path).map_err(super::map_v3_error)?);
        node_store.set_file_coordinator(Arc::clone(&coordinator));
        edge_store.set_file_coordinator(Arc::clone(&coordinator));

        let (kind_index, name_index) =
            match crate::backend::native::v3::index_persistence::restore_indexes(
                &db_path,
                header.node_count,
            ) {
                Ok((kind, name)) => (kind, name),
                Err(_) => (KindIndex::new(), super::NameIndex::new()),
            };

        let backend = Self::assemble_backend(
            db_path.clone(),
            coordinator,
            sqlite_conn,
            btree,
            node_store,
            edge_store,
            allocator,
            wal,
            header,
            kind_index,
            name_index,
        );

        if backend.kind_index.kind_count() == 0 && backend.header.read().node_count > 0 {
            backend.rebuild_indexes();
        }

        if let Some(kv_store) = Self::recover_kv_store(&db_path) {
            let mut kv_guard = backend.kv_store.write();
            *kv_guard = Some(kv_store);
        }

        Ok(backend)
    }
}
