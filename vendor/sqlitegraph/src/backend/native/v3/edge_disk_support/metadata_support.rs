use super::*;

const EDGE_METADATA_MAGIC: &[u8; 8] = b"V3EDGE\x00\x00";
const EDGE_METADATA_SIZE: usize = 24;

pub(super) fn persist_btree_metadata(store: &V3EdgeStore) -> NativeResult<()> {
    let meta_path = match store.metadata_path() {
        Some(p) => p,
        None => return Ok(()),
    };

    let btree = store.btree.read();
    let root_page_id = btree.root_page_id();
    let tree_height = btree.tree_height();

    let mut data = Vec::with_capacity(EDGE_METADATA_SIZE);
    data.extend_from_slice(EDGE_METADATA_MAGIC);
    data.extend_from_slice(&root_page_id.to_le_bytes());
    data.extend_from_slice(&tree_height.to_le_bytes());

    let checksum: u32 = data.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
    data.extend_from_slice(&checksum.to_le_bytes());

    std::fs::write(&meta_path, &data).map_err(|e| NativeBackendError::IoError {
        context: format!("Failed to write edge metadata: {}", meta_path.display()),
        source: e,
    })?;

    Ok(())
}

pub(super) fn recover_btree_metadata(store: &V3EdgeStore) -> NativeResult<Option<(u64, u32)>> {
    let meta_path = match store.metadata_path() {
        Some(p) => p,
        None => return Ok(None),
    };

    if !meta_path.exists() {
        return Ok(None);
    }

    let data = std::fs::read(&meta_path).map_err(|e| NativeBackendError::IoError {
        context: format!("Failed to read edge metadata: {}", meta_path.display()),
        source: e,
    })?;

    if data.len() < EDGE_METADATA_SIZE {
        return Ok(None);
    }

    if &data[0..8] != EDGE_METADATA_MAGIC {
        return Ok(None);
    }

    let root_page_id = u64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);
    let tree_height = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);

    let stored_checksum = u32::from_le_bytes([data[20], data[21], data[22], data[23]]);
    let computed_checksum: u32 = data[..20]
        .iter()
        .fold(0u32, |acc, &b| acc.wrapping_add(b as u32));

    if stored_checksum != computed_checksum {
        return Ok(None);
    }

    Ok(Some((root_page_id, tree_height)))
}

pub(super) fn write_page_to_disk(
    store: &V3EdgeStore,
    db_path: &Path,
    page_id: u64,
    data: &[u8],
) -> NativeResult<()> {
    #[cfg(feature = "v3-forensics")]
    {
        use crate::backend::native::v3::constants::V3_HEADER_SIZE;
        let offset: u64 = if page_id == 0 {
            0
        } else {
            V3_HEADER_SIZE + (page_id - 1) * (store.page_size as u64)
        };
        crate::track_page_alloc!(page_id, Subsystem::EdgeStore, ForensicPageType::Edge);
        crate::track_page_write!(
            page_id,
            Subsystem::EdgeStore,
            ForensicPageType::Edge,
            offset,
            "EdgeStore::write_page_to_disk"
        );
    }

    if let Some(ref coordinator) = store.file_coordinator {
        let page_data = if data.len() < store.page_size as usize {
            let mut padded = data.to_vec();
            padded.resize(store.page_size as usize, 0);
            padded
        } else {
            data.to_vec()
        };
        return coordinator.write_page(page_id, &page_data);
    }

    use crate::backend::native::v3::constants::V3_HEADER_SIZE;

    let offset: u64 = if page_id == 0 {
        0
    } else {
        V3_HEADER_SIZE + (page_id - 1) * (store.page_size as u64)
    };

    let file_exists = db_path.exists();
    let mut file = OpenOptions::new()
        .write(true)
        .create(!file_exists)
        .open(db_path)
        .map_err(|e| NativeBackendError::IoError {
            context: format!("Failed to open db file for page write: {}", page_id),
            source: e,
        })?;

    let required_len = offset + data.len() as u64;
    let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if required_len > current_len {
        file.set_len(required_len)
            .map_err(|e| NativeBackendError::IoError {
                context: format!(
                    "Failed to extend file to {} bytes for page {}",
                    required_len, page_id
                ),
                source: e,
            })?;
    }

    file.seek(SeekFrom::Start(offset))
        .map_err(|e| NativeBackendError::IoError {
            context: format!("Failed to seek to page {} offset {}", page_id, offset),
            source: e,
        })?;

    file.write_all(data)
        .map_err(|e| NativeBackendError::IoError {
            context: format!("Failed to write page {} data", page_id),
            source: e,
        })?;

    file.sync_data().map_err(|e| NativeBackendError::IoError {
        context: format!("Failed to sync page {} write", page_id),
        source: e,
    })?;

    Ok(())
}
