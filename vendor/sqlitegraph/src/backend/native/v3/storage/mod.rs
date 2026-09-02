//! Storage media detection and adaptive configuration
//!
//! Automatically detects storage media type (SSD vs HDD) and configures
//! optimal page sizes and I/O strategies for improved performance.

pub mod adaptive_page;
pub mod media_detector;

pub use adaptive_page::{AdaptivePageManager, PageConfig};
pub use media_detector::{MediaDetector, MediaType};
