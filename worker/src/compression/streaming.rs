//! Streaming compression with R2 multipart upload support.
//!
//! This module provides streaming compression that works within Cloudflare Workers'
//! 128MB memory limit by:
//!
//! 1. Compressing data in chunks (not buffering entire file)
//! 2. Accumulating compressed output until we have enough for an R2 part (5-8MB)
//! 3. Uploading parts immediately to free memory
//!
//! Memory usage is approximately:
//! - Input buffer: ~1MB (streaming read chunks)
//! - Compressor state: ~1MB (compression context)
//! - Part buffer: 5-8MB (minimum R2 part size)
//! - Total: ~9MB regardless of file size
//!
//! ## Compression Format Differences
//!
//! - **zstd**: Supports concatenated frames. Each chunk can be compressed independently
//!   and the resulting frames can be concatenated. Decompressor handles this transparently.
//!
//! - **brotli**: Does NOT support concatenated streams. Must use a stateful compressor
//!   that maintains compression context across all chunks. This module uses
//!   `StatefulBrotliCompressor` for this purpose.
//!
//! - **gzip**: While technically supports concatenated streams, many tools don't handle
//!   them well. Uses `StatefulGzipCompressor` for maximum compatibility.

use sha2::{Digest, Sha256};
use std::io::Write;

use super::config::{CompressionLevel, CompressionType};
use crate::error::{WorkerError, WorkerResult};
use crate::storage::TARGET_PART_SIZE;

/// Streaming compressor with hash computation.
///
/// This compressor accumulates compressed data and signals when a part is ready
/// to be uploaded. It computes the SHA256 hash of the compressed output as it goes.
pub struct StreamingCompressor {
    /// Accumulated compressed data (part buffer).
    buffer: Vec<u8>,

    /// Hash of compressed data (computed incrementally).
    hasher: Sha256,

    /// Total compressed bytes produced so far.
    total_size: u64,

    /// Compression type being used.
    compression: CompressionType,

    /// Target size before signaling part is ready.
    target_part_size: usize,

    /// Compression level to use.
    level: CompressionLevel,
}

impl StreamingCompressor {
    /// Create a new streaming compressor.
    ///
    /// # Arguments
    /// * `compression` - The compression type to use
    /// * `level` - Compression level
    /// * `target_part_size` - Target size for each part (default: 8MB)
    pub fn new(
        compression: CompressionType,
        level: CompressionLevel,
        target_part_size: usize,
    ) -> Self {
        Self {
            // Pre-allocate with some extra room to avoid reallocation
            buffer: Vec::with_capacity(target_part_size + 1024 * 1024),
            hasher: Sha256::new(),
            total_size: 0,
            compression,
            target_part_size,
            level,
        }
    }

    /// Create a new streaming compressor with default target part size.
    pub fn with_defaults(compression: CompressionType, level: CompressionLevel) -> Self {
        Self::new(compression, level, TARGET_PART_SIZE)
    }

    /// Compress a chunk of input data.
    ///
    /// This compresses the input chunk and adds it to the internal buffer.
    /// If the buffer reaches the target part size, returns exactly target_part_size
    /// bytes so they can be uploaded as a multipart part.
    ///
    /// IMPORTANT: R2 multipart uploads require all non-trailing parts to have
    /// exactly the same size. This method ensures that by returning exactly
    /// target_part_size bytes and keeping any excess in the buffer.
    ///
    /// # Arguments
    /// * `input` - Chunk of uncompressed data
    ///
    /// # Returns
    /// * `Ok(Some(data))` - Exactly target_part_size bytes ready for upload
    /// * `Ok(None)` - Buffer is not yet full, continue accumulating
    pub fn compress_chunk(&mut self, input: &[u8]) -> WorkerResult<Option<Vec<u8>>> {
        let compressed = self.compress_data(input)?;

        // Update hash and accumulate
        self.hasher.update(&compressed);
        self.buffer.extend(compressed);

        // Check if we have enough for a part
        // CRITICAL: Return exactly target_part_size bytes, keeping excess for next part
        if self.buffer.len() >= self.target_part_size {
            // Split buffer: take exactly target_part_size, keep remainder
            let remainder = self.buffer.split_off(self.target_part_size);
            let part_data = std::mem::replace(&mut self.buffer, remainder);
            self.total_size += part_data.len() as u64;
            Ok(Some(part_data))
        } else {
            Ok(None)
        }
    }

