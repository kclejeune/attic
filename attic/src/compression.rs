//! Compression types shared between server and worker.
//!
//! This module provides common compression type definitions that are used
//! by both the attic server and the Cloudflare Worker implementation.
//! The actual compression implementations remain separate due to different
//! runtime requirements (tokio async vs WASM sync).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Compression type for NAR storage.
///
/// This enum represents the compression algorithms supported by Attic.
/// Both the server and worker implementations use this type for configuration
/// and narinfo generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressionType {
    /// No compression.
    None,

    /// Zstandard compression.
    ///
    /// Fast compression with good ratios. Supports concatenated frames.
    /// - Server: Uses async-compression with tokio
    /// - Worker: Uses zstd-wasm via JS bindings
    Zstd,

    /// Brotli compression.
    ///
    /// Excellent compression ratios, especially for text-like data.
    /// Does NOT support concatenated streams.
    /// - Server: Uses async-compression with tokio
    /// - Worker: Uses pure Rust brotli crate
    #[default]
    #[serde(rename = "br", alias = "brotli")]
    Brotli,

    /// Gzip compression.
    ///
    /// Wide compatibility, moderate compression.
    /// - Server: Not currently supported
    /// - Worker: Uses flate2 with rust_backend
    #[serde(alias = "gz")]
    Gzip,

    /// XZ/LZMA2 compression.
    ///
    /// High compression ratios, slower speed.
    /// Does NOT support concatenated streams.
    /// - Server: Uses async-compression with tokio
    /// - Worker: Uses lzma-rust2 (pure Rust)
    #[serde(alias = "lzma")]
    Xz,

    /// Bzip2 compression.
    ///
    /// Legacy format, primarily for compatibility with existing caches.
    /// - Server: Supported for reading narinfo
    /// - Worker: Not supported
    Bzip2,
}

impl CompressionType {
    /// Returns the canonical string representation for storage/narinfo.
    ///
    /// This is the value used in narinfo files and database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionType::None => "none",
            CompressionType::Zstd => "zstd",
            CompressionType::Brotli => "br",
            CompressionType::Gzip => "gzip",
            CompressionType::Xz => "xz",
            CompressionType::Bzip2 => "bzip2",
        }
    }

    /// Returns the file extension for compressed NARs.
    ///
    /// Used when constructing NAR URLs like `nar/<hash>.nar.zst`.
    pub fn file_extension(&self) -> &'static str {
        match self {
            CompressionType::None => "",
            CompressionType::Zstd => ".zst",
            CompressionType::Brotli => ".br",
            CompressionType::Gzip => ".gz",
            CompressionType::Xz => ".xz",
            CompressionType::Bzip2 => ".bz2",
        }
    }

    /// Returns whether this compression type supports concatenated streams.
    ///
    /// Zstd supports concatenating independently compressed frames.
    /// Other formats require stateful compression across all chunks.
    pub fn supports_concatenation(&self) -> bool {
        matches!(self, CompressionType::Zstd)
    }

    /// Returns all supported compression types.
    pub fn all() -> &'static [CompressionType] {
        &[
            CompressionType::None,
            CompressionType::Zstd,
            CompressionType::Brotli,
            CompressionType::Gzip,
            CompressionType::Xz,
            CompressionType::Bzip2,
        ]
    }
}

impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for CompressionType {
    type Err = InvalidCompressionType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(CompressionType::None),
            "zstd" | "zst" => Ok(CompressionType::Zstd),
            "br" | "brotli" => Ok(CompressionType::Brotli),
            "gzip" | "gz" => Ok(CompressionType::Gzip),
            "xz" | "lzma" => Ok(CompressionType::Xz),
            "bzip2" | "bz2" => Ok(CompressionType::Bzip2),
            _ => Err(InvalidCompressionType(s.to_string())),
        }
    }
}

/// Error returned when parsing an invalid compression type.
#[derive(Debug, Clone)]
pub struct InvalidCompressionType(pub String);

impl fmt::Display for InvalidCompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid compression type: '{}'. Valid options: none, zstd, br (brotli), gzip, xz",
            self.0
        )
    }
}

impl std::error::Error for InvalidCompressionType {}

/// Compression level preset.
///
/// These presets map to appropriate levels for each compression algorithm.
/// The actual numeric level depends on the compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressionLevel {
    /// Fastest compression, lowest ratio.
    Fastest,

    /// Balanced speed and compression ratio.
    #[default]
    Default,

    /// Better compression ratio, slower.
    Better,

    /// Best compression ratio, slowest.
    Best,
}

