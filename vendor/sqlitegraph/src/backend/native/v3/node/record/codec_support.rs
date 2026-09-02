use super::*;

impl NodeRecordV3 {
    /// Serialize the node record to bytes.
    pub fn serialize(&self) -> NativeResult<Vec<u8>> {
        let mut buffer = Vec::with_capacity(self.serialized_size());

        buffer.extend_from_slice(&self.id.to_be_bytes());
        buffer.extend_from_slice(&self.flags.0.to_be_bytes());
        buffer.extend_from_slice(&self.kind_offset.to_be_bytes());
        buffer.extend_from_slice(&self.name_offset.to_be_bytes());

        let encoded_data_len = if self.is_external() {
            self.data_len | constants::EXTERNAL_DATA_FLAG
        } else {
            self.data_len
        };
        buffer.extend_from_slice(&encoded_data_len.to_be_bytes());
        buffer.extend_from_slice(&[0u8; 2]);
        buffer.extend_from_slice(&self.outgoing_cluster_offset.to_be_bytes());
        buffer.extend_from_slice(&self.outgoing_edge_count.to_be_bytes());
        buffer.extend_from_slice(&self.incoming_cluster_offset.to_be_bytes());
        buffer.extend_from_slice(&self.incoming_edge_count.to_be_bytes());

        assert_eq!(
            buffer.len(),
            FIXED_METADATA_SIZE,
            "Fixed metadata must be exactly {} bytes",
            FIXED_METADATA_SIZE
        );

        if let Some(ref data) = self.data_inline {
            buffer.extend_from_slice(data);
        } else if let Some(offset) = self.data_external_offset {
            buffer.extend_from_slice(&offset.to_be_bytes());
        }

        Ok(buffer)
    }

    /// Deserialize a node record from bytes.
    pub fn deserialize(bytes: &[u8]) -> NativeResult<Self> {
        if bytes.len() < FIXED_METADATA_SIZE {
            return Err(NativeBackendError::InvalidHeader {
                field: "node_record".to_string(),
                reason: format!(
                    "insufficient bytes: expected at least {}, found {}",
                    FIXED_METADATA_SIZE,
                    bytes.len()
                ),
            });
        }

        let mut offset = 0;

        let id = i64::from_be_bytes(
            bytes[offset..offset + constants::ID_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.id".to_string(),
                    reason: "invalid ID bytes".to_string(),
                })?,
        );
        offset += constants::ID_SIZE;

        let flags = NodeFlags(u32::from_be_bytes(
            bytes[offset..offset + constants::FLAGS_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.flags".to_string(),
                    reason: "invalid flags bytes".to_string(),
                })?,
        ));
        offset += constants::FLAGS_SIZE;

        let kind_offset = u16::from_be_bytes(
            bytes[offset..offset + constants::KIND_OFFSET_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.kind_offset".to_string(),
                    reason: "invalid kind_offset bytes".to_string(),
                })?,
        );
        offset += constants::KIND_OFFSET_SIZE;

        let name_offset = u16::from_be_bytes(
            bytes[offset..offset + constants::NAME_OFFSET_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.name_offset".to_string(),
                    reason: "invalid name_offset bytes".to_string(),
                })?,
        );
        offset += constants::NAME_OFFSET_SIZE;

        let encoded_data_len = u16::from_be_bytes(
            bytes[offset..offset + constants::DATA_LEN_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.data_len".to_string(),
                    reason: "invalid data_len bytes".to_string(),
                })?,
        );
        offset += constants::DATA_LEN_SIZE;

        let is_external = (encoded_data_len & constants::EXTERNAL_DATA_FLAG) != 0;
        offset += 2;

        let outgoing_cluster_offset = u64::from_be_bytes(
            bytes[offset..offset + constants::OUTGOING_CLUSTER_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.outgoing_cluster_offset".to_string(),
                    reason: "invalid outgoing_cluster_offset bytes".to_string(),
                })?,
        );
        offset += constants::OUTGOING_CLUSTER_SIZE;

        let outgoing_edge_count = u32::from_be_bytes(
            bytes[offset..offset + constants::OUTGOING_COUNT_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.outgoing_edge_count".to_string(),
                    reason: "invalid outgoing_edge_count bytes".to_string(),
                })?,
        );
        offset += constants::OUTGOING_COUNT_SIZE;

        let incoming_cluster_offset = u64::from_be_bytes(
            bytes[offset..offset + constants::INCOMING_CLUSTER_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.incoming_cluster_offset".to_string(),
                    reason: "invalid incoming_cluster_offset bytes".to_string(),
                })?,
        );
        offset += constants::INCOMING_CLUSTER_SIZE;

        let incoming_edge_count = u32::from_be_bytes(
            bytes[offset..offset + constants::INCOMING_COUNT_SIZE]
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node_record.incoming_edge_count".to_string(),
                    reason: "invalid incoming_edge_count bytes".to_string(),
                })?,
        );
        offset += constants::INCOMING_COUNT_SIZE;

        assert_eq!(
            offset, FIXED_METADATA_SIZE,
            "Offset should be at end of fixed metadata"
        );

        let (data_inline, data_external_offset) = if is_external {
            let external_offset = if bytes.len() > offset {
                let ext_offset =
                    u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0u8; 8]));
                Some(ext_offset)
            } else {
                None
            };
            (None, external_offset)
        } else {
            (Some(bytes[offset..].to_vec()), None)
        };

        Ok(NodeRecordV3 {
            id,
            flags,
            kind_offset,
            name_offset,
            data_len: encoded_data_len,
            data_inline,
            data_external_offset,
            outgoing_cluster_offset,
            outgoing_edge_count,
            incoming_cluster_offset,
            incoming_edge_count,
        })
    }
}
