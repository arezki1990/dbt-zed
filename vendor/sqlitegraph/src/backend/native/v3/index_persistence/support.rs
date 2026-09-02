use super::*;

pub(super) fn write_all<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    field: &str,
) -> Result<(), IndexPersistenceError> {
    writer
        .write_all(bytes)
        .map_err(|e| IndexPersistenceError::Io(format!("Failed to write {}: {}", field, e)))
}

pub(super) fn write_string_entry<W: Write>(
    writer: &mut W,
    value: &str,
    node_ids: &[i64],
    field: &str,
) -> Result<(), IndexPersistenceError> {
    let value_bytes = value.as_bytes();
    write_all(
        writer,
        &(value_bytes.len() as u32).to_be_bytes(),
        &format!("{} len", field),
    )?;
    write_all(writer, value_bytes, &format!("{} bytes", field))?;
    write_all(writer, &(node_ids.len() as u32).to_be_bytes(), "node count")?;
    for &node_id in node_ids {
        write_all(writer, &node_id.to_be_bytes(), "node ID")?;
    }
    Ok(())
}

pub(super) fn read_array<const N: usize, R: Read>(
    reader: &mut R,
    field: &str,
) -> Result<[u8; N], IndexPersistenceError> {
    let mut bytes = [0u8; N];
    reader.read_exact(&mut bytes).map_err(|e| {
        IndexPersistenceError::Corrupted(format!("Failed to read {}: {}", field, e))
    })?;
    Ok(bytes)
}

pub(super) struct SliceCursor<'a> {
    remaining: &'a [u8],
}

impl<'a> SliceCursor<'a> {
    pub(super) fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }

    fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], IndexPersistenceError> {
        if self.remaining.len() < count {
            return Err(IndexPersistenceError::Corrupted(format!(
                "Unexpected EOF: needed {} bytes, got {}",
                count,
                self.remaining.len()
            )));
        }
        let (head, tail) = self.remaining.split_at(count);
        self.remaining = tail;
        Ok(head)
    }

    pub(super) fn read_u32(&mut self) -> Result<u32, IndexPersistenceError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .expect("invariant: read_bytes guaranteed 4 bytes"),
        ))
    }

    fn read_i64(&mut self) -> Result<i64, IndexPersistenceError> {
        let bytes = self.read_bytes(8)?;
        Ok(i64::from_be_bytes(
            bytes
                .try_into()
                .expect("invariant: read_bytes guaranteed 8 bytes"),
        ))
    }

    fn read_string(&mut self, field: &str) -> Result<String, IndexPersistenceError> {
        let len = self.read_u32()? as usize;
        let bytes = self.read_bytes(len)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|_| IndexPersistenceError::Corrupted(format!("Invalid UTF-8 in {}", field)))?;
        Ok(value.to_string())
    }
}

pub(super) fn read_string_index_entries(
    cursor: &mut SliceCursor<'_>,
    entry_count: usize,
    field: &str,
) -> Result<HashMap<String, Vec<i64>>, IndexPersistenceError> {
    let mut data = HashMap::new();
    for _ in 0..entry_count {
        let key = cursor.read_string(field)?;
        let node_count = cursor.read_u32()? as usize;
        let mut node_ids = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            node_ids.push(cursor.read_i64()?);
        }
        data.insert(key, node_ids);
    }
    Ok(data)
}
