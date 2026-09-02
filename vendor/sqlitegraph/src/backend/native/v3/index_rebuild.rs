use super::V3Backend;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};

impl V3Backend {
    pub(super) fn rebuild_indexes(&self) {
        self.kind_index.clear();
        self.name_index.clear();

        let header = self.header.read();
        let node_count = header.node_count;
        drop(header);

        for id in 1..=node_count as i64 {
            if let Ok(Some(record)) = self.get_node_internal(id) {
                let data_bytes = if let Some(inline) = record.data_inline {
                    inline
                } else if let Some(offset) = record.data_external_offset {
                    let actual_data_len = record.data_len
                        & crate::backend::native::v3::node::record::constants::MAX_DATA_LEN;
                    let mut buffer = vec![0u8; actual_data_len as usize];
                    if let Ok(mut file) = OpenOptions::new().read(true).open(&self.db_path)
                        && file.seek(SeekFrom::Start(offset)).is_ok()
                    {
                        let _ = file.read_exact(&mut buffer);
                    }
                    buffer
                } else {
                    Vec::new()
                };

                let (kind, name, _data) = Self::parse_node_data(&data_bytes, id);
                self.kind_index.insert(kind, id);
                self.name_index.insert(name, id);
            }
        }
    }
}
