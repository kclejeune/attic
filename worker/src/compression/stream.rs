//! Compression stream implementation for WASM.
//!
//! Provides buffered compression with dual hash computation:
//! - NAR hash: SHA256 of uncompressed data
//! - File hash: SHA256 of compressed data

use sha2::{Digest, Sha256};

use super::config::{CompressionConfig, CompressionLevel, CompressionType};
use crate::error::WorkerResult;

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
pub fn compress_buffer(input: &[u8], config: &CompressionConfig) -> WorkerResult<CompressionResult> {
    // Compute NAR hash (hash of uncompressed data)
    let mut nar_hasher = Sha256::new();
    nar_hasher.update(input);
    let nar_hash = hex::encode(nar_hasher.finalize());
    let nar_size = input.len() as u64;

    // Compress data
    let compressed = match config.r#type {
        CompressionType::None => input.to_vec(),
        CompressionType::Zstd => {
            let level = match config.level {
                CompressionLevel::Fastest => ruzstd::encoding::CompressionLevel::Fastest,
                CompressionLevel::Default => ruzstd::encoding::CompressionLevel::Default,
                CompressionLevel::Better => ruzstd::encoding::CompressionLevel::Better,
                CompressionLevel::Best => ruzstd::encoding::CompressionLevel::Best,
            };

            ruzstd::encoding::compress_to_vec(input, level)
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
        compression: config.r#type,
    })
}
