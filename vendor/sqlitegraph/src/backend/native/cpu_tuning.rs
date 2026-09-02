//! Runtime CPU detection and optimization mapping for SQLiteGraph Native Backend.
//!
//! This module provides CPU-specific optimizations for graph traversal operations.
//! It detects CPU capabilities at runtime and maps them to the appropriate optimization
//! strategies while maintaining backwards compatibility and safety.

use crate::backend::native::types::CpuProfile;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global cached CPU profile to avoid repeated detection
static CACHED_CPU_PROFILE: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Convert CpuProfile to usize for atomic storage
#[inline]
fn profile_to_usize(profile: CpuProfile) -> usize {
    match profile {
        CpuProfile::Generic => 0,
        CpuProfile::Auto => 1,
        CpuProfile::X86Zen4 => 2,
        CpuProfile::X86Avx2 => 3,
        CpuProfile::X86Avx512 => 4,
    }
}

/// Convert usize back to CpuProfile
#[inline]
fn usize_to_profile(value: usize) -> CpuProfile {
    match value {
        0 => CpuProfile::Generic,
        1 => CpuProfile::Auto,
        2 => CpuProfile::X86Zen4,
        3 => CpuProfile::X86Avx2,
        4 => CpuProfile::X86Avx512,
        _ => CpuProfile::Generic,
    }
}

/// Detect the optimal CPU profile for the current system
///
/// This function performs runtime CPU detection to determine the best available
/// optimizations. It uses a conservative approach to ensure safety and compatibility.
///
/// # Returns
///
/// The optimal CpuProfile for the current CPU
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::backend::native::cpu_tuning::detect_cpu_profile;
///
/// let profile = detect_cpu_profile();
/// println!("Detected CPU profile: {}", profile);
/// ```
pub fn detect_cpu_profile() -> CpuProfile {
    // Check if we have a cached result
    let cached = CACHED_CPU_PROFILE.load(Ordering::Relaxed);
    if cached != usize::MAX {
        return usize_to_profile(cached);
    }

    let detected = if cfg!(target_arch = "x86_64") {
        detect_x86_64_profile()
    } else if cfg!(target_arch = "aarch64") {
        detect_aarch64_profile()
    } else {
        CpuProfile::Generic
    };

    // Cache the result for future calls
    let profile_int = profile_to_usize(detected);
    CACHED_CPU_PROFILE.store(profile_int, Ordering::Relaxed);

    detected
}

/// Detect CPU profile for x86_64 architecture
#[inline]
fn detect_x86_64_profile() -> CpuProfile {
    // Check for AVX-512 support first (highest performance)
    if has_avx512_support() {
        return CpuProfile::X86Avx512;
    }

    // Check for AVX2 support
    if has_avx2_support() {
        // Additional check for Zen 4 specific features
        if is_zen4_cpu() {
            return CpuProfile::X86Zen4;
        }
        return CpuProfile::X86Avx2;
    }

    // Fall back to generic profile
    CpuProfile::Generic
}

/// Detect CPU profile for AArch64 architecture
#[inline]
fn detect_aarch64_profile() -> CpuProfile {
    // AArch64 optimizations are less mature, use generic for now
    // Future: detect NEON, SVE capabilities
    CpuProfile::Generic
}

/// Check if the CPU supports AVX2 instructions
#[inline]
fn has_avx2_support() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Check if the CPU supports AVX-512 instructions
#[inline]
fn has_avx512_support() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // Check for core AVX-512 features
        std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512vl")
            && std::arch::is_x86_feature_detected!("avx512dq")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Heuristic detection for AMD Zen 4 CPUs
