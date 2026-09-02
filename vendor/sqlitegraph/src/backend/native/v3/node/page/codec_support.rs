use super::*;
use crate::backend::native::types::NodeFlags;
use crate::backend::native::v3::node::record::constants as record_constants;

pub(super) fn skip_remaining_fields(data: &[u8], mut offset: usize) -> NativeResult<usize> {
    offset += 4;

    let (_, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.kind_offset".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let (_, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.name_offset".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let (encoded_data_len, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.data_len".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let is_external = (encoded_data_len & record_constants::EXTERNAL_DATA_FLAG) != 0;
    let data_len = encoded_data_len & record_constants::MAX_DATA_LEN;

    let (_, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.outgoing_cluster_offset".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let (_, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.outgoing_edge_count".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let (_, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.incoming_cluster_offset".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    let (_, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.incoming_edge_count".to_string(),
            reason: "invalid varint encoding".to_string(),
        })?;
    offset += bytes_read;

    if is_external {
        offset += 8;
    } else {
        offset += data_len as usize;
    }

    Ok(offset)
}

pub(super) fn decode_node_at_offset(
    data: &[u8],
    base_id: i64,
) -> NativeResult<Option<NodeRecordV3>> {
    let (node, _) = decode_full_node(data, base_id)?;
    Ok(Some(node))
}

pub(super) fn pack_nodes(
    nodes: &[NodeRecordV3],
    base_id: i64,
    used_bytes: u16,
) -> NativeResult<Vec<u8>> {
    let mut buffer = Vec::with_capacity(used_bytes as usize);

    for node in nodes {
        let delta = encode_id_delta(node.id(), base_id);
        buffer.extend_from_slice(&encode_varint(delta as u64));
        buffer.extend_from_slice(&node.flags.0.to_be_bytes());
        buffer.extend_from_slice(&encode_varint_u16(node.kind_offset));
        buffer.extend_from_slice(&encode_varint_u16(node.name_offset));

        let encoded_data_len = if node.is_external() {
            node.data_len | record_constants::EXTERNAL_DATA_FLAG
        } else {
            node.data_len
        };
        buffer.extend_from_slice(&encode_varint_u16(encoded_data_len));
        buffer.extend_from_slice(&encode_varint(node.outgoing_cluster_offset));
        buffer.extend_from_slice(&encode_varint(node.outgoing_edge_count as u64));
        buffer.extend_from_slice(&encode_varint(node.incoming_cluster_offset));
        buffer.extend_from_slice(&encode_varint(node.incoming_edge_count as u64));

        if let Some(data) = &node.data_inline {
            buffer.extend_from_slice(data);
        } else if let Some(offset) = node.data_external_offset {
            buffer.extend_from_slice(&offset.to_be_bytes());
        }
    }

    Ok(buffer)
}

pub(super) fn unpack_nodes(
    data: &[u8],
    base_id: i64,
    node_count: usize,
) -> NativeResult<(Vec<NodeRecordV3>, usize)> {
    let mut nodes = Vec::with_capacity(node_count);
    let mut offset = 0;

    for _ in 0..node_count {
        let (node, consumed) = decode_full_node(&data[offset..], base_id)?;
        nodes.push(node);
        offset += consumed;
    }

    Ok((nodes, offset))
}

fn decode_full_node(data: &[u8], base_id: i64) -> NativeResult<(NodeRecordV3, usize)> {
    let mut offset = 0;

    let (delta, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.id_delta".to_string(),
            reason: "invalid varint encoding for ID delta".to_string(),
        })?;
    offset += bytes_read;

    let id =
        decode_id_delta(delta as u32, base_id).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.id".to_string(),
            reason: format!(
                "failed to reconstruct ID from delta {} and base_id {}",
                delta, base_id
            ),
        })?;

    if offset + 4 > data.len() {
        return Err(NativeBackendError::InvalidHeader {
            field: "node.flags".to_string(),
            reason: "insufficient bytes for flags".to_string(),
        });
    }
    let flags = NodeFlags(u32::from_be_bytes(
        data.get(offset..offset + 4)
            .ok_or_else(|| NativeBackendError::InvalidHeader {
                field: "node.flags".to_string(),
                reason: "cannot read flag bytes".to_string(),
            })?
            .try_into()
            .map_err(|_| NativeBackendError::InvalidHeader {
                field: "node.flags".to_string(),
                reason: "invalid flag byte array".to_string(),
            })?,
    ));
    offset += 4;

    let (kind_offset, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.kind_offset".to_string(),
            reason: "invalid varint encoding for kind_offset".to_string(),
        })?;
    offset += bytes_read;

    let (name_offset, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.name_offset".to_string(),
            reason: "invalid varint encoding for name_offset".to_string(),
        })?;
    offset += bytes_read;

    let (encoded_data_len, bytes_read) =
        decode_varint_u16(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.data_len".to_string(),
            reason: "invalid varint encoding for data_len".to_string(),
        })?;
    offset += bytes_read;

    let is_external = (encoded_data_len & record_constants::EXTERNAL_DATA_FLAG) != 0;
    let data_len = encoded_data_len & record_constants::MAX_DATA_LEN;

    let (outgoing_cluster_offset, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.outgoing_cluster_offset".to_string(),
            reason: "invalid varint encoding for outgoing_cluster_offset".to_string(),
        })?;
    offset += bytes_read;

    let (outgoing_edge_count, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.outgoing_edge_count".to_string(),
            reason: "invalid varint encoding for outgoing_edge_count".to_string(),
        })?;
    let outgoing_edge_count = outgoing_edge_count as u32;
    offset += bytes_read;

    let (incoming_cluster_offset, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.incoming_cluster_offset".to_string(),
            reason: "invalid varint encoding for incoming_cluster_offset".to_string(),
        })?;
    offset += bytes_read;

    let (incoming_edge_count, bytes_read) =
        decode_varint(&data[offset..]).map_err(|_| NativeBackendError::InvalidHeader {
            field: "node.incoming_edge_count".to_string(),
            reason: "invalid varint encoding for incoming_edge_count".to_string(),
        })?;
    let incoming_edge_count = incoming_edge_count as u32;
    offset += bytes_read;

    let (data_inline, data_external_offset) = if is_external {
        if offset + 8 > data.len() {
            return Err(NativeBackendError::InvalidHeader {
                field: "node.data_external_offset".to_string(),
                reason: format!(
                    "insufficient bytes for external offset: need 8, have {}",
                    data.len().saturating_sub(offset)
                ),
            });
        }
        let ext_offset = u64::from_be_bytes(
            data.get(offset..offset + 8)
                .ok_or_else(|| NativeBackendError::InvalidHeader {
                    field: "node.data_external_offset".to_string(),
                    reason: "cannot read external offset bytes".to_string(),
                })?
                .try_into()
                .map_err(|_| NativeBackendError::InvalidHeader {
                    field: "node.data_external_offset".to_string(),
                    reason: "invalid external offset byte array".to_string(),
                })?,
        );
        offset += 8;
        (None, Some(ext_offset))
    } else {
        let data_end = offset + data_len as usize;
        if data_end > data.len() {
            return Err(NativeBackendError::InvalidHeader {
                field: "node.data_inline".to_string(),
                reason: format!(
                    "insufficient bytes for inline data: need {}, have {}",
                    data_len,
                    data.len().saturating_sub(offset)
                ),
            });
        }
        let inline_data = data[offset..data_end].to_vec();
        offset = data_end;
        (Some(inline_data), None)
    };

    Ok((
        NodeRecordV3 {
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
        },
        offset,
    ))
}
