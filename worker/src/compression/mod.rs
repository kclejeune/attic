//! Compression utilities for WASM.
//!
//! Provides Brotli and Zstd compression for Cloudflare Workers.
//! - Brotli: Pure Rust implementation (brotli crate)
//! - Zstd: JavaScript WASM bindings (@bokuweb/zstd-wasm)
//!
//! # Compression Strategies
//!
//! ## Buffered Compression (`compress_buffer`)
//! Used for small files (<15MB). Buffers entire file, compresses, uploads.
//!
//! ## Streaming Compression (`StreamingCompressor`)
//! Used for large files (>15MB). Compresses in chunks, uploads via R2 multipart.
//! Memory stays under ~9MB regardless of file size.

mod config;
pub mod js_zstd;
mod stream;
mod streaming;

pub use config::{CompressionConfig, CompressionLevel, CompressionType};
pub use stream::compress_buffer;
pub use streaming::{
    NarHasher, StatefulBrotliCompressor, StatefulGzipCompressor, StreamingCompressor,
};
