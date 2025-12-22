//! Compression stream implementation for WASM.
//!
//! Provides buffered compression with dual hash computation:
//! - NAR hash: SHA256 of uncompressed data
//! - File hash: SHA256 of compressed data

use sha2::{Digest, Sha256};

use super::config::{CompressionConfig, CompressionLevel, CompressionType};
use crate::error::{WorkerError, WorkerResult};

/// Result of a compression operation.
pub struct CompressionResult {
    /// Compressed data (or original if no compression).
    pub data: Vec<u8>,

    /// Hash of the original (uncompressed) NAR data (hex-encoded SHA256).
    pub nar_hash: String,

    /// Size of the original (uncompressed) NAR data.
    pub nar_size: u64,

    /// Hash of the compressed file (hex-encoded SHA256).
    pub file_hash: String,

    /// Size of the compressed file.
    pub file_size: u64,

    /// Compression type used.
    pub compression: CompressionType,
}

/// Compress a buffer with dual hashing.
///
/// This function:
/// 1. Computes SHA256 hash of the input (NAR hash)
/// 2. Compresses the input using the configured compression
/// 3. Computes SHA256 hash of the output (file hash)
pub fn compress_buffer(
    input: &[u8],
    config: &CompressionConfig,
) -> WorkerResult<CompressionResult> {
    // Compute NAR hash (hash of uncompressed data)
    let mut nar_hasher = Sha256::new();
    nar_hasher.update(input);
    let nar_hash = hex::encode(nar_hasher.finalize());
    let nar_size = input.len() as u64;

    // Compress data and track actual compression type used
    let (compressed, actual_compression) = match config.r#type {
        CompressionType::None => (input.to_vec(), CompressionType::None),
        CompressionType::Zstd => {
            // Map compression level to zstd level (1-22)
            let level = match config.level {
                CompressionLevel::Fastest => 1,
                CompressionLevel::Default => 3,
                CompressionLevel::Better => 9,
                CompressionLevel::Best => 19,
            };

            (
                super::js_zstd::compress(input, level)?,
                CompressionType::Zstd,
            )
        }
        CompressionType::Brotli => {
            // Map compression level to brotli quality (0-11)
            let quality = match config.level {
                CompressionLevel::Fastest => 1,
                CompressionLevel::Default => 4,
                CompressionLevel::Better => 7,
                CompressionLevel::Best => 11,
            };

            let mut compressed = Vec::new();
            let params = brotli::enc::BrotliEncoderParams {
                quality,
                lgwin: 22, // Window size (22 = 4MB)
                ..Default::default()
            };

            brotli::BrotliCompress(&mut std::io::Cursor::new(input), &mut compressed, &params)
                .map_err(|e| {
                    WorkerError::Compression(format!("Brotli compression failed: {:?}", e))
                })?;

            (compressed, CompressionType::Brotli)
        }
        CompressionType::Gzip => {
            // Map compression level to flate2 level (0-9)
            let level = match config.level {
                CompressionLevel::Fastest => flate2::Compression::fast(),
                CompressionLevel::Default => flate2::Compression::default(),
                CompressionLevel::Better => flate2::Compression::new(7),
                CompressionLevel::Best => flate2::Compression::best(),
            };

            use flate2::write::GzEncoder;
            use std::io::Write;

            let mut encoder = GzEncoder::new(Vec::new(), level);
            encoder.write_all(input).map_err(|e| {
                WorkerError::Compression(format!("Gzip compression failed: {:?}", e))
            })?;
            let compressed = encoder
                .finish()
                .map_err(|e| WorkerError::Compression(format!("Gzip finish failed: {:?}", e)))?;

            (compressed, CompressionType::Gzip)
        }
    };

    // Compute file hash (hash of compressed data)
    let mut file_hasher = Sha256::new();
    file_hasher.update(&compressed);
    let file_hash = hex::encode(file_hasher.finalize());
    let file_size = compressed.len() as u64;

    Ok(CompressionResult {
        data: compressed,
        nar_hash,
        nar_size,
        file_hash,
        file_size,
        compression: actual_compression,
    })
}
