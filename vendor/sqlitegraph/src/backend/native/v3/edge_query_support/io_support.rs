use super::*;

impl V3EdgeStore {
    pub(crate) fn read_page_from_disk(
        &self,
        file: &mut std::fs::File,
        db_path: &Path,
        page_id: u64,
    ) -> NativeResult<Vec<u8>> {
        use crate::backend::native::v3::constants::V3_HEADER_SIZE;
        use std::io::{Read, Seek, SeekFrom};

        let offset = V3_HEADER_SIZE + (page_id - 1) * (self.page_size as u64);
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| NativeBackendError::IoError {
                context: format!("Failed to seek to edge page {} offset {}", page_id, offset),
                source: e,
            })?;

        let mut buffer = vec![0u8; self.page_size as usize];
        file.read_exact(&mut buffer)
            .map_err(|e| NativeBackendError::IoError {
                context: format!(
                    "Failed to read edge page {} from {}",
                    page_id,
                    db_path.display()
                ),
                source: e,
            })?;
        Ok(buffer)
    }

    /// Create new edge store (in-memory only)
    /// NOTE: Prefer with_path_and_allocator() for database-backed edge stores
    pub fn new(
        btree: BTreeManager,
        wal: Option<WALWriter>,
        allocator: Arc<RwLock<PageAllocator>>,
        page_size: u32,
    ) -> Self {
        Self {
            btree: Arc::new(parking_lot::RwLock::new(btree)),
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_weighted: Arc::new(RwLock::new(HashMap::new())),
            edge_types: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            hit_time_ns: AtomicU64::new(0),
            miss_time_ns: AtomicU64::new(0),
            dirty_clusters: RwLock::new(HashMap::new()),
            db_path: None,
            allocator,
            page_size,
            file_coordinator: None,
        }
    }

    /// Create new edge store with database path and allocator
    /// This is the preferred constructor for database-backed edge stores
    pub fn with_path_and_allocator(
        btree: BTreeManager,
        wal: Option<WALWriter>,
        db_path: PathBuf,
        allocator: Arc<RwLock<PageAllocator>>,
        page_size: u32,
    ) -> Self {
        Self {
            btree: Arc::new(parking_lot::RwLock::new(btree)),
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_weighted: Arc::new(RwLock::new(HashMap::new())),
            edge_types: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            hit_time_ns: AtomicU64::new(0),
            miss_time_ns: AtomicU64::new(0),
            dirty_clusters: RwLock::new(HashMap::new()),
            db_path: Some(db_path),
            allocator,
            page_size,
            file_coordinator: None,
        }
    }

    /// Create new edge store with disk persistence path
    /// NOTE: This creates a standalone allocator for backward compatibility.
    /// For proper page allocation, use with_path_and_allocator() instead.
    pub fn with_path(btree: BTreeManager, wal: Option<WALWriter>, db_path: PathBuf) -> Self {
        let header = PersistentHeaderV3::new_v3();
        Self {
            btree: Arc::new(parking_lot::RwLock::new(btree)),
            wal: wal.map(|w| Arc::new(RwLock::new(w))),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_weighted: Arc::new(RwLock::new(HashMap::new())),
            edge_types: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            hit_time_ns: AtomicU64::new(0),
            miss_time_ns: AtomicU64::new(0),
            dirty_clusters: RwLock::new(HashMap::new()),
            db_path: Some(db_path),
            allocator: Arc::new(RwLock::new(PageAllocator::new(&header))),
            page_size: header.page_size,
            file_coordinator: None,
        }
    }
}

pub(super) fn weighted_neighbors_from_cluster(
    store: &V3EdgeStore,
    src: i64,
    dir: Direction,
    cluster: &V3EdgeCluster,
) -> Arc<[(i64, f32)]> {
    let mut edge_types = store.edge_types.write();
    let mut neighbors = Vec::with_capacity(cluster.edges.len());
    for e in &cluster.edges {
        let edge_type = V3EdgeCluster::extract_edge_type(&e.edge_data);
        if let Some(et) = edge_type {
            edge_types.insert((src, e.neighbor_id, dir), et);
        }
        let weight = V3EdgeCluster::extract_edge_weight(&e.edge_data);
        neighbors.push((e.neighbor_id, weight));
    }
    neighbors.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    Arc::from(neighbors.into_boxed_slice())
}

pub(super) fn unweighted_neighbors_from_weighted(neighbors: &[(i64, f32)]) -> Arc<[i64]> {
    let dsts: Vec<i64> = neighbors.iter().map(|(dst, _)| *dst).collect();
    Arc::from(dsts.into_boxed_slice())
}