///
/// This uses a combination of CPUID detection and heuristics to identify
/// Zen 4 processors. It's designed to be conservative and will fall back
/// to AVX2 profile if detection is uncertain.
#[inline]
fn is_zen4_cpu() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // Basic Zen 4 detection using CPU vendor and family/model detection
        // This is a simplified heuristic - in production, you'd want more comprehensive detection
        if has_avx2_support() {
            // Zen 4 typically supports these specific features
            // We use heuristics to avoid unsafe CPUID calls
            std::arch::is_x86_feature_detected!("avx2")
                && std::arch::is_x86_feature_detected!("fma")
                && std::arch::is_x86_feature_detected!("bmi2")
                && std::arch::is_x86_feature_detected!("adx")
                && std::arch::is_x86_feature_detected!("sha")
        } else {
            false
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Get the effective CPU profile based on configuration and detection
///
/// This function resolves the `Auto` profile by performing runtime detection
/// and returns the actual profile to use for optimizations.
///
/// # Arguments
///
/// * `profile` - The configured CPU profile (may be `Auto`)
///
/// # Returns
///
/// The resolved CpuProfile to use for optimizations
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::backend::native::{CpuProfile, cpu_tuning::resolve_cpu_profile};
///
/// let profile = resolve_cpu_profile(CpuProfile::Auto);
/// println!("Using CPU profile: {}", profile);
/// ```
pub fn resolve_cpu_profile(profile: CpuProfile) -> CpuProfile {
    match profile {
        CpuProfile::Auto => detect_cpu_profile(),
        CpuProfile::Generic => CpuProfile::Generic,
        CpuProfile::X86Zen4 => {
            // Validate that the CPU actually supports Zen 4 features
            if cfg!(target_arch = "x86_64") && has_avx2_support() && is_zen4_cpu() {
                CpuProfile::X86Zen4
            } else {
                // Fall back to the best supported profile
                detect_cpu_profile()
            }
        }
        CpuProfile::X86Avx2 => {
            // Validate AVX2 support
            if has_avx2_support() {
                CpuProfile::X86Avx2
            } else {
                CpuProfile::Generic
            }
        }
        CpuProfile::X86Avx512 => {
            // Validate AVX-512 support
            if has_avx512_support() {
                CpuProfile::X86Avx512
            } else if has_avx2_support() {
                CpuProfile::X86Avx2
            } else {
                CpuProfile::Generic
            }
        }
    }
}

/// Check if a specific optimization is available for the current profile
///
/// This function allows checking for specific CPU features without
/// exposing the entire profile system to downstream code.
///
/// # Arguments
///
/// * `profile` - The CPU profile to check
/// * `feature` - The feature to check for
///
/// # Returns
///
/// true if the feature is available for the given profile
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::backend::native::{CpuProfile, cpu_tuning::has_feature};
///
/// let profile = CpuProfile::X86Zen4;
/// assert!(has_feature(profile, "avx2"));
/// ```
pub fn has_feature(profile: CpuProfile, feature: &str) -> bool {
    let resolved = resolve_cpu_profile(profile);

    match resolved {
        CpuProfile::Generic => false, // No specific CPU features
        CpuProfile::Auto => has_feature(CpuProfile::Auto, feature), // Resolve Auto first
        CpuProfile::X86Zen4 | CpuProfile::X86Avx2 => match feature.to_lowercase().as_str() {
            "avx2" | "fma" | "bmi2" => has_avx2_support(),
            "avx512" | "avx512f" | "avx512vl" => has_avx512_support(),
            _ => false,
        },
        CpuProfile::X86Avx512 => match feature.to_lowercase().as_str() {
            "avx2" | "fma" | "bmi2" | "avx512" | "avx512f" | "avx512vl" | "avx512dq" => {
                has_avx512_support()
            }
            _ => false,
        },
    }
}

/// Get optimization hints for a given CPU profile
///
/// Returns performance tuning hints that can be applied to algorithms
/// based on the detected CPU capabilities.
///
/// # Arguments
///
/// * `profile` - The CPU profile to get hints for
///
/// # Returns
///
/// A tuple of (cache_line_size, vector_width, branch_prediction_friendly)
///
/// # Examples
///
/// ```rust
/// use sqlitegraph::backend::native::{CpuProfile, cpu_tuning::get_optimization_hints};
///
/// let (cache_line, vector_width, branch_friendly) = get_optimization_hints(CpuProfile::X86Zen4);
/// println!("Cache line: {}, Vector width: {}, Branch friendly: {}",
///          cache_line, vector_width, branch_friendly);
/// ```
pub fn get_optimization_hints(profile: CpuProfile) -> (usize, usize, bool) {
    let resolved = resolve_cpu_profile(profile);

    match resolved {
        CpuProfile::Generic => (64, 0, false), // Baseline cache line, no SIMD, conservative branching
        CpuProfile::X86Zen4 => {
            // Zen 4 specific optimizations
            (64, 256, true) // 64-byte cache line, 256-bit AVX2, good branch prediction
        }
        CpuProfile::X86Avx2 => {
            // Intel AVX2 systems
            (64, 256, true)
        }
        CpuProfile::X86Avx512 => {
            // AVX-512 systems
            (64, 512, true)
        }
        CpuProfile::Auto => get_optimization_hints(CpuProfile::Auto), // Resolve Auto first
    }
}

/// Reset the cached CPU profile (useful for testing)
#[cfg(test)]
pub fn reset_cpu_profile_cache() {
    CACHED_CPU_PROFILE.store(usize::MAX, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cpu_profile() {
        let profile = detect_cpu_profile();
        // Should return a valid profile without panicking
        match profile {
            CpuProfile::Generic
            | CpuProfile::Auto
            | CpuProfile::X86Zen4
            | CpuProfile::X86Avx2
            | CpuProfile::X86Avx512 => {
                // Valid profile
            }
        }
    }

    #[test]
    fn test_resolve_cpu_profile() {
        // Test that Auto gets resolved to a concrete profile
        let auto_resolved = resolve_cpu_profile(CpuProfile::Auto);
        assert_ne!(auto_resolved, CpuProfile::Auto);

        // Test that Generic stays Generic
        let generic_resolved = resolve_cpu_profile(CpuProfile::Generic);
        assert_eq!(generic_resolved, CpuProfile::Generic);

        // Test that profiles fall back gracefully if CPU doesn't support features
        let _avx512_resolved = resolve_cpu_profile(CpuProfile::X86Avx512);
        // Should not panic, should fall back to best supported profile
    }

    #[test]
    fn test_has_feature() {
        // Test with Generic profile (should have no features)
        assert!(!has_feature(CpuProfile::Generic, "avx2"));
        assert!(!has_feature(CpuProfile::Generic, "avx512"));
        assert!(!has_feature(CpuProfile::Generic, "invalid"));

        // Test case insensitivity
        let profile = if has_avx2_support() {
            CpuProfile::X86Avx2
        } else {
            CpuProfile::Generic
        };

        if has_avx2_support() {
            assert!(has_feature(profile, "AVX2"));
            assert!(has_feature(profile, "avx2"));
            assert!(has_feature(profile, "AVX2"));
        }
    }

    #[test]
    fn test_get_optimization_hints() {
        let (cache_line, vector_width, branch_friendly) =
            get_optimization_hints(CpuProfile::Generic);
        assert_eq!(cache_line, 64);
        assert_eq!(vector_width, 0);
        assert!(!branch_friendly);

        // Test that profiles give reasonable hints
        let hints = get_optimization_hints(CpuProfile::X86Avx2);
        assert_eq!(hints.0, 64); // Cache line should be 64 bytes
        assert_eq!(hints.1, 256); // AVX2 width should be 256 bits
        assert!(hints.2); // Branch prediction should be good
    }

    #[test]
    fn test_caching() {
        reset_cpu_profile_cache();

        // First call should perform detection
        let profile1 = detect_cpu_profile();

        // Second call should use cached result
        let profile2 = detect_cpu_profile();

        // Should return the same profile
        assert_eq!(profile1, profile2);
    }

    #[test]
    fn test_profile_conversions() {
        assert_eq!(profile_to_usize(CpuProfile::Generic), 0);
        assert_eq!(profile_to_usize(CpuProfile::Auto), 1);
        assert_eq!(profile_to_usize(CpuProfile::X86Zen4), 2);
        assert_eq!(profile_to_usize(CpuProfile::X86Avx2), 3);
        assert_eq!(profile_to_usize(CpuProfile::X86Avx512), 4);

        assert_eq!(usize_to_profile(0), CpuProfile::Generic);
        assert_eq!(usize_to_profile(1), CpuProfile::Auto);
        assert_eq!(usize_to_profile(2), CpuProfile::X86Zen4);
        assert_eq!(usize_to_profile(3), CpuProfile::X86Avx2);
        assert_eq!(usize_to_profile(4), CpuProfile::X86Avx512);

        // Test invalid conversion
        assert_eq!(usize_to_profile(999), CpuProfile::Generic);
    }
}