impl CompressionLevel {
    /// Convert to a zstd compression level (1-22).
    ///
    /// Returns u32 for compatibility with WASM zstd bindings.
    pub fn to_zstd_level(&self) -> u32 {
        match self {
            CompressionLevel::Fastest => 1,
            CompressionLevel::Default => 3,
            CompressionLevel::Better => 9,
            CompressionLevel::Best => 19,
        }
    }

    /// Convert to a zstd compression level as i32.
    ///
    /// For use with async-compression which expects i32.
    pub fn to_zstd_level_i32(&self) -> i32 {
        self.to_zstd_level() as i32
    }

    /// Convert to a brotli quality level (0-11).
    pub fn to_brotli_quality(&self) -> i32 {
        match self {
            CompressionLevel::Fastest => 1,
            CompressionLevel::Default => 4,
            CompressionLevel::Better => 7,
            CompressionLevel::Best => 11,
        }
    }

    /// Convert to a gzip/flate2 compression level (1-9).
    pub fn to_gzip_level(&self) -> u32 {
        match self {
            CompressionLevel::Fastest => 1,
            CompressionLevel::Default => 6,
            CompressionLevel::Better => 7,
            CompressionLevel::Best => 9,
        }
    }

    /// Convert to an XZ/LZMA preset (0-9).
    pub fn to_xz_preset(&self) -> u32 {
        match self {
            CompressionLevel::Fastest => 1,
            CompressionLevel::Default => 6,
            CompressionLevel::Better => 7,
            CompressionLevel::Best => 9,
        }
    }
}

/// Compression configuration.
///
/// This provides a common configuration structure for compression settings.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CompressionConfig {
    /// The compression type to use.
    #[serde(default, rename = "type")]
    pub compression_type: CompressionType,

    /// The compression level preset.
    #[serde(default)]
    pub level: CompressionLevel,
}

impl CompressionConfig {
    /// Create a new compression config with the given type and default level.
    pub fn new(compression_type: CompressionType) -> Self {
        Self {
            compression_type,
            level: CompressionLevel::Default,
        }
    }

    /// Create a new compression config with the given type and level.
    pub fn with_level(compression_type: CompressionType, level: CompressionLevel) -> Self {
        Self {
            compression_type,
            level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_type_roundtrip() {
        for ct in CompressionType::all() {
            let s = ct.as_str();
            let parsed: CompressionType = s.parse().unwrap();
            assert_eq!(*ct, parsed);
        }
    }

    #[test]
    fn test_compression_type_aliases() {
        assert_eq!(
            "brotli".parse::<CompressionType>().unwrap(),
            CompressionType::Brotli
        );
        assert_eq!(
            "br".parse::<CompressionType>().unwrap(),
            CompressionType::Brotli
        );
        assert_eq!(
            "gz".parse::<CompressionType>().unwrap(),
            CompressionType::Gzip
        );
        assert_eq!(
            "lzma".parse::<CompressionType>().unwrap(),
            CompressionType::Xz
        );
        assert_eq!(
            "zst".parse::<CompressionType>().unwrap(),
            CompressionType::Zstd
        );
        assert_eq!(
            "bz2".parse::<CompressionType>().unwrap(),
            CompressionType::Bzip2
        );
    }

    #[test]
    fn test_file_extensions() {
        assert_eq!(CompressionType::None.file_extension(), "");
        assert_eq!(CompressionType::Zstd.file_extension(), ".zst");
        assert_eq!(CompressionType::Brotli.file_extension(), ".br");
        assert_eq!(CompressionType::Gzip.file_extension(), ".gz");
        assert_eq!(CompressionType::Xz.file_extension(), ".xz");
        assert_eq!(CompressionType::Bzip2.file_extension(), ".bz2");
    }

    #[test]
    fn test_serde_roundtrip() {
        let ct = CompressionType::Brotli;
        let json = serde_json::to_string(&ct).unwrap();
        assert_eq!(json, "\"br\"");
        let parsed: CompressionType = serde_json::from_str(&json).unwrap();
        assert_eq!(ct, parsed);
    }

    #[test]
    fn test_serde_aliases() {
        // Test that serde aliases work for deserialization
        let brotli: CompressionType = serde_json::from_str("\"brotli\"").unwrap();
        assert_eq!(brotli, CompressionType::Brotli);

        let gz: CompressionType = serde_json::from_str("\"gz\"").unwrap();
        assert_eq!(gz, CompressionType::Gzip);
    }

    #[test]
    fn test_compression_levels() {
        assert_eq!(CompressionLevel::Fastest.to_zstd_level(), 1);
        assert_eq!(CompressionLevel::Best.to_zstd_level(), 19);
        assert_eq!(CompressionLevel::Default.to_brotli_quality(), 4);
        assert_eq!(CompressionLevel::Best.to_xz_preset(), 9);
    }
}
