use super::{CompactEdgeRecord, Direction, NativeBackendError, NativeResult, V3EdgeCluster};
use crate::backend::native::v3::compression::edge_delta::{compress_edge_ids, decompress_edge_ids};

pub(crate) fn encode_edge_type_data(edge_type: Option<&str>) -> Vec<u8> {
    if let Some(edge_type) = edge_type {
        let edge_type_bytes = edge_type.as_bytes();
        let mut data = Vec::with_capacity(1 + edge_type_bytes.len());
        data.push(edge_type_bytes.len() as u8);
        data.extend_from_slice(edge_type_bytes);
        data
    } else {
        Vec::new()
    }
}

pub(crate) fn encode_weighted_edge_data(edge_type: Option<&str>, weight: f32) -> Vec<u8> {
    let edge_type_bytes = edge_type.map(str::as_bytes).unwrap_or(&[]);
    let mut data = Vec::with_capacity(6 + edge_type_bytes.len());
    data.push(0x80);
    data.extend_from_slice(&weight.to_be_bytes());
    data.push(edge_type_bytes.len() as u8);
    data.extend_from_slice(edge_type_bytes);
    data
}

pub(crate) fn extract_edge_type(edge_data: &[u8]) -> Option<String> {
    if edge_data.is_empty() {
        return None;
    }

    if edge_data[0] == 0x80 {
        if edge_data.len() < 6 {
            return None;
        }
        let type_len = edge_data[5] as usize;
        if edge_data.len() < 6 + type_len {
            return None;
        }
        Some(String::from_utf8_lossy(&edge_data[6..6 + type_len]).to_string())
    } else {
        let type_len = edge_data[0] as usize;
        if edge_data.len() < 1 + type_len {
            return None;
        }
        Some(String::from_utf8_lossy(&edge_data[1..1 + type_len]).to_string())
    }
}

pub(crate) fn extract_edge_weight(edge_data: &[u8]) -> f32 {
    if edge_data.len() >= 5 && edge_data[0] == 0x80 {
        f32::from_be_bytes([edge_data[1], edge_data[2], edge_data[3], edge_data[4]])
    } else {
        1.0
    }
}

pub(crate) fn serialize_cluster(cluster: &V3EdgeCluster) -> NativeResult<Vec<u8>> {
    let mut result = Vec::new();
    result.push(cluster.format_version);

    if cluster.format_version >= 2 {
        result.extend_from_slice(&cluster.src.to_be_bytes());
        result.push(if cluster.direction == Direction::Outgoing {
            0
        } else {
            1
        });
    }

    let count = cluster.edges.len() as u32;
    result.extend_from_slice(&count.to_be_bytes());

    if cluster.format_version >= 3 {
        result.push(1);

        let neighbor_ids: Vec<i64> = cluster.edges.iter().map(|e| e.neighbor_id).collect();
        let compressed_ids = compress_edge_ids(&neighbor_ids);

        result.extend_from_slice(&(compressed_ids.len() as u32).to_be_bytes());
        result.extend_from_slice(&compressed_ids);

        for edge in &cluster.edges {
            result.extend_from_slice(&edge.edge_type_offset.to_be_bytes());
            let data_len = edge.edge_data.len() as u16;
            result.extend_from_slice(&data_len.to_be_bytes());
            result.extend_from_slice(&edge.edge_data);
        }
    } else {
        for edge in &cluster.edges {
            let edge_bytes = edge.serialize();
            result.extend_from_slice(&edge_bytes);
        }
    }

    Ok(result)
}

