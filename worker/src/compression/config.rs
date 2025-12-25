//! Compression configuration.
//!
//! Re-exports shared compression types from the attic core crate
//! and provides worker-specific configuration helpers.

use serde::Deserialize;

// Re-export shared types from attic core
pub use attic::compression::{CompressionLevel, CompressionType};

/// Compression configuration for the worker.
///
/// This wraps the shared types with worker-specific configuration parsing.
#[derive(Debug, Clone, Deserialize)]
pub struct CompressionConfig {
    /// Compression type.
    #[serde(default)]
    pub r#type: CompressionType,

    /// Compression level.
    #[serde(default)]
    pub level: CompressionLevel,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            r#type: CompressionType::Brotli,
            level: CompressionLevel::Default,
        }
    }
}

impl CompressionConfig {
    /// Create a compression config from a database string.
    pub fn from_str(compression: &str) -> Self {
        let r#type = compression.parse().unwrap_or(CompressionType::Brotli);
        Self {
            r#type,
            level: CompressionLevel::Default,
        }
    }
}
