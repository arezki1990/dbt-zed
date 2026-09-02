use super::writer::V3WALPaths;
use crate::backend::native::{NativeBackendError, NativeResult};

pub fn write_kv_checkpoint(
    db_path: &std::path::Path,
    kv_store: &super::super::kv_store::store::KvStore,
) -> NativeResult<()> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let checkpoint_path = V3WALPaths::checkpoint_file(db_path);
    let temp_path = V3WALPaths::temp_checkpoint_file(db_path);
    let checkpoint_data = kv_store.to_bytes();

    let mut hasher = Sha256::new();
    hasher.update(&checkpoint_data);
    let checksum: [u8; 32] = hasher.finalize().into();

    let mut file = std::fs::File::create(&temp_path).map_err(|e| NativeBackendError::IoError {
        context: format!(
            "Failed to create temp checkpoint file: {}",
            temp_path.display()
        ),
        source: e,
    })?;

    let magic: [u8; 8] = [b'V', b'3', b'K', b'V', b'C', b'K', 0, 3];
    file.write_all(&magic)
        .map_err(|e| NativeBackendError::IoError {
            context: "Failed to write checkpoint magic".to_string(),
            source: e,
        })?;

    let version: u32 = 3;
    file.write_all(&version.to_le_bytes())
        .map_err(|e| NativeBackendError::IoError {
            context: "Failed to write checkpoint version".to_string(),
            source: e,
        })?;

    let data_len: u32 = checkpoint_data.len().try_into().unwrap_or(u32::MAX);
    file.write_all(&data_len.to_le_bytes())
        .map_err(|e| NativeBackendError::IoError {
            context: "Failed to write checkpoint data length".to_string(),
            source: e,
        })?;

    file.write_all(&checksum)
        .map_err(|e| NativeBackendError::IoError {
            context: "Failed to write checkpoint checksum".to_string(),
            source: e,
        })?;

    file.write_all(&checkpoint_data)
        .map_err(|e| NativeBackendError::IoError {
            context: "Failed to write checkpoint data".to_string(),
            source: e,
        })?;

    file.flush().map_err(|e| NativeBackendError::IoError {
        context: "Failed to flush checkpoint file".to_string(),
        source: e,
    })?;
    file.sync_all().map_err(|e| NativeBackendError::IoError {
        context: "Failed to sync checkpoint file".to_string(),
        source: e,
    })?;

    std::fs::rename(&temp_path, &checkpoint_path).map_err(|e| NativeBackendError::IoError {
        context: format!(
            "Failed to rename checkpoint file: {} -> {}",
            temp_path.display(),
            checkpoint_path.display()
        ),
        source: e,
    })?;

    Ok(())
}

pub fn read_kv_checkpoint(
    db_path: &std::path::Path,
    kv_store: &mut super::super::kv_store::store::KvStore,
) -> NativeResult<bool> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let checkpoint_path = V3WALPaths::checkpoint_file(db_path);
    if !checkpoint_path.exists() {
        return Ok(false);
    }

    let mut file =
        std::fs::File::open(&checkpoint_path).map_err(|e| NativeBackendError::IoError {
            context: format!(
                "Failed to open checkpoint file: {}",
                checkpoint_path.display()
            ),
            source: e,
        })?;

    let mut magic_bytes = [0u8; 8];
    file.read_exact(&mut magic_bytes).map_err(|e| {
        cleanup_corrupt_checkpoint(&checkpoint_path);
        NativeBackendError::IoError {
            context: "Failed to read checkpoint magic".to_string(),
            source: e,
        }
    })?;

    if magic_bytes[0..6] != [b'V', b'3', b'K', b'V', b'C', b'K'] {
        cleanup_corrupt_checkpoint(&checkpoint_path);
        return Err(NativeBackendError::InvalidHeader {
            field: "magic".to_string(),
            reason: format!("checkpoint magic prefix mismatch: got {:?}", magic_bytes),
        });
    }

    let has_checksum = magic_bytes[7] >= 2;

    let mut version_bytes = [0u8; 4];
    file.read_exact(&mut version_bytes).map_err(|e| {
        cleanup_corrupt_checkpoint(&checkpoint_path);
        NativeBackendError::IoError {
            context: "Failed to read checkpoint version".to_string(),
            source: e,
        }
    })?;
    let _version = u32::from_le_bytes(version_bytes);

    let mut len_bytes = [0u8; 4];
    file.read_exact(&mut len_bytes).map_err(|e| {
        cleanup_corrupt_checkpoint(&checkpoint_path);
        NativeBackendError::IoError {
            context: "Failed to read checkpoint data length".to_string(),
            source: e,
        }
    })?;
    let data_len = u32::from_le_bytes(len_bytes) as usize;

    let stored_checksum: Option<[u8; 32]> = if has_checksum {
        let mut checksum_bytes = [0u8; 32];
        file.read_exact(&mut checksum_bytes).map_err(|e| {
            cleanup_corrupt_checkpoint(&checkpoint_path);
            NativeBackendError::IoError {
                context: "Failed to read checkpoint checksum".to_string(),
                source: e,
            }
        })?;
        Some(checksum_bytes)
    } else {
        None
    };

    let mut checkpoint_data = vec![0u8; data_len];
    file.read_exact(&mut checkpoint_data).map_err(|e| {
        cleanup_corrupt_checkpoint(&checkpoint_path);
        NativeBackendError::IoError {
            context: "Failed to read checkpoint data".to_string(),
            source: e,
        }
    })?;

    if let Some(stored) = stored_checksum {
        let mut hasher = Sha256::new();
        hasher.update(&checkpoint_data);
        let calculated: [u8; 32] = hasher.finalize().into();

        if calculated != stored {
            cleanup_corrupt_checkpoint(&checkpoint_path);
            return Err(NativeBackendError::InvalidHeader {
                field: "checksum".to_string(),
                reason: "checkpoint checksum mismatch - data may be corrupt".to_string(),
            });
        }
    }

    kv_store
        .from_bytes(&checkpoint_data, magic_bytes[7])
        .map_err(|e| {
            cleanup_corrupt_checkpoint(&checkpoint_path);
            NativeBackendError::InvalidHeader {
                field: "checkpoint_data".to_string(),
                reason: format!("Failed to deserialize checkpoint: {}", e),
            }
        })?;

    Ok(true)
}

fn cleanup_corrupt_checkpoint(checkpoint_path: &std::path::Path) {
    let _ = std::fs::remove_file(checkpoint_path);
    eprintln!(
        "WARNING: Deleted corrupt KV checkpoint file: {}",
        checkpoint_path.display()
    );
}
