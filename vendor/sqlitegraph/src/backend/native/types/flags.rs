//! Bitfield flag types for marking node and edge state

use std::fmt;

/// Node flags bitfield for marking node state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NodeFlags(pub u32);

impl NodeFlags {
    /// Create empty flags
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Check if flag is set
    pub const fn contains(&self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    /// Set a flag
    pub const fn set(&self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    /// Clear a flag
    pub const fn clear(&self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }

    /// No flags set
    pub const NONE: Self = Self(0);

    /// Node is deleted
    pub const DELETED: Self = Self(1);
}

impl fmt::LowerHex for NodeFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", self.0)
    }
}

/// Edge flags bitfield for marking edge state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct EdgeFlags(pub u16);

impl EdgeFlags {
    /// Create empty flags
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Check if flag is set
    pub const fn contains(&self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    /// Set a flag
    pub const fn set(&self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    /// Clear a flag
    pub const fn clear(&self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }

    /// No flags set
    pub const NONE: Self = Self(0);
}

impl fmt::LowerHex for EdgeFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", self.0)
    }
}