    /// Internal compression dispatch.
    fn compress_data(&self, input: &[u8]) -> WorkerResult<Vec<u8>> {
        match self.compression {
            CompressionType::None => Ok(input.to_vec()),
            CompressionType::Zstd => {
                // Each chunk is compressed as an independent zstd frame.
                // This is slightly less efficient than streaming compression
                // but works with the WASM bindings we have.
                super::js_zstd::compress(input, self.level.to_zstd_level())
            }
            CompressionType::Brotli => {
                // Use streaming brotli compression
                compress_brotli_chunk(input, &self.level)
            }
            CompressionType::Gzip => {
                // Note: Gzip streaming should use StatefulGzipCompressor instead.
                // This is a fallback for small files using buffered compression.
                use flate2::write::GzEncoder;
                use std::io::Write;

                let level = flate2::Compression::new(self.level.to_gzip_level());

                let mut encoder = GzEncoder::new(Vec::new(), level);
                encoder.write_all(input).map_err(|e| {
                    WorkerError::Compression(format!("Gzip compression failed: {:?}", e))
                })?;
                Ok(encoder.finish().map_err(|e| {
                    WorkerError::Compression(format!("Gzip finish failed: {:?}", e))
                })?)
            }
            CompressionType::Xz => {
                // Note: XZ streaming should use StatefulXzCompressor instead.
                // This is a fallback for small files using buffered compression.
                use lzma_rust2::{XzOptions, XzWriter};
                use std::io::Write;

                let options = XzOptions::with_preset(self.level.to_xz_preset());

                let mut compressed = Vec::new();
                {
                    let mut encoder = XzWriter::new(&mut compressed, options).map_err(|e| {
                        WorkerError::Compression(format!("XZ init failed: {:?}", e))
                    })?;
                    encoder.write_all(input).map_err(|e| {
                        WorkerError::Compression(format!("XZ compression failed: {:?}", e))
                    })?;
                    encoder.finish().map_err(|e| {
                        WorkerError::Compression(format!("XZ finish failed: {:?}", e))
                    })?;
                }

                Ok(compressed)
            }
            // Bzip2 not supported for compression in worker
            CompressionType::Bzip2 => Ok(input.to_vec()),
        }
    }

    /// Get remaining buffer contents and finalize hash.
    ///
    /// Call this after processing all input chunks. Returns:
    /// - Remaining compressed data (may be empty or less than target size)
    /// - SHA256 hash of all compressed data (hex-encoded)
    /// - Total size of compressed data
    pub fn finish(self) -> StreamingCompressionResult {
        let file_hash = hex::encode(self.hasher.finalize());
        let remaining_size = self.buffer.len() as u64;

        StreamingCompressionResult {
            remaining_data: self.buffer,
            file_hash,
            total_size: self.total_size + remaining_size,
            compression: self.compression,
        }
    }
}

/// Result of finishing a streaming compression.
pub struct StreamingCompressionResult {
    /// Remaining data in the buffer (last part, may be < 5MB).
    pub remaining_data: Vec<u8>,

    /// SHA256 hash of all compressed data (hex-encoded).
    pub file_hash: String,

    /// Total size of all compressed data.
    pub total_size: u64,

    /// Compression type used.
    pub compression: CompressionType,
}

/// Compress a chunk using Brotli.
///
/// Each chunk is compressed independently. While this is slightly less efficient
/// than true streaming compression, it allows us to work with chunk-at-a-time
/// processing which fits the multipart upload model.
fn compress_brotli_chunk(input: &[u8], level: &CompressionLevel) -> WorkerResult<Vec<u8>> {
    let mut compressed = Vec::new();
    let params = brotli::enc::BrotliEncoderParams {
        quality: level.to_brotli_quality(),
        lgwin: 22, // Window size (22 = 4MB)
        ..Default::default()
    };

    brotli::BrotliCompress(&mut std::io::Cursor::new(input), &mut compressed, &params).map_err(
        |e| WorkerError::Compression(format!("Brotli streaming compression failed: {:?}", e)),
    )?;

    Ok(compressed)
}

/// Streaming NAR hasher.
///
/// Computes the SHA256 hash of NAR data as it streams through.
/// This is separate from the file (compressed) hash computed by StreamingCompressor.
pub struct NarHasher {
    hasher: Sha256,
    size: u64,
}

