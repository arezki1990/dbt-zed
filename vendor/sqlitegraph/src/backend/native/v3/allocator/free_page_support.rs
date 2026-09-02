/// Simple XOR checksum for page integrity
pub fn xor_checksum(data: &[u8]) -> u64 {
    const SEED: u64 = 0x5A5A5A5A5A5A5A5A;
    let mut checksum = SEED;
    for (i, &byte) in data.iter().enumerate() {
        checksum ^= (byte as u64) ^ (i as u64);
    }
    checksum
}

/// Page header format for free list linkage
///
/// Each free page stores a pointer to the next free page
/// in its header (first 8 bytes after standard page header).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreePageHeader {
    /// Next free page ID (0 if none)
    pub next_free: u64,
    /// Checksum for this free page
    pub checksum: u64,
}

impl FreePageHeader {
    /// Size of free page header in bytes
    pub const SIZE: usize = 16;

    /// Create a new free page header
    pub fn new(next_free: u64) -> Self {
        Self {
            next_free,
            checksum: 0,
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.next_free.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.checksum.to_le_bytes());
        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }

        let next_free = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let checksum = u64::from_le_bytes(bytes[8..16].try_into().ok()?);

        Some(Self {
            next_free,
            checksum,
        })
    }

    /// Calculate checksum for this header
    pub fn calculate_checksum(&self) -> u64 {
        xor_checksum(&self.next_free.to_le_bytes())
    }
}
