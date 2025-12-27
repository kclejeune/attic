//! Compression stream implementation for WASM.
//!
//! Provides buffered compression with dual hash computation:
//! - NAR hash: SHA256 of uncompressed data
//! - File hash: SHA256 of compressed data

use sha2::{Digest, Sha256};

use super::config::{CompressionConfig, CompressionType};
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
        CompressionType::Zstd => (
            super::js_zstd::compress(input, config.level.to_zstd_level())?,
            CompressionType::Zstd,
        ),
        CompressionType::Brotli => {
            let mut compressed = Vec::new();
            let params = brotli::enc::BrotliEncoderParams {
                quality: config.level.to_brotli_quality(),
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
            use flate2::write::GzEncoder;
            use std::io::Write;

            let mut encoder = GzEncoder::new(
                Vec::new(),
                flate2::Compression::new(config.level.to_gzip_level()),
            );
            encoder.write_all(input).map_err(|e| {
                WorkerError::Compression(format!("Gzip compression failed: {:?}", e))
            })?;
            let compressed = encoder
                .finish()
                .map_err(|e| WorkerError::Compression(format!("Gzip finish failed: {:?}", e)))?;

            (compressed, CompressionType::Gzip)
        }
        CompressionType::Xz => {
            use lzma_rust2::{XzOptions, XzWriter};
            use std::io::Write;

            let options = XzOptions::with_preset(config.level.to_xz_preset());

            let mut compressed = Vec::new();
            {
                let mut encoder = XzWriter::new(&mut compressed, options)
                    .map_err(|e| WorkerError::Compression(format!("XZ init failed: {:?}", e)))?;
                encoder.write_all(input).map_err(|e| {
                    WorkerError::Compression(format!("XZ compression failed: {:?}", e))
                })?;
                encoder
                    .finish()
                    .map_err(|e| WorkerError::Compression(format!("XZ finish failed: {:?}", e)))?;
            }

            (compressed, CompressionType::Xz)
        }
        // Bzip2 not supported for compression in worker
        CompressionType::Bzip2 => (input.to_vec(), CompressionType::None),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::CompressionLevel;

    #[test]
    fn test_compress_buffer_none() {
        let input = b"Hello, World!";
        let config = CompressionConfig {
            r#type: CompressionType::None,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(input, &config).unwrap();

        // No compression means data unchanged
        assert_eq!(result.data, input);
        assert_eq!(result.compression, CompressionType::None);
        assert_eq!(result.nar_size, input.len() as u64);
        assert_eq!(result.file_size, input.len() as u64);

        // Hashes should be the same since data is unchanged
        assert_eq!(result.nar_hash, result.file_hash);

        // Verify hash is correct
        assert_eq!(
            result.nar_hash,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_compress_buffer_brotli() {
        // Use highly compressible data
        let input = vec![0x42u8; 1000];
        let config = CompressionConfig {
            r#type: CompressionType::Brotli,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(&input, &config).unwrap();

        // Should compress well
        assert!(result.file_size < result.nar_size);
        assert_eq!(result.compression, CompressionType::Brotli);
        assert_eq!(result.nar_size, 1000);

        // Verify decompression works
        let mut decompressed = Vec::new();
        brotli::BrotliDecompress(&mut std::io::Cursor::new(&result.data), &mut decompressed)
            .expect("Brotli decompression should succeed");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_compress_buffer_gzip() {
        let input = vec![0x42u8; 1000];
        let config = CompressionConfig {
            r#type: CompressionType::Gzip,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(&input, &config).unwrap();

        // Should compress well
        assert!(result.file_size < result.nar_size);
        assert_eq!(result.compression, CompressionType::Gzip);

        // Verify decompression works
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decompressed = Vec::new();
        let mut decoder = GzDecoder::new(&result.data[..]);
        decoder
            .read_to_end(&mut decompressed)
            .expect("Gzip decompression should succeed");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_compress_buffer_xz() {
        let input = vec![0x42u8; 1000];
        let config = CompressionConfig {
            r#type: CompressionType::Xz,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(&input, &config).unwrap();

        // Should compress well
        assert!(result.file_size < result.nar_size);
        assert_eq!(result.compression, CompressionType::Xz);

        // Verify decompression works
        use lzma_rust2::XzReader;
        use std::io::Read;

        let mut decompressed = Vec::new();
        let mut decoder = XzReader::new(&result.data[..], false);
        decoder
            .read_to_end(&mut decompressed)
            .expect("XZ decompression should succeed");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_compress_buffer_bzip2_fallback() {
        // Bzip2 is not supported, should fall back to no compression
        let input = b"test data";
        let config = CompressionConfig {
            r#type: CompressionType::Bzip2,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(input, &config).unwrap();

        // Should fall back to no compression
        assert_eq!(result.data, input);
        assert_eq!(result.compression, CompressionType::None);
    }

    #[test]
    fn test_compress_buffer_hash_correctness() {
        let input = b"test data for hashing";
        let config = CompressionConfig {
            r#type: CompressionType::Brotli,
            level: CompressionLevel::Default,
        };

        let result = compress_buffer(input, &config).unwrap();

        // NAR hash should be hash of original data
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(input);
        let expected_nar_hash = hex::encode(hasher.finalize());
        assert_eq!(result.nar_hash, expected_nar_hash);

        // File hash should be hash of compressed data
        let mut hasher = Sha256::new();
        hasher.update(&result.data);
        let expected_file_hash = hex::encode(hasher.finalize());
        assert_eq!(result.file_hash, expected_file_hash);

        // Hashes should be different (data was compressed)
        assert_ne!(result.nar_hash, result.file_hash);
    }

    // Note: Zstd tests require the WASM runtime and are tested via integration tests
}
