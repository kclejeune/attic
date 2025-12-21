//! Compression configuration.

use serde::{Deserialize, Serialize};

/// Compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompressionType {
    /// No compression.
    #[serde(rename = "none")]
    #[default]
    None,

    /// Zstd compression.
    /// Note: ruzstd encoding is incomplete as of 0.8, so this currently panics.
    #[serde(rename = "zstd")]
    Zstd,
}

impl CompressionType {
    /// Returns the compression type as a string for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionType::None => "none",
            CompressionType::Zstd => "zstd",
        }
    }

    /// Returns the file extension for this compression type.
    pub fn file_extension(&self) -> &'static str {
        match self {
            CompressionType::None => "",
            CompressionType::Zstd => ".zst",
        }
    }
}

/// Compression configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct CompressionConfig {
    /// Compression type.
    #[serde(default)]
    pub r#type: CompressionType,

    /// Compression level.
    /// For zstd: Fastest (1), Default (3), Better (7), Best (11)
    #[serde(default = "default_level")]
    pub level: CompressionLevel,
}

/// Compression level for ruzstd.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub enum CompressionLevel {
    /// Fastest compression (roughly level 1).
    Fastest,
    /// Default compression (roughly level 3).
    #[default]
    Default,
    /// Better compression (roughly level 7).
    Better,
    /// Best compression (roughly level 11).
    Best,
}

fn default_level() -> CompressionLevel {
    CompressionLevel::Default
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            r#type: CompressionType::None,
            level: CompressionLevel::Default,
        }
    }
}