pub(crate) fn deserialize_cluster(bytes: &[u8], page_id: u64) -> NativeResult<V3EdgeCluster> {
    if bytes.len() < 5 {
        return Err(NativeBackendError::DeserializationError {
            context: "Edge cluster bytes too short".to_string(),
        });
    }

    let format_version = bytes[0];
    if format_version > 3 {
        return Err(NativeBackendError::DeserializationError {
            context: format!("Unknown edge cluster format version: {}", format_version),
        });
    }

    let mut pos = 1;
    let (src, direction) = if format_version >= 2 {
        if bytes.len() < 1 + 8 + 1 {
            return Err(NativeBackendError::DeserializationError {
                context: "Edge cluster v2 header too short".to_string(),
            });
        }
        let src = i64::from_be_bytes(
            bytes[pos..pos + 8]
                .try_into()
                .expect("bounds checked above"),
        );
        pos += 8;
        let dir_byte = bytes[pos];
        pos += 1;
        let direction = if dir_byte == 1 {
            Direction::Incoming
        } else {
            Direction::Outgoing
        };
        (src, direction)
    } else {
        (0, Direction::Outgoing)
    };

    if pos + 4 > bytes.len() {
        return Err(NativeBackendError::DeserializationError {
            context: "Edge cluster truncated at edge count".to_string(),
        });
    }
    let count =
        u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
    pos += 4;

    let mut edges = Vec::with_capacity(count);

    if format_version >= 3 {
        if pos >= bytes.len() {
            return Err(NativeBackendError::DeserializationError {
                context: "Missing compression flag".to_string(),
            });
        }
        let compressed_flag = bytes[pos];
        pos += 1;

        if compressed_flag == 1 {
            if pos + 4 > bytes.len() {
                return Err(NativeBackendError::DeserializationError {
                    context: "Missing compressed ID length".to_string(),
                });
            }
            let compressed_len =
                u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                    as usize;
            pos += 4;

            if pos + compressed_len > bytes.len() {
                return Err(NativeBackendError::DeserializationError {
                    context: "Compressed IDs truncated".to_string(),
                });
            }
            let compressed_data = &bytes[pos..pos + compressed_len];
            pos += compressed_len;

            let neighbor_ids = decompress_edge_ids(compressed_data, count).map_err(|e| {
                NativeBackendError::DeserializationError {
                    context: format!("Failed to decompress edge IDs: {}", e),
                }
            })?;

            for neighbor_id in neighbor_ids {
                if pos + 4 > bytes.len() {
                    return Err(NativeBackendError::DeserializationError {
                        context: "Edge metadata truncated".to_string(),
                    });
                }

                let type_offset = u16::from_be_bytes(
                    bytes[pos..pos + 2]
                        .try_into()
                        .expect("bounds checked above"),
                );
                pos += 2;

                let data_len = u16::from_be_bytes(
                    bytes[pos..pos + 2]
                        .try_into()
                        .expect("bounds checked above"),
                ) as usize;
                pos += 2;

                let edge_data = if data_len > 0 {
                    if pos + data_len > bytes.len() {
                        return Err(NativeBackendError::DeserializationError {
                            context: "Edge data truncated".to_string(),
                        });
                    }
                    let data = bytes[pos..pos + data_len].to_vec();
                    pos += data_len;
                    data
                } else {
                    Vec::new()
                };

                edges.push(CompactEdgeRecord::new(neighbor_id, type_offset, edge_data));
            }
        }
    }

    if edges.is_empty() {
        for _ in 0..count {
            if pos + 12 > bytes.len() {
                return Err(NativeBackendError::DeserializationError {
                    context: "Edge data truncated".to_string(),
                });
            }

            let neighbor_id = i64::from_be_bytes(
                bytes[pos..pos + 8]
                    .try_into()
                    .expect("bounds checked above"),
            );
            pos += 8;

            let type_offset = u16::from_be_bytes(
                bytes[pos..pos + 2]
                    .try_into()
                    .expect("bounds checked above"),
            );
            pos += 2;

            let data_len = u16::from_be_bytes(
                bytes[pos..pos + 2]
                    .try_into()
                    .expect("bounds checked above"),
            ) as usize;
            pos += 2;

            let edge_data = if data_len > 0 {
                if pos + data_len > bytes.len() {
                    return Err(NativeBackendError::DeserializationError {
                        context: "Edge data truncated".to_string(),
                    });
                }
                bytes[pos..pos + data_len].to_vec()
            } else {
                Vec::new()
            };
            pos += data_len;

            edges.push(CompactEdgeRecord::new(neighbor_id, type_offset, edge_data));
        }
    }

    Ok(V3EdgeCluster {
        src,
        edges,
        direction,
        format_version,
        page_id,
    })
}
