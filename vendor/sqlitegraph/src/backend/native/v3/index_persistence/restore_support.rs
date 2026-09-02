use super::*;

pub(super) fn restore_indexes(
    db_path: &Path,
    db_node_count: u64,
) -> Result<(KindIndex, NameIndex), IndexPersistenceError> {
    let index_path = super::index_path_for_db(db_path);

    #[cfg(feature = "v3-forensics")]
    let file_open_syscall_start = std::time::Instant::now();

    let mut file = std::fs::File::open(&index_path)
        .map_err(|_| IndexPersistenceError::Corrupted("Index file not found".to_string()))?;

    #[cfg(feature = "v3-forensics")]
    let file_open_syscall_elapsed = file_open_syscall_start.elapsed();

    #[cfg(feature = "v3-forensics")]
    let file_open_start = std::time::Instant::now();

    let magic = support::read_array::<4, _>(&mut file, "magic")?;
    if &magic != INDEX_MAGIC {
        return Err(IndexPersistenceError::InvalidMagic(magic.to_vec()));
    }

    let version = u32::from_be_bytes(support::read_array::<4, _>(&mut file, "version")?);
    if version != INDEX_VERSION {
        return Err(IndexPersistenceError::UnsupportedVersion(version));
    }

    let stored_node_count =
        u64::from_be_bytes(support::read_array::<8, _>(&mut file, "stored node count")?);
    if stored_node_count != db_node_count {
        return Err(IndexPersistenceError::Corrupted(format!(
            "Stale index: sidecar node_count {} != DB node_count {}",
            stored_node_count, db_node_count
        )));
    }

    #[cfg(feature = "v3-forensics")]
    let file_open_elapsed = file_open_start.elapsed();

    #[cfg(feature = "v3-forensics")]
    let bulk_read_start = std::time::Instant::now();

    let mut remaining_data = Vec::new();
    file.read_to_end(&mut remaining_data).map_err(|e| {
        IndexPersistenceError::Corrupted(format!("Failed to read index data: {}", e))
    })?;

    #[cfg(feature = "v3-forensics")]
    let bulk_read_elapsed = bulk_read_start.elapsed();

    let mut cursor = support::SliceCursor::new(&remaining_data);
    let kind_count = cursor.read_u32()? as usize;

    #[cfg(feature = "v3-forensics")]
    let kind_loop_start = std::time::Instant::now();

    let kind_data = support::read_string_index_entries(&mut cursor, kind_count, "kind")?;

    #[cfg(feature = "v3-forensics")]
    let kind_loop_elapsed = kind_loop_start.elapsed();

    let name_count = cursor.read_u32()? as usize;

    #[cfg(feature = "v3-forensics")]
    let name_loop_start = std::time::Instant::now();

    let name_data = support::read_string_index_entries(&mut cursor, name_count, "name")?;

    #[cfg(feature = "v3-forensics")]
    let name_loop_elapsed = name_loop_start.elapsed();

    #[cfg(feature = "v3-forensics")]
    let index_creation_start = std::time::Instant::now();

    let kind_index = KindIndex::new();
    let name_index = NameIndex::new();

    #[cfg(feature = "v3-forensics")]
    let index_creation_elapsed = index_creation_start.elapsed();

    #[cfg(feature = "v3-forensics")]
    let hashmap_import_start = std::time::Instant::now();

    kind_index.import(kind_data);
    name_index.import(name_data);

    #[cfg(feature = "v3-forensics")]
    let hashmap_import_elapsed = hashmap_import_start.elapsed();

    #[cfg(feature = "v3-forensics")]
    {
        use std::io::Write;
        let _ = std::io::stderr().flush();
        eprintln!("    [restore_indexes breakdown]");
        eprintln!(
            "      - File::open() syscall:    {:.2} µs",
            file_open_syscall_elapsed.as_secs_f64() * 1_000_000.0
        );
        eprintln!(
            "      - read/verify header:      {:.2} µs",
            file_open_elapsed.as_secs_f64() * 1_000_000.0
        );
        eprintln!(
            "      - bulk_read() all data:    {:.2} µs ({:.2} KB)",
            bulk_read_elapsed.as_secs_f64() * 1_000_000.0,
            remaining_data.len() as f64 / 1024.0
        );
        eprintln!(
            "      - kind_loop ({}, entries):   {:.2} µs",
            kind_count,
            kind_loop_elapsed.as_secs_f64() * 1_000_000.0
        );
        eprintln!(
            "      - name_loop ({}, entries):   {:.2} µs",
            name_count,
            name_loop_elapsed.as_secs_f64() * 1_000_000.0
        );
        eprintln!(
            "      - index creation (2 objs):  {:.2} µs",
            index_creation_elapsed.as_secs_f64() * 1_000_000.0
        );
        eprintln!(
            "      - hashmap import:            {:.2} µs",
            hashmap_import_elapsed.as_secs_f64() * 1_000_000.0
        );
        let _ = std::io::stderr().flush();
    }

    Ok((kind_index, name_index))
}
