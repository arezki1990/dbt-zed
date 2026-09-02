use super::*;

/// Read a big-endian u32 from `bytes` at `start` covering `len` bytes,
/// returning an `InvalidHeader` error (instead of panicking via `unwrap()`)
/// if the slice is not the expected width.
pub(super) fn read_u32_be(
    bytes: &[u8],
    start: usize,
    len: usize,
    field: &str,
) -> NativeResult<u32> {
    let slice = bytes
        .get(start..start + len)
        .ok_or_else(|| NativeBackendError::InvalidHeader {
            field: field.to_string(),
            reason: format!("slice out of bounds at offset {start} (len {len})"),
        })?;
    let arr: [u8; 4] = slice
        .try_into()
        .map_err(|_| NativeBackendError::InvalidHeader {
            field: field.to_string(),
            reason: format!("expected 4 bytes at offset {start}, got {}", slice.len()),
        })?;
    Ok(u32::from_be_bytes(arr))
}

/// Read a big-endian u64 from `bytes` at `start` covering `len` bytes,
/// returning an `InvalidHeader` error (instead of panicking via `unwrap()`)
/// if the slice is not the expected width.
pub(super) fn read_u64_be(
    bytes: &[u8],
    start: usize,
    len: usize,
    field: &str,
) -> NativeResult<u64> {
    let slice = bytes
        .get(start..start + len)
        .ok_or_else(|| NativeBackendError::InvalidHeader {
            field: field.to_string(),
            reason: format!("slice out of bounds at offset {start} (len {len})"),
        })?;
    let arr: [u8; 8] = slice
        .try_into()
        .map_err(|_| NativeBackendError::InvalidHeader {
            field: field.to_string(),
            reason: format!("expected 8 bytes at offset {start}, got {}", slice.len()),
        })?;
    Ok(u64::from_be_bytes(arr))
}
