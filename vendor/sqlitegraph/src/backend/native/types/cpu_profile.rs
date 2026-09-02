//! CPU Profile for performance optimizations

use serde::{Deserialize, Serialize};

/// CPU Profile for performance optimizations
///
/// This enum allows application developers to choose CPU-specific optimizations
/// while maintaining backwards compatibility. All profiles are safe and will
/// gracefully degrade on unsupported hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CpuProfile {
    /// Generic profile compatible with all CPUs
    /// Uses portable optimizations without CPU-specific instructions
    #[default]
    Generic,
    /// Auto-detect and use optimal profile
    /// Runtime detection selects the best available profile
    Auto,
    /// Optimized for AMD Zen 4 (Ryzen 7000 series)
    /// Target: AMD Ryzen 7 7800X3D with AVX2, FMA, BMI2
    X86Zen4,
    /// Optimized for Intel CPUs with AVX2 support
    /// Target: Intel Skylake+ with 256-bit vector instructions
    X86Avx2,
    /// Optimized for Intel CPUs with AVX-512 support
    /// Target: Intel Xeon/Server with 512-bit vector instructions
    X86Avx512,
}

impl std::fmt::Display for CpuProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generic => write!(f, "generic"),
            Self::Auto => write!(f, "auto"),
            Self::X86Zen4 => write!(f, "x86-zen4"),
            Self::X86Avx2 => write!(f, "x86-avx2"),
            Self::X86Avx512 => write!(f, "x86-avx512"),
        }
    }
}

impl std::str::FromStr for CpuProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "generic" => Ok(Self::Generic),
            "auto" => Ok(Self::Auto),
            "x86-zen4" | "zen4" | "znver4" => Ok(Self::X86Zen4),
            "x86-avx2" | "avx2" => Ok(Self::X86Avx2),
            "x86-avx512" | "avx512" => Ok(Self::X86Avx512),
            _ => Err(format!(
                "Invalid CPU profile '{}'. Valid options: generic, auto, x86-zen4, x86-avx2, x86-avx512",
                s
            )),
        }
    }
}
