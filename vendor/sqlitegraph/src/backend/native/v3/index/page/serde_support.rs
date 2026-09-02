use super::*;

/// Helper function to extract a fixed-size byte array from a slice.
fn extract_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
    field: &str,
) -> NativeResult<[u8; N]> {
    let end = offset + N;
    if end > bytes.len() {
        return Err(NativeBackendError::InvalidHeader {
            field: field.to_string(),
            reason: format!(
                "insufficient bytes at offset {}: need {}, have {}",
                offset,
                N,
                bytes.len()
            ),
        });
    }

    let mut arr = [0u8; N];
    arr.copy_from_slice(&bytes[offset..end]);
    Ok(arr)
}

impl IndexPage {
    /// Calculate checksum for page data
    fn calculate_checksum(&self, data: &[u8]) -> u32 {
        v3_constants::checksum::xor_checksum(data) as u32
    }

    /// Verify B+Tree invariants (debug builds only)
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) -> NativeResult<()> {
        match self {
            IndexPage::Internal {
                keys,
                children,
                is_root,
                ..
            } => {
                if keys.len() > MAX_KEYS {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_verify".to_string(),
                        reason: format!(
                            "internal node has {} keys, max is {}",
                            keys.len(),
                            MAX_KEYS
                        ),
                    });
                }

                if !is_root && !keys.is_empty() && keys.len() < MIN_KEYS {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_verify".to_string(),
                        reason: format!(
                            "non-root internal node has {} keys, min is {}",
                            keys.len(),
                            MIN_KEYS
                        ),
                    });
                }

                for i in 1..keys.len() {
                    if keys[i - 1] >= keys[i] {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "btree_verify".to_string(),
                            reason: format!("keys out of order: {} >= {}", keys[i - 1], keys[i]),
                        });
                    }
                }

                if !keys.is_empty() && children.len() != keys.len() + 1 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_verify".to_string(),
                        reason: format!(
                            "children count ({}) != keys count ({}) + 1",
                            children.len(),
                            keys.len()
                        ),
                    });
                }
            }
            IndexPage::Leaf {
                entries, is_root, ..
            } => {
                if entries.len() > MAX_ENTRIES {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_verify".to_string(),
                        reason: format!(
                            "leaf node has {} entries, max is {}",
                            entries.len(),
                            MAX_ENTRIES
                        ),
                    });
                }

                if !is_root && !entries.is_empty() && entries.len() < MIN_ENTRIES {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "btree_verify".to_string(),
                        reason: format!(
                            "non-root leaf node has {} entries, min is {}",
                            entries.len(),
                            MIN_ENTRIES
                        ),
                    });
                }

                for i in 1..entries.len() {
                    if entries[i - 1].0 >= entries[i].0 {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "btree_verify".to_string(),
                            reason: format!(
                                "entries out of order: {} >= {}",
                                entries[i - 1].0,
                                entries[i].0
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Pack the page into a 4KB byte array.
    pub fn pack(&self) -> NativeResult<[u8; 4096]> {
        let mut bytes = [0u8; 4096];

        bytes[constants::PAGE_ID_OFFSET..constants::PAGE_ID_OFFSET + 8]
            .copy_from_slice(&self.page_id().to_be_bytes());

        match self {
            IndexPage::Internal { .. } => bytes[constants::IS_LEAF_OFFSET] = 0,
            IndexPage::Leaf { .. } => bytes[constants::IS_LEAF_OFFSET] = 1,
        }

        bytes[constants::IS_ROOT_OFFSET] = if self.is_root() { 1 } else { 0 };

        let count = self.count() as u16;
        bytes[constants::COUNT_OFFSET..constants::COUNT_OFFSET + 2]
            .copy_from_slice(&count.to_be_bytes());

        let checksum_offset = constants::CHECKSUM_OFFSET;
        let mut data_offset = constants::DATA_START_OFFSET;

        match self {
            IndexPage::Internal { keys, children, .. } => {
                if !keys.is_empty() && children.len() != keys.len() + 1 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "internal_page".to_string(),
                        reason: format!(
                            "children count ({}) must be keys count ({}) + 1",
                            children.len(),
                            keys.len()
                        ),
                    });
                }
                if keys.is_empty() && children.len() > 1 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "internal_page".to_string(),
                        reason: format!("empty page has too many children: {}", children.len()),
                    });
                }
                if keys.len() > MAX_KEYS {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "internal_page".to_string(),
                        reason: format!("keys count ({}) exceeds max ({})", keys.len(), MAX_KEYS),
                    });
                }

                for &key in keys {
                    if data_offset + KEY_SIZE > 4096 {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "internal_page".to_string(),
                            reason: "page overflow writing keys".to_string(),
                        });
                    }
                    bytes[data_offset..data_offset + KEY_SIZE].copy_from_slice(&key.to_be_bytes());
                    data_offset += KEY_SIZE;
                }

                for &child in children {
                    if data_offset + PAGE_ID_SIZE > 4096 {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "internal_page".to_string(),
                            reason: "page overflow writing children".to_string(),
                        });
                    }
                    bytes[data_offset..data_offset + PAGE_ID_SIZE]
                        .copy_from_slice(&child.to_be_bytes());
                    data_offset += PAGE_ID_SIZE;
                }
            }
            IndexPage::Leaf {
                entries, next_leaf, ..
            } => {
                if entries.len() > MAX_ENTRIES {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "leaf_page".to_string(),
                        reason: format!(
                            "entries count ({}) exceeds max ({})",
                            entries.len(),
                            MAX_ENTRIES
                        ),
                    });
                }

                for &(node_id, page_id) in entries {
                    if data_offset + ENTRY_SIZE > 4096 {
                        return Err(NativeBackendError::InvalidHeader {
                            field: "leaf_page".to_string(),
                            reason: "page overflow writing entries".to_string(),
                        });
                    }
                    bytes[data_offset..data_offset + KEY_SIZE]
                        .copy_from_slice(&node_id.to_be_bytes());
                    data_offset += KEY_SIZE;
                    bytes[data_offset..data_offset + PAGE_ID_SIZE]
                        .copy_from_slice(&page_id.to_be_bytes());
                    data_offset += PAGE_ID_SIZE;
                }

                if data_offset + PAGE_ID_SIZE > 4096 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "leaf_page".to_string(),
                        reason: "page overflow writing next_leaf".to_string(),
                    });
                }
                bytes[data_offset..data_offset + PAGE_ID_SIZE]
                    .copy_from_slice(&next_leaf.to_be_bytes());
                data_offset += PAGE_ID_SIZE;
            }
        }

        let checksum = self.calculate_checksum(&bytes[..data_offset]);
        bytes[checksum_offset..checksum_offset + 4].copy_from_slice(&checksum.to_be_bytes());

        Ok(bytes)
    }

    /// Unpack a page from a byte array.
    pub fn unpack(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < 4096 {
            return Err(NativeBackendError::InvalidHeader {
                field: "page_data".to_string(),
                reason: format!("insufficient bytes: expected 4096, found {}", bytes.len()),
            });
        }

        let page_id =
            u64::from_be_bytes(extract_array(bytes, constants::PAGE_ID_OFFSET, "page_id")?);
        let is_leaf = bytes[constants::IS_LEAF_OFFSET] == 1;
        let is_root = bytes[constants::IS_ROOT_OFFSET] == 1;
        let count =
            u16::from_be_bytes(extract_array(bytes, constants::COUNT_OFFSET, "count")?) as usize;
        let checksum = u32::from_be_bytes(extract_array(
            bytes,
            constants::CHECKSUM_OFFSET,
            "checksum",
        )?);

        let mut data_offset = constants::DATA_START_OFFSET;

        if is_leaf {
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                if data_offset + ENTRY_SIZE > 4096 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "leaf_page".to_string(),
                        reason: "page overflow reading entries".to_string(),
                    });
                }
                let node_id = u64::from_be_bytes(extract_array(bytes, data_offset, "node_id")?);
                data_offset += KEY_SIZE;
                let page_id = u64::from_be_bytes(extract_array(bytes, data_offset, "page_id")?);
                data_offset += PAGE_ID_SIZE;
                entries.push((node_id, page_id));
            }

            let next_leaf = if data_offset + PAGE_ID_SIZE <= 4096 {
                u64::from_be_bytes(extract_array(bytes, data_offset, "next_leaf")?)
            } else {
                return Err(NativeBackendError::InvalidHeader {
                    field: "leaf_page".to_string(),
                    reason: "missing next_leaf pointer".to_string(),
                });
            };

            let calculated_checksum =
                Self::calculate_checksum_leaf(page_id, &entries, next_leaf, is_root);
            if calculated_checksum != checksum {
                return Err(NativeBackendError::InvalidHeader {
                    field: "leaf_checksum".to_string(),
                    reason: format!(
                        "checksum mismatch: expected {}, found {}",
                        calculated_checksum, checksum
                    ),
                });
            }

            Ok(IndexPage::Leaf {
                page_id,
                entries,
                next_leaf,
                checksum,
                is_root,
            })
        } else {
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                if data_offset + KEY_SIZE > 4096 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "internal_page".to_string(),
                        reason: "page overflow reading keys".to_string(),
                    });
                }
                let key = u64::from_be_bytes(extract_array(bytes, data_offset, "key")?);
                data_offset += KEY_SIZE;
                keys.push(key);
            }

            let child_count = count + 1;
            let mut children = Vec::with_capacity(child_count);
            for _ in 0..child_count {
                if data_offset + PAGE_ID_SIZE > 4096 {
                    return Err(NativeBackendError::InvalidHeader {
                        field: "internal_page".to_string(),
                        reason: "page overflow reading children".to_string(),
                    });
                }
                let child = u64::from_be_bytes(extract_array(bytes, data_offset, "child")?);
                data_offset += PAGE_ID_SIZE;
                children.push(child);
            }

            let calculated_checksum =
                Self::calculate_checksum_internal(page_id, &keys, &children, is_root);
            if calculated_checksum != checksum {
                return Err(NativeBackendError::InvalidHeader {
                    field: "internal_checksum".to_string(),
                    reason: format!(
                        "checksum mismatch: expected {}, found {}",
                        calculated_checksum, checksum
                    ),
                });
            }

            Ok(IndexPage::Internal {
                page_id,
                keys,
                children,
                checksum,
                is_root,
            })
        }
    }

    fn calculate_checksum_leaf(
        page_id: u64,
        entries: &[(u64, u64)],
        next_leaf: u64,
        is_root: bool,
    ) -> u32 {
        let mut data = Vec::with_capacity(4096);
        data.extend_from_slice(&page_id.to_be_bytes());
        data.push(1);
        data.push(if is_root { 1 } else { 0 });
        data.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        data.extend_from_slice(&[0u8; 4]);
        data.extend_from_slice(&[0u8; 16]);

        for &(node_id, page_id) in entries {
            data.extend_from_slice(&node_id.to_be_bytes());
            data.extend_from_slice(&page_id.to_be_bytes());
        }
        data.extend_from_slice(&next_leaf.to_be_bytes());

        v3_constants::checksum::xor_checksum(&data) as u32
    }

    fn calculate_checksum_internal(
        page_id: u64,
        keys: &[u64],
        children: &[u64],
        is_root: bool,
    ) -> u32 {
        let mut data = Vec::with_capacity(4096);
        data.extend_from_slice(&page_id.to_be_bytes());
        data.push(0);
        data.push(if is_root { 1 } else { 0 });
        data.extend_from_slice(&(keys.len() as u16).to_be_bytes());
        data.extend_from_slice(&[0u8; 4]);
        data.extend_from_slice(&[0u8; 16]);

        for &key in keys {
            data.extend_from_slice(&key.to_be_bytes());
        }
        for &child in children {
            data.extend_from_slice(&child.to_be_bytes());
        }

        v3_constants::checksum::xor_checksum(&data) as u32
    }
}
