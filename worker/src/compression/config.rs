//! Compression configuration.

use serde::{Deserialize, Serialize};

/// Compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompressionType {
    /// No compression.
    #[serde(rename = "none")]
    None,

    /// Zstd compression.
    /// Uses @bokuweb/zstd-wasm via JS bindings for Cloudflare Workers.
    #[serde(rename = "zstd")]
    Zstd,

    /// Brotli compression (default).
    /// Pure Rust implementation with excellent WASM support.
    /// Best compression ratio for Nix binary caches.
    #[serde(rename = "br")]
    #[default]
    Brotli,

    /// Gzip compression.
    /// Uses native CompressionStream API for streaming large files.
    #[serde(rename = "gzip")]
    Gzip,

    /// XZ/LZMA2 compression.
    /// Pure Rust implementation via lzma-rust2.
    /// Compatible with original attic server's xz compression.
    #[serde(rename = "xz")]
    Xz,
}

impl CompressionType {
    /// Returns the compression type as a string for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionType::None => "none",
            CompressionType::Zstd => "zstd",
            CompressionType::Brotli => "br",
            CompressionType::Gzip => "gzip",
            CompressionType::Xz => "xz",
        }
    }

    /// Returns the file extension for this compression type.
    pub fn file_extension(&self) -> &'static str {
        match self {
            CompressionType::None => "",
            CompressionType::Zstd => ".zst",
            CompressionType::Brotli => ".br",
            CompressionType::Gzip => ".gz",
            CompressionType::Xz => ".xz",
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
            r#type: CompressionType::Brotli,
            level: CompressionLevel::Default,
        }
    }
}

impl CompressionConfig {
    /// Create a compression config from a database string.
    pub fn from_str(compression: &str) -> Self {
        let r#type = match compression {
            "none" => CompressionType::None,
            "zst" | "zstd" => CompressionType::Zstd,
            "br" | "brotli" => CompressionType::Brotli,
            "gz" | "gzip" => CompressionType::Gzip,
            "xz" | "lzma" => CompressionType::Xz,
            _ => CompressionType::Brotli, // Default to brotli for unknown
        };
        Self {
            r#type,
            level: CompressionLevel::Default,
        }
    }
}