impl NarHasher {
    /// Create a new NAR hasher.
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            size: 0,
        }
    }

    /// Update hash with NAR data chunk.
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
        self.size += data.len() as u64;
    }

    /// Finalize and return the hash (hex-encoded) and total size.
    pub fn finalize(self) -> (String, u64) {
        let hash = hex::encode(self.hasher.finalize());
        (hash, self.size)
    }
}

impl Default for NarHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Stateful streaming brotli compressor.
///
/// Unlike `StreamingCompressor` which compresses each chunk independently,
/// this compressor maintains brotli compression state across all input chunks,
/// producing a single valid brotli stream that can be decompressed.
///
/// This is necessary because brotli (unlike zstd) does not support concatenated
/// streams - concatenating independently compressed brotli blocks produces
/// invalid/corrupt output.
pub struct StatefulBrotliCompressor {
    /// The brotli compressor wrapping our part collector.
    compressor: brotli::CompressorWriter<PartCollector>,

    /// Hash of compressed data (computed from parts as they're extracted).
    hasher: Sha256,

    /// Total compressed bytes produced so far (in completed parts).
    total_size: u64,
}

/// Internal writer that collects compressed output and splits into parts.
struct PartCollector {
    /// Buffer for compressed output.
    buffer: Vec<u8>,

    /// Parts that are ready for upload.
    ready_parts: Vec<Vec<u8>>,

    /// Target size for each part.
    target_part_size: usize,
}

impl PartCollector {
    fn new(target_part_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(target_part_size + 1024 * 1024),
            ready_parts: Vec::new(),
            target_part_size,
        }
    }

    /// Take any ready parts from the collector.
    fn take_ready_parts(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.ready_parts)
    }

    /// Get remaining buffer contents.
    fn take_remaining(self) -> Vec<u8> {
        self.buffer
    }
}

impl Write for PartCollector {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Split off complete parts
        while self.buffer.len() >= self.target_part_size {
            let remainder = self.buffer.split_off(self.target_part_size);
            let part = std::mem::replace(&mut self.buffer, remainder);
            self.ready_parts.push(part);
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl StatefulBrotliCompressor {
    /// Create a new stateful brotli compressor.
    pub fn new(level: CompressionLevel, target_part_size: usize) -> Self {
        let collector = PartCollector::new(target_part_size);
        let params = brotli::enc::BrotliEncoderParams {
            quality: level.to_brotli_quality(),
            lgwin: 22, // Window size (22 = 4MB)
            ..Default::default()
        };

        let compressor = brotli::CompressorWriter::with_params(collector, 4096, &params);

        Self {
            compressor,
            hasher: Sha256::new(),
            total_size: 0,
        }
    }

    /// Create with default target part size.
    pub fn with_defaults(level: CompressionLevel) -> Self {
        Self::new(level, TARGET_PART_SIZE)
    }

    /// Compress a chunk of input data.
    ///
    /// Returns any parts that are ready for upload (exactly target_part_size bytes each).
    pub fn compress_chunk(&mut self, input: &[u8]) -> WorkerResult<Vec<Vec<u8>>> {
        // Write input to the compressor
        self.compressor
            .write_all(input)
            .map_err(|e| WorkerError::Compression(format!("Brotli compression failed: {:?}", e)))?;

        // Extract any ready parts
        let parts = self.compressor.get_mut().take_ready_parts();

        // Update hash and size for each part
        for part in &parts {
            self.hasher.update(part);
            self.total_size += part.len() as u64;
        }

        Ok(parts)
    }

    /// Finish compression and return remaining data plus hash.
    pub fn finish(self) -> WorkerResult<StatefulBrotliResult> {
        // Finish the compressor to flush all remaining data
        // into_inner() returns the inner writer directly (not a Result)
        let collector = self.compressor.into_inner();

        let remaining_data = collector.take_remaining();

        // Update hash with remaining data
        let mut hasher = self.hasher;
        hasher.update(&remaining_data);
        let file_hash = hex::encode(hasher.finalize());

        let remaining_size = remaining_data.len() as u64;

        Ok(StatefulBrotliResult {
            remaining_data,
            file_hash,
            total_size: self.total_size + remaining_size,
        })
    }
}

/// Result of finishing a stateful brotli compression.
pub struct StatefulBrotliResult {
    /// Remaining data in the buffer (last part, may be < 5MB).
    pub remaining_data: Vec<u8>,

    /// SHA256 hash of all compressed data (hex-encoded).
    pub file_hash: String,

    /// Total size of all compressed data.
    pub total_size: u64,
}

/// Stateful streaming gzip compressor.
///
/// Similar to `StatefulBrotliCompressor`, this maintains gzip compression state
/// across all input chunks, producing a single valid gzip stream.
///
/// Gzip (like brotli) does not support concatenated streams well - while
/// technically valid, many tools don't handle concatenated gzip files correctly.
/// Using a stateful compressor ensures maximum compatibility.
pub struct StatefulGzipCompressor {
    /// The gzip encoder wrapping our part collector.
    encoder: flate2::write::GzEncoder<PartCollector>,

    /// Hash of compressed data (computed from parts as they're extracted).
    hasher: Sha256,

    /// Total compressed bytes produced so far (in completed parts).
    total_size: u64,
}

impl StatefulGzipCompressor {
    /// Create a new stateful gzip compressor.
    pub fn new(level: CompressionLevel, target_part_size: usize) -> Self {
        let collector = PartCollector::new(target_part_size);
        let encoder = flate2::write::GzEncoder::new(
            collector,
            flate2::Compression::new(level.to_gzip_level()),
        );

        Self {
            encoder,
            hasher: Sha256::new(),
            total_size: 0,
        }
    }

    /// Create with default target part size.
    pub fn with_defaults(level: CompressionLevel) -> Self {
        Self::new(level, TARGET_PART_SIZE)
    }

    /// Compress a chunk of input data.
    ///
    /// Returns any parts that are ready for upload (exactly target_part_size bytes each).
    pub fn compress_chunk(&mut self, input: &[u8]) -> WorkerResult<Vec<Vec<u8>>> {
        // Write input to the encoder
        self.encoder
            .write_all(input)
            .map_err(|e| WorkerError::Compression(format!("Gzip compression failed: {:?}", e)))?;

        // Extract any ready parts
        let parts = self.encoder.get_mut().take_ready_parts();

        // Update hash and size for each part
        for part in &parts {
            self.hasher.update(part);
            self.total_size += part.len() as u64;
        }

        Ok(parts)
    }

    /// Finish compression and return remaining data plus hash.
    pub fn finish(self) -> WorkerResult<StatefulGzipResult> {
        // Finish the encoder to flush all remaining data and write gzip trailer
        let collector = self
            .encoder
            .finish()
            .map_err(|e| WorkerError::Compression(format!("Gzip finish failed: {:?}", e)))?;

        let remaining_data = collector.take_remaining();

        // Update hash with remaining data
        let mut hasher = self.hasher;
        hasher.update(&remaining_data);
        let file_hash = hex::encode(hasher.finalize());

        let remaining_size = remaining_data.len() as u64;

        Ok(StatefulGzipResult {
            remaining_data,
            file_hash,
            total_size: self.total_size + remaining_size,
        })
    }
}

/// Result of finishing a stateful gzip compression.
pub struct StatefulGzipResult {
    /// Remaining data in the buffer (last part, may be < 5MB).
    pub remaining_data: Vec<u8>,

    /// SHA256 hash of all compressed data (hex-encoded).
    pub file_hash: String,

    /// Total size of all compressed data.
    pub total_size: u64,
}

/// Stateful streaming XZ/LZMA2 compressor.
///
/// Similar to `StatefulBrotliCompressor`, this maintains XZ compression state
/// across all input chunks, producing a single valid XZ stream.
///
/// XZ does not support concatenated streams - each XZ file must be a complete
/// stream with proper header and footer. Using a stateful compressor ensures
/// we produce a valid, decompressable output.
pub struct StatefulXzCompressor {
    /// The XZ encoder wrapping our part collector.
    encoder: lzma_rust2::XzWriter<PartCollector>,

    /// Hash of compressed data (computed from parts as they're extracted).
    hasher: Sha256,

    /// Total compressed bytes produced so far (in completed parts).
    total_size: u64,
}

impl StatefulXzCompressor {
    /// Create a new stateful XZ compressor.
    pub fn new(level: CompressionLevel, target_part_size: usize) -> WorkerResult<Self> {
        use lzma_rust2::XzOptions;

        let options = XzOptions::with_preset(level.to_xz_preset());

        let collector = PartCollector::new(target_part_size);
        let encoder = lzma_rust2::XzWriter::new(collector, options)
            .map_err(|e| WorkerError::Compression(format!("XZ init failed: {:?}", e)))?;

        Ok(Self {
            encoder,
            hasher: Sha256::new(),
            total_size: 0,
        })
    }

    /// Create with default target part size.
    pub fn with_defaults(level: CompressionLevel) -> WorkerResult<Self> {
        Self::new(level, TARGET_PART_SIZE)
    }

    /// Compress a chunk of input data.
    ///
    /// Returns any parts that are ready for upload (exactly target_part_size bytes each).
    pub fn compress_chunk(&mut self, input: &[u8]) -> WorkerResult<Vec<Vec<u8>>> {
        // Write input to the encoder
        self.encoder
            .write_all(input)
            .map_err(|e| WorkerError::Compression(format!("XZ compression failed: {:?}", e)))?;

        // Extract any ready parts
        let parts = self.encoder.inner_mut().take_ready_parts();

        // Update hash and size for each part
        for part in &parts {
            self.hasher.update(part);
            self.total_size += part.len() as u64;
        }

        Ok(parts)
    }

    /// Finish compression and return remaining data plus hash.
    pub fn finish(self) -> WorkerResult<StatefulXzResult> {
        // Finish the encoder to flush all remaining data and write XZ footer
        let collector = self
            .encoder
            .finish()
            .map_err(|e| WorkerError::Compression(format!("XZ finish failed: {:?}", e)))?;

        let remaining_data = collector.take_remaining();

        // Update hash with remaining data
        let mut hasher = self.hasher;
        hasher.update(&remaining_data);
        let file_hash = hex::encode(hasher.finalize());

        let remaining_size = remaining_data.len() as u64;

        Ok(StatefulXzResult {
            remaining_data,
            file_hash,
            total_size: self.total_size + remaining_size,
        })
    }
}

/// Result of finishing a stateful XZ compression.
pub struct StatefulXzResult {
    /// Remaining data in the buffer (last part, may be < 5MB).
    pub remaining_data: Vec<u8>,

    /// SHA256 hash of all compressed data (hex-encoded).
    pub file_hash: String,

    /// Total size of all compressed data.
    pub total_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_compressor_small_input() {
        // Test that small inputs don't trigger part upload
        let mut compressor =
            StreamingCompressor::new(CompressionType::None, CompressionLevel::Default, 1024);

        let result = compressor.compress_chunk(b"hello world").unwrap();
        assert!(result.is_none()); // Should not be ready yet

        let finish = compressor.finish();
        assert_eq!(finish.remaining_data, b"hello world");
        assert_eq!(finish.total_size, 11);
    }

    #[test]
    fn test_streaming_compressor_triggers_part() {
        // Test that large inputs trigger part upload with exactly target_part_size bytes
        let mut compressor =
            StreamingCompressor::new(CompressionType::None, CompressionLevel::Default, 100);

        // Add data that exceeds target (150 bytes > 100 target)
        let data = vec![0u8; 150];
        let result = compressor.compress_chunk(&data).unwrap();
        assert!(result.is_some());
        // Part should be exactly target_part_size (100 bytes)
        assert_eq!(result.unwrap().len(), 100);

        let finish = compressor.finish();
        // Remaining 50 bytes should be in the buffer
        assert_eq!(finish.remaining_data.len(), 50);
    }

    #[test]
    fn test_streaming_compressor_multiple_parts() {
        // Simulate uploading a large file (like a 200MB NAR) in chunks
        // Using small sizes for the test
        let target_part_size = 100; // 100 bytes per part
        let chunk_size = 30; // Read 30 bytes at a time
        let total_size = 350; // Total "file" size

        let mut compressor = StreamingCompressor::new(
            CompressionType::None,
            CompressionLevel::Default,
            target_part_size,
        );

        let mut parts_uploaded = 0;
        let mut total_bytes_in_parts = 0;

        // Simulate reading in chunks
        for i in 0..(total_size / chunk_size) {
            let chunk = vec![(i % 256) as u8; chunk_size];
            if let Some(part_data) = compressor.compress_chunk(&chunk).unwrap() {
                parts_uploaded += 1;
                total_bytes_in_parts += part_data.len();
            }
        }

        // Handle remaining bytes
        let remaining = total_size % chunk_size;
        if remaining > 0 {
            let chunk = vec![0u8; remaining];
            if let Some(part_data) = compressor.compress_chunk(&chunk).unwrap() {
                parts_uploaded += 1;
                total_bytes_in_parts += part_data.len();
            }
        }

        let finish = compressor.finish();
        let final_bytes = finish.remaining_data.len();

        // Verify total bytes matches
        assert_eq!(
            total_bytes_in_parts + final_bytes,
            total_size,
            "Total bytes should match: {} parts + {} remaining = {} (expected {})",
            total_bytes_in_parts,
            final_bytes,
            total_bytes_in_parts + final_bytes,
            total_size
        );

        // Should have uploaded at least 3 parts (350 / 100 = 3.5)
        assert!(
            parts_uploaded >= 3,
            "Expected at least 3 parts, got {}",
            parts_uploaded
        );
    }

    #[test]
    fn test_streaming_compressor_with_brotli() {
        // Test that brotli compression produces smaller output
        let mut compressor =
            StreamingCompressor::new(CompressionType::Brotli, CompressionLevel::Default, 10000);

        // Highly compressible data (repeated pattern)
        let data = vec![0x42u8; 1000];
        let result = compressor.compress_chunk(&data).unwrap();
        assert!(result.is_none()); // Not enough for a part yet

        let finish = compressor.finish();

        // Brotli should compress this significantly
        assert!(
            finish.remaining_data.len() < data.len(),
            "Compressed size {} should be less than original {}",
            finish.remaining_data.len(),
            data.len()
        );

        // Hash should be computed correctly
        assert!(!finish.file_hash.is_empty());
        assert_eq!(finish.compression, CompressionType::Brotli);
    }

    #[test]
    fn test_streaming_compressor_hash_correctness() {
        // Verify that the hash is computed correctly over multiple chunks
        let mut compressor =
            StreamingCompressor::new(CompressionType::None, CompressionLevel::Default, 10000);

        compressor.compress_chunk(b"hello ").unwrap();
        compressor.compress_chunk(b"world").unwrap();

        let finish = compressor.finish();

        // For uncompressed data, the file hash should match the input hash
        // SHA256 of "hello world"
        assert_eq!(
            finish.file_hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_nar_hasher() {
        let mut hasher = NarHasher::new();
        hasher.update(b"hello ");
        hasher.update(b"world");
        let (hash, size) = hasher.finalize();

        assert_eq!(size, 11);
        // SHA256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_simulated_large_file_upload() {
        // Simulate the full upload flow for a large file
        // This mimics what handle_streaming_compressed_upload does

        const SIMULATED_NAR_SIZE: usize = 50 * 1024 * 1024; // 50MB
        const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks (typical stream read size)
        const TARGET_PART_SIZE: usize = 8 * 1024 * 1024; // 8MB parts

        // Create compressor and hasher
        let mut compressor = StreamingCompressor::new(
            CompressionType::None, // Use None for faster test
            CompressionLevel::Default,
            TARGET_PART_SIZE,
        );
        let mut nar_hasher = NarHasher::new();

        let mut parts: Vec<Vec<u8>> = Vec::new();

        // Simulate streaming the NAR file
        let mut bytes_processed = 0;
        while bytes_processed < SIMULATED_NAR_SIZE {
            let remaining = SIMULATED_NAR_SIZE - bytes_processed;
            let this_chunk_size = remaining.min(CHUNK_SIZE);

            // Generate some data (in reality this would be NAR bytes)
            let chunk = vec![(bytes_processed % 256) as u8; this_chunk_size];

            // Update NAR hash
            nar_hasher.update(&chunk);

            // Compress and check for part
            if let Some(part_data) = compressor.compress_chunk(&chunk).unwrap() {
                parts.push(part_data);
            }

            bytes_processed += this_chunk_size;
        }

        // Finalize
        let (nar_hash, nar_size) = nar_hasher.finalize();
        let compression_result = compressor.finish();

        // Add remaining data as final part
        if !compression_result.remaining_data.is_empty() {
            parts.push(compression_result.remaining_data);
        }

        // Verify results
        assert_eq!(nar_size as usize, SIMULATED_NAR_SIZE);
        assert!(!nar_hash.is_empty());
        assert!(!compression_result.file_hash.is_empty());

        // Calculate total uploaded bytes
        let total_uploaded: usize = parts.iter().map(|p| p.len()).sum();
        assert_eq!(total_uploaded, SIMULATED_NAR_SIZE);

        // Should have multiple parts (50MB / 8MB = ~6-7 parts)
        assert!(
            parts.len() >= 6,
            "Expected at least 6 parts for 50MB file, got {}",
            parts.len()
        );

        // R2 multipart requirement: all non-trailing parts must be exactly the same size
        // The last part can be smaller
        for (i, part) in parts.iter().enumerate() {
            if i < parts.len() - 1 {
                // All non-final parts must be exactly TARGET_PART_SIZE
                assert_eq!(
                    part.len(),
                    TARGET_PART_SIZE,
                    "Non-final part {} should be exactly {} bytes, got {}",
                    i,
                    TARGET_PART_SIZE,
                    part.len()
                );
            } else {
                // Final part can be any size <= TARGET_PART_SIZE
                assert!(
                    part.len() <= TARGET_PART_SIZE,
                    "Final part {} size {} exceeds maximum {}",
                    i,
                    part.len(),
                    TARGET_PART_SIZE
                );
            }
        }

        println!("Simulated 50MB upload:");
        println!("  NAR hash: {}", &nar_hash[..16]);
        println!("  File hash: {}", &compression_result.file_hash[..16]);
        println!("  Parts: {}", parts.len());
        println!("  Total size: {} bytes", total_uploaded);
    }

    #[test]
    fn test_stateful_brotli_compressor() {
        // Test that stateful brotli produces decompressable output
        let mut compressor = StatefulBrotliCompressor::new(CompressionLevel::Default, 1000);

        // Add data in multiple chunks
        let data1 = b"Hello, this is some test data. ";
        let data2 = b"It should compress well with brotli. ";
        let data3 = b"Multiple chunks should produce a single valid stream.";

        let parts1 = compressor.compress_chunk(data1).unwrap();
        let parts2 = compressor.compress_chunk(data2).unwrap();
        let parts3 = compressor.compress_chunk(data3).unwrap();

        // Finish compression
        let result = compressor.finish().unwrap();

        // Collect all compressed data
        let mut compressed: Vec<u8> = Vec::new();
        for part in parts1.iter().chain(parts2.iter()).chain(parts3.iter()) {
            compressed.extend(part);
        }
        compressed.extend(&result.remaining_data);

        // Verify we got compressed data
        assert!(!compressed.is_empty());
        assert!(!result.file_hash.is_empty());

        // Verify it can be decompressed
        let mut decompressed = Vec::new();
        brotli::BrotliDecompress(&mut std::io::Cursor::new(&compressed), &mut decompressed)
            .expect("Brotli decompression should succeed");

        // Verify decompressed matches original
        let original: Vec<u8> = data1
            .iter()
            .chain(data2.iter())
            .chain(data3.iter())
            .copied()
            .collect();
        assert_eq!(
            decompressed, original,
            "Decompressed data should match original"
        );

        println!("Stateful brotli test:");
        println!("  Original size: {} bytes", original.len());
        println!("  Compressed size: {} bytes", compressed.len());
        println!(
            "  Compression ratio: {:.1}%",
            (1.0 - compressed.len() as f64 / original.len() as f64) * 100.0
        );
    }

    #[test]
    fn test_stateful_brotli_large_file() {
        // Test stateful brotli with a larger file that produces multiple parts
        const TARGET_PART_SIZE: usize = 1000;
        const CHUNK_SIZE: usize = 200;
        const TOTAL_SIZE: usize = 10000;

        let mut compressor =
            StatefulBrotliCompressor::new(CompressionLevel::Default, TARGET_PART_SIZE);

        let mut all_parts: Vec<Vec<u8>> = Vec::new();
        let mut original_data: Vec<u8> = Vec::new();

        // Generate and compress data in chunks
        let mut bytes_processed = 0;
        while bytes_processed < TOTAL_SIZE {
            let remaining = TOTAL_SIZE - bytes_processed;
            let this_chunk_size = remaining.min(CHUNK_SIZE);

            // Generate some data with a pattern
            let chunk: Vec<u8> = (0..this_chunk_size)
                .map(|i| ((bytes_processed + i) % 256) as u8)
                .collect();

            original_data.extend(&chunk);

            let parts = compressor.compress_chunk(&chunk).unwrap();
            all_parts.extend(parts);

            bytes_processed += this_chunk_size;
        }

        // Finish compression
        let result = compressor.finish().unwrap();

        // Collect all compressed data
        let mut compressed: Vec<u8> = Vec::new();
        for part in &all_parts {
            compressed.extend(part);
        }
        compressed.extend(&result.remaining_data);

        // Verify it can be decompressed
        let mut decompressed = Vec::new();
        brotli::BrotliDecompress(&mut std::io::Cursor::new(&compressed), &mut decompressed)
            .expect("Brotli decompression should succeed for large file");

        // Verify decompressed matches original
        assert_eq!(
            decompressed.len(),
            original_data.len(),
            "Decompressed size should match original"
        );
        assert_eq!(
            decompressed, original_data,
            "Decompressed data should match original"
        );

        println!("Stateful brotli large file test:");
        println!("  Original size: {} bytes", original_data.len());
        println!("  Compressed size: {} bytes", compressed.len());
        println!("  Parts generated: {}", all_parts.len());
    }

    #[test]
    fn test_stateful_gzip_compressor() {
        // Test that stateful gzip produces decompressable output
        let mut compressor = StatefulGzipCompressor::new(CompressionLevel::Default, 1000);

        // Add data in multiple chunks
        let data1 = b"Hello, this is some test data. ";
        let data2 = b"It should compress well with gzip. ";
        let data3 = b"Multiple chunks should produce a single valid stream.";

        let parts1 = compressor.compress_chunk(data1).unwrap();
        let parts2 = compressor.compress_chunk(data2).unwrap();
        let parts3 = compressor.compress_chunk(data3).unwrap();

        // Finish compression
        let result = compressor.finish().unwrap();

        // Collect all compressed data
        let mut compressed: Vec<u8> = Vec::new();
        for part in parts1.iter().chain(parts2.iter()).chain(parts3.iter()) {
            compressed.extend(part);
        }
        compressed.extend(&result.remaining_data);

        // Verify we got compressed data
        assert!(!compressed.is_empty());
        assert!(!result.file_hash.is_empty());

        // Verify it can be decompressed using flate2
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decompressed = Vec::new();
        let mut decoder = GzDecoder::new(&compressed[..]);
        decoder
            .read_to_end(&mut decompressed)
            .expect("Gzip decompression should succeed");

        // Verify decompressed matches original
        let original: Vec<u8> = data1
            .iter()
            .chain(data2.iter())
            .chain(data3.iter())
            .copied()
            .collect();
        assert_eq!(
            decompressed, original,
            "Decompressed data should match original"
        );

        println!("Stateful gzip test:");
        println!("  Original size: {} bytes", original.len());
        println!("  Compressed size: {} bytes", compressed.len());
        println!(
            "  Compression ratio: {:.1}%",
            (1.0 - compressed.len() as f64 / original.len() as f64) * 100.0
        );
    }

    #[test]
    fn test_stateful_gzip_large_file() {
        // Test stateful gzip with a larger file that produces multiple parts
        const TARGET_PART_SIZE: usize = 1000;
        const CHUNK_SIZE: usize = 200;
        const TOTAL_SIZE: usize = 10000;

        let mut compressor =
            StatefulGzipCompressor::new(CompressionLevel::Default, TARGET_PART_SIZE);

        let mut all_parts: Vec<Vec<u8>> = Vec::new();
        let mut original_data: Vec<u8> = Vec::new();

        // Generate and compress data in chunks
        let mut bytes_processed = 0;
        while bytes_processed < TOTAL_SIZE {
            let remaining = TOTAL_SIZE - bytes_processed;
            let this_chunk_size = remaining.min(CHUNK_SIZE);

            // Generate some data with a pattern
            let chunk: Vec<u8> = (0..this_chunk_size)
                .map(|i| ((bytes_processed + i) % 256) as u8)
                .collect();

            original_data.extend(&chunk);

            let parts = compressor.compress_chunk(&chunk).unwrap();
            all_parts.extend(parts);

            bytes_processed += this_chunk_size;
        }

        // Finish compression
        let result = compressor.finish().unwrap();

        // Collect all compressed data
        let mut compressed: Vec<u8> = Vec::new();
        for part in &all_parts {
            compressed.extend(part);
        }
        compressed.extend(&result.remaining_data);

        // Verify it can be decompressed using flate2
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decompressed = Vec::new();
        let mut decoder = GzDecoder::new(&compressed[..]);
        decoder
            .read_to_end(&mut decompressed)
            .expect("Gzip decompression should succeed for large file");

        // Verify decompressed matches original
        assert_eq!(
            decompressed.len(),
            original_data.len(),
            "Decompressed size should match original"
        );
        assert_eq!(
            decompressed, original_data,
            "Decompressed data should match original"
        );

        println!("Stateful gzip large file test:");
        println!("  Original size: {} bytes", original_data.len());
        println!("  Compressed size: {} bytes", compressed.len());
        println!("  Parts generated: {}", all_parts.len());
    }
}
